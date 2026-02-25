# Benchmark Bug Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all 5 bugs and 1 quality issue discovered in the live benchmark (`docs/research/2026-02-25-live-benchmark-report.md`).

**Architecture:** Each bug is an independent fix with its own test(s). Fixes are ordered by priority (P0 first) and dependencies (Bug #1 before #2, since both touch file-walking patterns). No cross-bug dependencies otherwise.

**Tech Stack:** Rust, `ignore` crate (gitignore-aware walking), `git2` (blame), `serde_json`, `tokio::test`, `tempfile`

---

## Task 1: Bug #1 — `list_dir` recursive includes `.git/` internals (P0, Low effort)

**Problem:** `list_dir` uses `walkdir::WalkDir` which does NOT respect `.gitignore` and does NOT filter `.git/`. When `recursive=true`, the first ~150 entries are `.git/objects/`, `.git/hooks/`, etc., hitting the 200-item cap before real project files appear.

Other tools (`search_for_pattern` at `src/tools/file.rs:196`, `find_file` at `src/tools/file.rs:298`) already use `ignore::WalkBuilder` which handles this correctly.

**Files:**
- Modify: `src/tools/file.rs:111-115` (replace `walkdir::WalkDir` with `ignore::WalkBuilder`)
- Test: `src/tools/file.rs` (in-file `#[cfg(test)]` module)

**Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `src/tools/file.rs`. The test creates a temp dir with a `.git/` directory and verifies that `list_dir` recursive does NOT include `.git/` entries.

```rust
#[tokio::test]
async fn list_dir_recursive_excludes_git() {
    let dir = tempfile::tempdir().unwrap();
    // Create a .git directory with some objects (simulating a real repo)
    std::fs::create_dir_all(dir.path().join(".git/objects/ab")).unwrap();
    std::fs::write(dir.path().join(".git/objects/ab/1234"), "blob").unwrap();
    std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    // Create real project files
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("README.md"), "# Hello").unwrap();

    let result = ListDir
        .call(
            serde_json::json!({
                "path": dir.path().display().to_string(),
                "recursive": true
            }),
            &make_ctx(dir.path()).await,
        )
        .await
        .unwrap();

    let entries: Vec<&str> = result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();

    // Should contain project files
    assert!(entries.iter().any(|e| e.contains("src/main.rs")), "missing src/main.rs");
    assert!(entries.iter().any(|e| e.contains("README.md")), "missing README.md");
    // Should NOT contain .git internals
    assert!(
        !entries.iter().any(|e| e.contains(".git")),
        "should not include .git entries, got: {:?}",
        entries
    );
}
```

Note: This test needs a helper `make_ctx` — check if one already exists in the test module of `file.rs`. If not, create a minimal one (just needs an `Agent` with a project root). Look at the pattern in `tests/integration.rs:15-31` (`project_with_files`) or `src/tools/semantic.rs:152-163` (`project_ctx`).

**Step 2: Run test to verify it fails**

Run: `cargo test list_dir_recursive_excludes_git -- --nocapture`
Expected: FAIL — `.git/` entries appear in the result

**Step 3: Implement the fix**

In `src/tools/file.rs`, replace the `walkdir::WalkDir` walker (lines 111-115) with `ignore::WalkBuilder`:

```rust
// Before (broken):
let walker = walkdir::WalkDir::new(path)
    .max_depth(depth)
    .into_iter()
    .flatten()
    .filter(|e| e.depth() > 0);

// After (fixed):
let max_depth = if recursive { None } else { Some(1) };
let walker = ignore::WalkBuilder::new(path)
    .max_depth(max_depth)
    .hidden(true)       // skip hidden files/dirs (including .git/)
    .git_ignore(true)   // respect .gitignore
    .build()
    .flatten()
    .filter(|e| e.depth() > 0);
```

Note: `ignore::WalkBuilder` returns `ignore::DirEntry` (not `walkdir::DirEntry`), but both have `.path()`, `.file_type()`, and `.depth()` methods — the rest of the function body should work unchanged.

Check that `ignore` is already in `Cargo.toml` dependencies (it should be, since `search_for_pattern` and `find_file` already use it).

**Step 4: Run test to verify it passes**

Run: `cargo test list_dir_recursive_excludes_git -- --nocapture`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test`
Expected: All 173+ tests pass

**Step 6: Commit**

```bash
git add src/tools/file.rs
git commit -m "fix: exclude .git/ from list_dir recursive via ignore::WalkBuilder"
```

---

## Task 2: Bug #3 — `find_symbol` project-wide always returns empty (P0, Medium effort)

**Problem:** `find_symbol` without `relative_path` uses `workspace/symbol` LSP request (added in commit `7ad0d55`). This returns empty when the LSP hasn't finished indexing or doesn't support `workspace/symbol`. There's no fallback.

**Fix approach:** Add a tree-sitter fallback. When `workspace/symbol` returns empty, iterate source files and use the AST `list_functions` logic to find matching symbols. Tree-sitter is offline, instant, and already works.

**Files:**
- Modify: `src/tools/symbol.rs:405-437` (project-wide `find_symbol` fast path)
- Read: `src/ast/parser.rs` (understand `list_functions` / `parse_symbols` API)
- Test: `src/tools/symbol.rs` (in-file `#[cfg(test)]` module)

**Step 1: Understand the tree-sitter API**

Read `src/ast/parser.rs` to find:
- What function parses a file and returns symbols/functions
- What its return type looks like
- How `list_functions` tool in `src/tools/ast.rs` calls it

The key function is likely something like `parse_functions(path)` or `parse_file(path)` that returns structs with `name`, `kind`, `start_line`, `end_line`. We need to convert these to the same `SymbolInfo` format or to JSON matching the existing `symbol_to_json` output.

**Step 2: Write the failing test**

```rust
#[tokio::test]
async fn find_symbol_project_wide_uses_treesitter_fallback() {
    // Create a project with a Rust file containing a known function
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn my_unique_function() -> i32 { 42 }\n\npub struct MyUniqueStruct {\n    x: i32,\n}\n",
    ).unwrap();

    let ctx = make_ctx(dir.path()).await;

    // Project-wide search (no relative_path) — should find via tree-sitter fallback
    // since no LSP server will be running in tests
    let result = FindSymbol
        .call(serde_json::json!({ "pattern": "my_unique_function" }), &ctx)
        .await
        .unwrap();

    let symbols = result["symbols"].as_array().unwrap();
    assert!(
        !symbols.is_empty(),
        "project-wide find_symbol should find symbols via tree-sitter fallback, got: {:?}",
        result
    );
    assert!(
        symbols.iter().any(|s| s["name"].as_str().unwrap().contains("my_unique_function")),
        "should find my_unique_function, got: {:?}",
        symbols
    );
}
```

**Step 3: Run test to verify it fails**

Run: `cargo test find_symbol_project_wide_uses_treesitter_fallback -- --nocapture`
Expected: FAIL — returns empty `{"symbols": [], "total": 0}`

**Step 4: Implement the tree-sitter fallback**

In `src/tools/symbol.rs`, after the `workspace/symbol` loop (around line 436), add a fallback when `matches` is still empty:

```rust
// After the workspace/symbol loop (line 436):
// Fallback: if LSP returned nothing, try tree-sitter across all source files
if matches.is_empty() {
    let walker = ignore::WalkBuilder::new(&root).hidden(true).git_ignore(true).build();
    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let Some(lang) = ast::detect_language(path) else {
            continue;
        };
        // Use tree-sitter to get functions from this file
        if let Ok(functions) = crate::ast::parser::parse_functions(path) {
            for func in functions {
                if func.name.to_lowercase().contains(&pattern_lower) {
                    let rel = path.strip_prefix(&root).unwrap_or(path);
                    // Convert AST result to the same JSON format as LSP results
                    matches.push(json!({
                        "name": func.name,
                        "name_path": func.name_path,
                        "kind": func.kind,
                        "file": rel.display().to_string(),
                        "start_line": func.start_line,
                        "end_line": func.end_line,
                    }));
                }
            }
        }
        // Early cap to avoid scanning entire huge projects
        if matches.len() > guard.max_results {
            break;
        }
    }
}
```

Adapt the field names based on what `parse_functions` actually returns (check `src/ast/parser.rs`). The goal is to produce the same JSON shape as `symbol_to_json()` so the LLM sees a consistent format.

**Step 5: Run test to verify it passes**

Run: `cargo test find_symbol_project_wide_uses_treesitter_fallback -- --nocapture`
Expected: PASS

**Step 6: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 7: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "fix: add tree-sitter fallback for project-wide find_symbol"
```

---

## Task 3: Bug #2 — `get_symbols_overview` project-wide returns empty (P1, Medium effort)

**Problem:** `get_symbols_overview` with no path or `path="."` goes through the directory branch (line 224), which uses `max_depth(Some(1))` — it only looks at immediate children of root. If all source files are in subdirectories (e.g., `src/`), nothing is returned.

**Fix approach:** When `path` is absent, `"."`, or the project root, walk the full project tree (respecting `.gitignore`) and aggregate symbols from all source files. Cap with `OutputGuard::cap_files()`.

**Files:**
- Modify: `src/tools/symbol.rs:224-279` (directory branch of `get_symbols_overview`)
- Test: `src/tools/symbol.rs` (in-file `#[cfg(test)]` module)

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn get_symbols_overview_project_wide_finds_nested_files() {
    let dir = tempfile::tempdir().unwrap();
    // Files in subdirectories (not at root level)
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn nested_function() {}\n",
    ).unwrap();

    let ctx = make_ctx(dir.path()).await;

    // No path → should find files in subdirectories
    let result = GetSymbolsOverview
        .call(serde_json::json!({}), &ctx)
        .await
        .unwrap();

    let files = result["files"].as_array().unwrap();
    assert!(
        !files.is_empty(),
        "project-wide get_symbols_overview should find nested files, got: {:?}",
        result
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test get_symbols_overview_project_wide_finds_nested_files -- --nocapture`
Expected: FAIL — `files` is empty `[]`

**Step 3: Implement the fix**

In the directory branch of `get_symbols_overview` (around line 227-229), when `rel_path` is `"."` or empty (indicating project-wide), remove the `max_depth(Some(1))` limit:

```rust
// Determine depth: project root = recursive, subdirectory = shallow
let is_project_root = rel_path == "." || rel_path.is_empty();
let walker = ignore::WalkBuilder::new(&full_path)
    .max_depth(if is_project_root { None } else { Some(1) })
    .hidden(true)
    .git_ignore(true)
    .build();
```

The existing `guard.cap_files()` call at line 237-238 already handles capping, so even a large project won't return unbounded results.

**Step 4: Run test to verify it passes**

Run: `cargo test get_symbols_overview_project_wide_finds_nested_files -- --nocapture`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 6: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "fix: get_symbols_overview walks full project tree when path is root"
```

---

## Task 4: Bug #4 — `git_blame` error message leaks wrong line number (P2, Low effort)

**Problem:** When a file has uncommitted changes, `blame_file()` blames the committed version but reads the working-directory version from disk. If the file has more lines on disk than in the last commit, `blame.get_line(i+1)` returns `None` for the extra lines, producing `"No blame hunk for line {i+1}"` — which is an internal 0-indexed number, not the user-requested range.

**Files:**
- Modify: `src/git/blame.rs:18-39`
- Test: `src/git/blame.rs` (add `#[cfg(test)]` module)

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn init_repo_with_file(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, content).unwrap();

        // Initialize git repo and make initial commit
        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[]).unwrap();

        (dir, file)
    }

    #[test]
    fn blame_committed_file_works() {
        let (dir, _file) = init_repo_with_file("line 1\nline 2\nline 3\n");
        let result = blame_file(dir.path(), Path::new("test.txt")).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].line, 1);
        assert_eq!(result[0].content, "line 1");
    }

    #[test]
    fn blame_file_with_uncommitted_additions_returns_helpful_error() {
        let (dir, file) = init_repo_with_file("line 1\nline 2\n");
        // Add more lines without committing
        std::fs::write(&file, "line 1\nline 2\nnew line 3\nnew line 4\n").unwrap();

        let result = blame_file(dir.path(), Path::new("test.txt"));
        // Should either succeed (blaming committed lines) or return a helpful error
        // mentioning uncommitted changes — not a raw "No blame hunk for line N"
        match result {
            Ok(lines) => {
                // If it succeeds, should have reasonable content
                assert!(!lines.is_empty());
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("uncommitted") || msg.contains("modified"),
                    "Error should mention uncommitted changes, got: {}",
                    msg
                );
            }
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test blame_file_with_uncommitted -- --nocapture`
Expected: FAIL — error message says "No blame hunk for line 3" with no mention of uncommitted changes

**Step 3: Implement the fix**

Two changes in `src/git/blame.rs`:

1. **Read the committed version instead of working directory** — this avoids the line-count mismatch entirely:

```rust
pub fn blame_file(repo_path: &Path, file: &Path) -> Result<Vec<BlameLine>> {
    let repo = open_repo(repo_path)?;
    let blame = repo.blame_file(file, None)?;

    // Read the COMMITTED version of the file (not the working directory version)
    // to avoid line-count mismatch when the file has uncommitted changes.
    let source = match committed_content(&repo, file) {
        Ok(content) => content,
        Err(_) => {
            // Fallback to disk if file is not yet committed (new file)
            std::fs::read_to_string(repo.workdir().unwrap_or(repo_path).join(file))?
        }
    };

    let mut result = vec![];
    for (i, line_text) in source.lines().enumerate() {
        let hunk = blame.get_line(i + 1).ok_or_else(|| {
            anyhow::anyhow!(
                "{} has uncommitted changes. git blame only covers committed content. \
                 Use git_diff to see uncommitted changes.",
                file.display()
            )
        })?;

        let sig = hunk.orig_signature();
        result.push(BlameLine {
            line: i + 1,
            content: line_text.to_string(),
            sha: format!("{:.8}", hunk.orig_commit_id()),
            author: sig.name().unwrap_or("unknown").to_string(),
            timestamp: sig.when().seconds(),
        });
    }
    Ok(result)
}

/// Read the HEAD version of a file from the git object store.
fn committed_content(repo: &git2::Repository, file: &Path) -> Result<String> {
    let head = repo.head()?.peel_to_tree()?;
    let entry = head.get_path(file)?;
    let blob = repo.find_blob(entry.id())?;
    Ok(std::str::from_utf8(blob.content())?.to_string())
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test blame_file_with_uncommitted -- --nocapture`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 6: Commit**

```bash
git add src/git/blame.rs
git commit -m "fix: git_blame reads committed content, better error for dirty files"
```

---

## Task 5: Bug #5 — Semantic search returns irrelevant results (P1, Medium effort)

**Problem:** Two issues — (a) default `chunk_size: 4000` is too coarse, producing averaged embeddings that match many queries superficially; (b) no markdown-aware chunking, so a research doc about "serena vs intellij" gets chunked by character count, diluting its signal.

**Fix approach:**
1. Reduce default `chunk_size` to 1200 and `chunk_overlap` to 200
2. Add markdown section-based chunking: split on `##`/`###` headings before applying character limits

**Files:**
- Modify: `src/config/project.rs:64-69` (default values)
- Modify: `src/embed/chunker.rs` (add markdown-aware splitting)
- Test: `src/embed/chunker.rs` (in-file `#[cfg(test)]` module)

**Step 1: Write the failing test for markdown chunking**

```rust
#[test]
fn markdown_chunks_split_on_headings() {
    let source = "\
# Title

Intro paragraph with some content.

## Section One

Content for section one that is meaningful.

## Section Two

Content for section two that is different.

### Subsection

More specific content here.
";
    // With a generous chunk_size, each heading section should still get its own chunk
    let chunks = split_markdown(source, 500, 50);
    assert!(
        chunks.len() >= 3,
        "Should split on heading boundaries, got {} chunks",
        chunks.len()
    );
    // First chunk should contain "Title" and "Intro"
    assert!(chunks[0].content.contains("Title"));
    // There should be a chunk starting with "## Section One" or containing it
    assert!(
        chunks.iter().any(|c| c.content.contains("Section One")),
        "Section One should be in a chunk"
    );
    assert!(
        chunks.iter().any(|c| c.content.contains("Section Two")),
        "Section Two should be in a chunk"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test markdown_chunks_split_on_headings -- --nocapture`
Expected: FAIL — `split_markdown` doesn't exist yet

**Step 3: Implement markdown-aware chunking**

Add a `split_markdown()` function to `src/embed/chunker.rs`:

```rust
/// Split markdown content by heading boundaries, then apply character limits.
///
/// Each `##` or `###` heading starts a new section. Sections that exceed
/// `chunk_size` are further split by the regular line-based `split()`.
pub fn split_markdown(source: &str, chunk_size: usize, chunk_overlap: usize) -> Vec<RawChunk> {
    if source.is_empty() {
        return vec![];
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut sections: Vec<(usize, usize)> = vec![]; // (start_idx, end_idx) 0-indexed
    let mut section_start = 0;

    for (i, line) in lines.iter().enumerate() {
        // Split on ## and ### headings (but not # which is the title)
        if i > 0 && (line.starts_with("## ") || line.starts_with("### ")) {
            sections.push((section_start, i));
            section_start = i;
        }
    }
    sections.push((section_start, lines.len()));

    let mut chunks = vec![];
    for (start, end) in sections {
        let section_text = lines[start..end].join("\n");
        if section_text.len() <= chunk_size {
            chunks.push(RawChunk {
                content: section_text,
                start_line: start + 1,
                end_line: end,
            });
        } else {
            // Section too large — fall back to character-based splitting
            let sub_chunks = split(&section_text, chunk_size, chunk_overlap);
            for mut sc in sub_chunks {
                // Adjust line numbers to be relative to the whole file
                sc.start_line += start;
                sc.end_line += start;
                chunks.push(sc);
            }
        }
    }
    chunks
}
```

**Step 4: Wire markdown chunking into the indexing pipeline**

In `src/embed/index.rs`, in the `build_index` function where `chunker::split()` is called, detect markdown files and use `split_markdown` instead:

```rust
// In build_index, where chunks are created (around the chunker::split call):
let chunks = if language == "markdown" {
    crate::embed::chunker::split_markdown(&content, config.embeddings.chunk_size, config.embeddings.chunk_overlap)
} else {
    crate::embed::chunker::split(&content, config.embeddings.chunk_size, config.embeddings.chunk_overlap)
};
```

Find the exact location by searching for `chunker::split` in `src/embed/index.rs`.

**Step 5: Update default chunk_size and chunk_overlap**

In `src/config/project.rs`, change the defaults:

```rust
fn default_chunk_size() -> usize {
    1200  // was 4000 — smaller chunks = higher semantic precision
}
fn default_chunk_overlap() -> usize {
    200   // was 400 — proportional to new chunk_size
}
```

**Step 6: Update existing chunker tests for new defaults**

Check if any existing tests hardcode `4000`/`400` values. If they pass explicit values to `split()`, they're fine. If they rely on default config values, update them.

**Step 7: Run all tests**

Run: `cargo test`
Expected: All tests pass (new + existing)

**Step 8: Commit**

```bash
git add src/embed/chunker.rs src/embed/index.rs src/config/project.rs
git commit -m "fix: improve semantic search with smaller chunks and markdown-aware splitting"
```

---

## Task 6: Quality — `semantic_search` token cost (P2, Low effort)

**Problem:** `semantic_search` returns full chunk content (~4000 chars each) for every result. This is the only tool that ignores the progressive disclosure pattern. Each search can cost ~5000 tokens.

**Fix approach:** Apply `OutputGuard` to `semantic_search`. In exploring mode, return a preview (first 150 chars + `...`). In focused mode, return full content with pagination.

**Files:**
- Modify: `src/tools/semantic.rs:19-63` (add OutputGuard params to schema + call)
- Test: `src/tools/semantic.rs` (in-file `#[cfg(test)]` module)

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn semantic_search_exploring_mode_returns_preview() {
    let (dir, ctx) = project_ctx().await;

    // Insert a chunk with long content
    let conn = crate::embed::index::open_db(dir.path()).unwrap();
    let long_content = "x".repeat(500);
    let chunk = crate::embed::schema::CodeChunk {
        id: None,
        file_path: "test.rs".to_string(),
        language: "rust".to_string(),
        content: long_content.clone(),
        start_line: 1,
        end_line: 10,
        file_hash: "abc".to_string(),
    };
    crate::embed::index::insert_chunk(&conn, &chunk, &[0.1, 0.2, 0.3]).unwrap();
    crate::embed::index::upsert_file_hash(&conn, "test.rs", "abc").unwrap();
    drop(conn);

    // In exploring mode (default), content should be a preview, not full
    // We can't easily test search quality without real embeddings, but we can
    // test the output format by checking the schema has detail_level
    let schema = SemanticSearch.input_schema();
    let props = schema["properties"].as_object().unwrap();
    assert!(
        props.contains_key("detail_level"),
        "semantic_search should accept detail_level parameter"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test semantic_search_exploring_mode_returns_preview -- --nocapture`
Expected: FAIL — `detail_level` not in input schema

**Step 3: Implement OutputGuard on semantic_search**

In `src/tools/semantic.rs`:

1. Add `detail_level`, `offset`, `limit` to the input schema (lines 19-30)
2. In `call()`, create `OutputGuard::from_input(&input)`
3. In exploring mode, truncate each result's `content` to a preview
4. In focused mode, return full content with pagination

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "description": "Natural language description or code snippet to search for"
            },
            "limit": { "type": "integer", "default": 10 },
            "detail_level": { "type": "string", "description": "Output detail: omit for compact preview (default), 'full' for complete chunk content" },
            "offset": { "type": "integer", "description": "Skip this many results (focused mode pagination)" },
            "limit": { "type": "integer", "description": "Max results per page (focused mode, default 50)" }
        }
    })
}

async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
    use super::output::OutputGuard;

    let query = input["query"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'query' parameter"))?;
    let limit = input["limit"].as_u64().unwrap_or(10) as usize;
    let guard = OutputGuard::from_input(&input);

    // ... existing embed + search logic ...

    let results: Vec<Value> = results.iter().map(|r| {
        let content_field = if guard.should_include_body() {
            // Focused mode: full content
            r.content.clone()
        } else {
            // Exploring mode: preview (first 150 chars)
            let preview_len = 150.min(r.content.len());
            let mut preview = r.content[..preview_len].to_string();
            if r.content.len() > preview_len {
                preview.push_str("...");
            }
            preview
        };

        json!({
            "file_path": r.file_path,
            "language": r.language,
            "content": content_field,
            "start_line": r.start_line,
            "end_line": r.end_line,
            "score": r.score,
        })
    }).collect();

    let (results, overflow) = guard.cap_items(results, "Use detail_level='full' with offset/limit for pagination");

    let mut result = json!({
        "results": results,
        "total": overflow.as_ref().map_or(results.len(), |o| o.total),
    });
    if let Some(ov) = overflow {
        result["overflow"] = OutputGuard::overflow_json(&ov);
    }
    Ok(result)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test semantic_search_exploring_mode_returns_preview -- --nocapture`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 6: Commit**

```bash
git add src/tools/semantic.rs
git commit -m "feat: apply OutputGuard to semantic_search for token-efficient output"
```

---

## Task 7: Final verification

**Step 1: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 2: Run fmt**

Run: `cargo fmt`
Expected: No changes (already formatted)

**Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass (173 + new tests)

**Step 4: Update test count in CLAUDE.md**

If new tests were added, update `CLAUDE.md` line that says `173 passing` to the new count.

**Step 5: Final commit if needed**

```bash
git add CLAUDE.md
git commit -m "chore: update test count after benchmark bugfixes"
```
