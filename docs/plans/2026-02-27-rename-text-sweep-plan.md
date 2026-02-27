# Rename Text Sweep Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance `rename_symbol` to report remaining textual occurrences (comments, docs, strings) that LSP rename missed.

**Architecture:** After the existing LSP rename phase, run a word-boundary regex sweep across the project root using `ignore::WalkBuilder`. Collect matches grouped by file, classified by type (documentation/config/source), capped at 20 entries with max 2 preview lines per file. Report in the response JSON — no auto-replace.

**Tech Stack:** Rust, `regex` crate (already a dependency), `ignore` crate (already a dependency)

---

### Task 1: Add `TextualMatch` struct and `text_sweep` function (tests first)

**Files:**
- Modify: `src/tools/symbol.rs` (add struct + function before `RenameSymbol` struct, ~line 1032)
- Test: `src/tools/symbol.rs` (existing `#[cfg(test)] mod tests` at line 1320)

**Step 1: Write failing tests for `text_sweep`**

Add these tests at the end of the existing `mod tests` block (before the closing `}`):

```rust
#[test]
fn text_sweep_finds_matches_in_comments_and_docs() {
    let dir = tempfile::tempdir().unwrap();

    // Source file with a comment mentioning the old name
    std::fs::write(
        dir.path().join("main.rs"),
        "fn bar() {}\n// FooHandler manages connections\n",
    )
    .unwrap();

    // Documentation file
    std::fs::write(
        dir.path().join("README.md"),
        "# Project\nThe FooHandler struct is the entry point.\nSee FooHandler::new() for details.\n",
    )
    .unwrap();

    // Config file
    std::fs::write(
        dir.path().join("config.toml"),
        "[server]\nhandler = \"FooHandler\"\n",
    )
    .unwrap();

    let lsp_files = std::collections::HashSet::new();
    let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

    // Should find matches in all 3 files
    assert_eq!(matches.len(), 3);

    // Documentation first, then config, then source
    assert_eq!(matches[0].kind, "documentation");
    assert_eq!(matches[1].kind, "config");
    assert_eq!(matches[2].kind, "source");

    // README has 2 occurrences, both shown as previews
    assert_eq!(matches[0].occurrence_count, 2);
    assert_eq!(matches[0].previews.len(), 2);

    // Config has 1 occurrence
    assert_eq!(matches[1].occurrence_count, 1);

    // Source has 1 occurrence (comment line)
    assert_eq!(matches[2].occurrence_count, 1);
}

#[test]
fn text_sweep_skips_lsp_modified_files() {
    let dir = tempfile::tempdir().unwrap();

    let modified_file = dir.path().join("already.rs");
    std::fs::write(&modified_file, "// FooHandler was here\n").unwrap();
    std::fs::write(
        dir.path().join("untouched.md"),
        "FooHandler docs\n",
    )
    .unwrap();

    let mut lsp_files = std::collections::HashSet::new();
    lsp_files.insert(modified_file);

    let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

    assert_eq!(matches.len(), 1);
    assert!(matches[0].file.contains("untouched.md"));
}

#[test]
fn text_sweep_respects_max_matches_cap() {
    let dir = tempfile::tempdir().unwrap();

    // Create 30 markdown files, each with one match
    for i in 0..30 {
        std::fs::write(
            dir.path().join(format!("doc{i:02}.md")),
            format!("FooHandler reference in doc {i}\n"),
        )
        .unwrap();
    }

    let lsp_files = std::collections::HashSet::new();
    let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

    assert_eq!(matches.len(), 20);
}

#[test]
fn text_sweep_limits_previews_per_file() {
    let dir = tempfile::tempdir().unwrap();

    // File with 10 occurrences
    let content = (0..10)
        .map(|i| format!("line {i}: FooHandler usage"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.path().join("many.rs"), &content).unwrap();

    let lsp_files = std::collections::HashSet::new();
    let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].occurrence_count, 10);
    assert_eq!(matches[0].previews.len(), 2); // capped at 2
    assert_eq!(matches[0].lines.len(), 10);   // all line numbers kept
}

#[test]
fn text_sweep_uses_word_boundary() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("test.rs"),
        "let foo_handler = 1;\n// FooHandler docs\nlet FooHandlerConfig = 2;\n",
    )
    .unwrap();

    let lsp_files = std::collections::HashSet::new();
    let matches = text_sweep(dir.path(), "FooHandler", &lsp_files, 20, 2).unwrap();

    assert_eq!(matches.len(), 1);
    // Should match "FooHandler" on its own and "FooHandlerConfig" (word boundary
    // matches at start of FooHandlerConfig since \b is between start-of-word and F)
    // Actually: \bFooHandler\b does NOT match inside FooHandlerConfig because
    // there's no word boundary between 'r' and 'C' (both are word chars).
    // So only 1 match: the comment line.
    assert_eq!(matches[0].occurrence_count, 1);
    assert!(matches[0].previews[0].contains("// FooHandler docs"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test text_sweep -- --nocapture 2>&1 | head -30`
Expected: FAIL — `text_sweep` function not found

**Step 3: Implement `TextualMatch` struct and `text_sweep` function**

Add before the `RenameSymbol` struct definition (~line 1032), after `InsertCode` impl:

```rust
use std::collections::HashSet;

/// A textual match found during post-rename sweep.
#[derive(Debug)]
struct TextualMatch {
    /// Relative path from project root
    file: String,
    /// All matching line numbers (1-indexed)
    lines: Vec<u32>,
    /// First N matching line contents (trimmed)
    previews: Vec<String>,
    /// Total occurrences in this file
    occurrence_count: usize,
    /// "documentation" | "config" | "source"
    kind: &'static str,
}

/// Classify a file by extension for result prioritization.
fn classify_file(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
    {
        "md" | "txt" | "rst" | "adoc" => "documentation",
        "toml" | "yaml" | "yml" | "json" => "config",
        _ => "source",
    }
}

/// Sort key for file classification (lower = higher priority).
fn classify_sort_key(kind: &str) -> u8 {
    match kind {
        "documentation" => 0,
        "config" => 1,
        _ => 2,
    }
}

/// Post-rename text sweep: finds remaining textual occurrences of `old_name`
/// that the LSP rename didn't touch.
fn text_sweep(
    project_root: &Path,
    old_name: &str,
    lsp_modified_files: &HashSet<PathBuf>,
    max_matches: usize,
    max_previews_per_file: usize,
) -> anyhow::Result<Vec<TextualMatch>> {
    let escaped = regex::escape(old_name);
    let pattern = format!(r"\b{escaped}\b");
    let re = regex::RegexBuilder::new(&pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()?;

    let mut file_matches: Vec<TextualMatch> = Vec::new();

    let walker = ignore::WalkBuilder::new(project_root)
        .hidden(true)
        .git_ignore(true)
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();

        // Skip files already modified by LSP rename
        if lsp_modified_files.contains(path) {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(path) else {
            continue; // skip binary / non-UTF8
        };

        let mut lines = Vec::new();
        let mut previews = Vec::new();

        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                lines.push((i + 1) as u32);
                if previews.len() < max_previews_per_file {
                    previews.push(line.trim().to_string());
                }
            }
        }

        if !lines.is_empty() {
            let rel_path = path
                .strip_prefix(project_root)
                .unwrap_or(path)
                .display()
                .to_string();
            let kind = classify_file(path);

            file_matches.push(TextualMatch {
                file: rel_path,
                lines,
                previews,
                occurrence_count: 0, // set below
                kind,
            });
            // Set occurrence_count from lines length
            let last = file_matches.last_mut().unwrap();
            last.occurrence_count = last.lines.len();
        }
    }

    // Sort: documentation first, config second, source third
    file_matches.sort_by_key(|m| classify_sort_key(m.kind));

    // Cap total entries
    file_matches.truncate(max_matches);

    Ok(file_matches)
}
```

Note: `std::collections::HashSet` and `std::path::PathBuf` are likely already imported at the top of `symbol.rs`. Check existing imports and add only what's missing. `regex` and `ignore` are already in `Cargo.toml` as dependencies.

**Step 4: Run tests to verify they pass**

Run: `cargo test text_sweep -- --nocapture`
Expected: all 5 tests PASS

**Step 5: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(rename): add text_sweep helper for post-rename textual match detection"
```

---

### Task 2: Integrate `text_sweep` into `RenameSymbol::call`

**Files:**
- Modify: `src/tools/symbol.rs` — `RenameSymbol::call` method (~line 1061-1177)
- Modify: `src/tools/symbol.rs` — `RenameSymbol::description` method (~line 1047)

**Step 1: Collect LSP-modified files**

In `RenameSymbol::call`, the existing code iterates `edit.changes` and `edit.document_changes` to apply edits. We need to collect the file paths into a `HashSet<PathBuf>` as we go.

Add `let mut lsp_files: HashSet<PathBuf> = HashSet::new();` right after `let mut total_edits = 0;` (~line 1088).

Then in each branch where a file path is resolved and written, add `lsp_files.insert(path.clone());` right after the `std::fs::write` call. There are 3 places:
1. Inside the `edit.changes` loop
2. Inside the `DocumentChanges::Edits` branch
3. Inside the `DocumentChangeOperation::Edit` branch

**Step 2: Add Phase 2 sweep after the LSP apply section**

After the existing response-building code (before the final `Ok(json!({...}))`), add:

```rust
// Phase 2: text sweep for remaining textual occurrences
let old_name_str = name_path.rsplit('/').next().unwrap_or(name_path);
let (textual, sweep_skipped, sweep_skip_reason) = if old_name_str.len() < 4 {
    (
        vec![],
        true,
        Some(format!(
            "name too short ({} chars, minimum 4)",
            old_name_str.len()
        )),
    )
} else {
    match text_sweep(&rename_root, old_name_str, &lsp_files, 20, 2) {
        Ok(matches) => (matches, false, None::<String>),
        Err(e) => {
            tracing::warn!("text sweep after rename failed: {e}");
            (vec![], false, Some(format!("sweep error: {e}")))
        }
    }
};

let textual_total: usize = textual.iter().map(|m| m.occurrence_count).sum();
let textual_shown = textual.len();
let textual_json: Vec<Value> = textual
    .into_iter()
    .map(|m| {
        json!({
            "file": m.file,
            "lines": m.lines,
            "previews": m.previews,
            "occurrence_count": m.occurrence_count,
            "kind": m.kind,
        })
    })
    .collect();
```

**Step 3: Update the response JSON**

Replace the existing `Ok(json!({...}))` at the end of the method with:

```rust
let mut result = json!({
    "status": "ok",
    "old_name": old_name_str,
    "new_name": new_name,
    "files_changed": files_changed,
    "total_edits": total_edits,
    "textual_matches": textual_json,
    "textual_match_count": textual_total,
    "textual_matches_shown": textual_shown,
    "sweep_skipped": sweep_skipped,
});
if let Some(reason) = sweep_skip_reason {
    result["sweep_skip_reason"] = json!(reason);
}
Ok(result)
```

**Step 4: Update tool description**

In the `description()` method (~line 1047), change:

```rust
"Rename a symbol across the entire codebase using LSP."
```

To:

```rust
"Rename a symbol across the entire codebase using LSP. After renaming, sweeps for remaining textual occurrences (comments, docs, strings) that LSP missed and reports them."
```

**Step 5: Verify compilation**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles with no errors (warnings ok)

**Step 6: Run all existing tests**

Run: `cargo test 2>&1 | tail -10`
Expected: all tests pass (the rename tests are e2e, so existing unit tests shouldn't be affected)

**Step 7: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat(rename): integrate text sweep into rename_symbol response"
```

---

### Task 3: Run full validation

**Step 1: Format**

Run: `cargo fmt`

**Step 2: Lint**

Run: `cargo clippy -- -D warnings 2>&1 | tail -20`
Expected: no warnings

**Step 3: Full test suite**

Run: `cargo test 2>&1 | tail -10`
Expected: all tests pass

**Step 4: Fix any issues found**

Address clippy warnings or test failures. Common things to watch for:
- Unused imports (if `HashSet`/`PathBuf` were already imported differently)
- The `tracing::warn!` macro may need `tracing` to be in scope (check existing usage in the file)

**Step 5: Final commit if needed**

```bash
git add -u
git commit -m "chore: fix clippy/fmt after rename text sweep"
```
