# Physical Position Access Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the "Bash python3 file-read bypass" gap by making `read_file` accept source files when a line range is given, and adding `context_lines` to `search_pattern`.

**Architecture:** Two surgical edits to `src/tools/file.rs`. No new files, no new structs. `ReadFile::call` reorders its source-file guard after extracting line-range params. `SearchPattern::call` collects file lines into a Vec, merges overlapping context windows, and emits compact block objects when `context_lines > 0`.

**Tech Stack:** Rust, serde_json (`json!` macro), `regex` crate, `ignore` crate (WalkBuilder). Tests use `tempfile::tempdir()` and `tokio::test`.

---

## Task 1: `read_file` — failing tests first

**Files:**
- Modify: `src/tools/file.rs` (tests module, starting after line 1870)

### Step 1: Add two failing tests inside `mod tests`

Append to the `mod tests` block in `src/tools/file.rs` (after the last test):

```rust
#[tokio::test]
async fn read_file_source_with_range_allowed() {
    let (dir, ctx) = project_ctx().await;
    let rs_file = dir.path().join("lib.rs");
    std::fs::write(&rs_file, "line1\nline2\nline3\nline4\nline5\n").unwrap();

    let result = ReadFile
        .call(
            json!({
                "path": rs_file.to_str().unwrap(),
                "start_line": 2,
                "end_line": 4
            }),
            &ctx,
        )
        .await
        .unwrap();

    let content = result["content"].as_str().unwrap();
    assert!(content.contains("line2"), "should include line2: {content}");
    assert!(content.contains("line4"), "should include line4: {content}");
    assert!(!content.contains("line5"), "should not include line5: {content}");
}

#[tokio::test]
async fn read_file_source_without_range_still_blocked() {
    let (dir, ctx) = project_ctx().await;
    let rs_file = dir.path().join("lib.rs");
    std::fs::write(&rs_file, "fn main() {}\n").unwrap();

    let result = ReadFile
        .call(json!({ "path": rs_file.to_str().unwrap() }), &ctx)
        .await;

    assert!(result.is_err(), "should still block source without range");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("start_line") || err.contains("line range"),
        "error hint should mention range option: {err}"
    );
}
```

### Step 2: Run tests to confirm they fail

```bash
cargo test read_file_source 2>&1 | tail -20
```

Expected: `read_file_source_with_range_allowed` FAILS with `"read_file is not available for source code files"`.
`read_file_source_without_range_still_blocked` FAILS because the hint doesn't mention `start_line` yet.

### Step 3: Implement — reorder source-file guard in `ReadFile::call`

**Modify:** `src/tools/file.rs`, `impl Tool for ReadFile/call` (lines 37–121).

Replace the current `call` body with:

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
    use super::output::{OutputGuard, OutputMode, OverflowInfo};

    let path = input["path"]
        .as_str()
        .or_else(|| input["file_path"].as_str())
        .ok_or_else(|| {
            RecoverableError::with_hint(
                "missing required parameter 'path'",
                "Provide the file path as: path=\"relative/path/to/file\"",
            )
        })?;
    let project_root = ctx.agent.project_root().await;
    let security = ctx.agent.security_config().await;
    let resolved = crate::util::path_security::validate_read_path(
        path,
        project_root.as_deref(),
        &security,
    )?;

    // Extract line range early — a targeted read unlocks source code files
    let start_line = input["start_line"].as_u64();
    let end_line = input["end_line"].as_u64();
    let has_range = start_line.is_some() && end_line.is_some();

    // Block source code files — unless a targeted line range is provided
    if !has_range {
        if let Some(lang) = crate::ast::detect_language(&resolved) {
            if lang != "markdown" {
                return Err(RecoverableError::with_hint(
                    "read_file is not available for source code files",
                    "Specify start_line + end_line for a targeted read, or use symbol tools:\n  \
                     list_symbols(path) — see all symbols + line numbers\n  \
                     find_symbol(name, include_body=true) — read a specific symbol body\n  \
                     list_functions(path) — quick function signatures\n  \
                     search_pattern(\".\", path) — read raw lines (e.g. imports); \
                     then edit_lines(path, line, count, text) to modify",
                )
                .into());
            }
        }
    }

    // Determine source tag
    let source_tag = {
        let inner = ctx.agent.inner.read().await;
        if let Some(project) = &inner.active_project {
            if let Some(lib) = project.library_registry.is_library_path(&resolved) {
                format!("lib:{}", lib.name)
            } else {
                "project".to_string()
            }
        } else {
            "project".to_string()
        }
    };

    let text = std::fs::read_to_string(&resolved)?;

    // If explicit line range given, use it directly (no capping)
    if let (Some(start), Some(end)) = (start_line, end_line) {
        let content = extract_lines(&text, start as usize, end as usize);
        return Ok(json!({ "content": content, "source": source_tag }));
    }

    // No line range: cap in exploring mode
    let guard = OutputGuard::from_input(&input);
    let total_lines = text.lines().count();
    let max_lines = guard.max_results; // 200 by default

    if guard.mode == OutputMode::Exploring && total_lines > max_lines {
        let content = extract_lines(&text, 1, max_lines);
        let overflow = OverflowInfo {
            shown: max_lines,
            total: total_lines,
            hint: format!(
                "File has {} lines. Use start_line/end_line to read specific ranges",
                total_lines
            ),
            next_offset: None,
            by_file: None,
            by_file_overflow: 0,
        };
        let mut result =
            json!({ "content": content, "total_lines": total_lines, "source": source_tag });
        result["overflow"] = OutputGuard::overflow_json(&overflow);
        Ok(result)
    } else {
        Ok(json!({ "content": text, "total_lines": total_lines, "source": source_tag }))
    }
}
```

Also update `description`:

```rust
fn description(&self) -> &str {
    "Read the contents of a file. Optionally restrict to a line range. \
     Source code files (.rs, .py, .ts, etc.) require start_line + end_line — \
     use symbol tools for whole-file reads."
}
```

### Step 4: Run both tests

```bash
cargo test read_file_source 2>&1 | tail -20
```

Expected: both PASS.

### Step 5: Run the full test suite to catch regressions

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass (533+).

### Step 6: Commit

```bash
git add src/tools/file.rs
git commit -m "feat(read_file): allow targeted source reads when start_line + end_line given"
```

---

## Task 2: `search_pattern` context_lines — failing tests first

**Files:**
- Modify: `src/tools/file.rs` (tests module, schema, call body)

### Step 1: Add four failing tests

Append to `mod tests` in `src/tools/file.rs`:

```rust
#[tokio::test]
async fn search_pattern_context_lines_zero_backward_compat() {
    // context_lines absent or 0 → old format (line + content keys)
    let ctx = test_ctx().await;
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("code.rs"), "fn main() {}\nlet x = 42;\n").unwrap();

    let result = SearchPattern
        .call(
            json!({ "pattern": "fn main", "path": dir.path().to_str().unwrap() }),
            &ctx,
        )
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    // Old format preserved
    assert_eq!(matches[0]["line"], 1);
    assert!(matches[0]["content"].as_str().unwrap().contains("fn main"));
}

#[tokio::test]
async fn search_pattern_context_lines_single_match() {
    let ctx = test_ctx().await;
    let dir = tempdir().unwrap();
    // TARGET is line 3; context=2 → block covers lines 1-5
    std::fs::write(
        dir.path().join("code.rs"),
        "line1\nline2\nTARGET\nline4\nline5\n",
    )
    .unwrap();

    let result = SearchPattern
        .call(
            json!({
                "pattern": "TARGET",
                "path": dir.path().to_str().unwrap(),
                "context_lines": 2
            }),
            &ctx,
        )
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["match_line"], 3, "match_line should be 1-indexed line of TARGET");
    assert_eq!(matches[0]["start_line"], 1, "start_line = match(3) - context(2) = 1");
    let content = matches[0]["content"].as_str().unwrap();
    assert!(content.contains("line1"), "context_before should include line1");
    assert!(content.contains("TARGET"), "content should include match");
    assert!(content.contains("line5"), "context_after should include line5");
}

#[tokio::test]
async fn search_pattern_context_lines_adjacent_matches_merge() {
    let ctx = test_ctx().await;
    let dir = tempdir().unwrap();
    // MATCH_A at line 3, MATCH_B at line 5; context=2 → windows overlap → one block
    std::fs::write(
        dir.path().join("code.rs"),
        "line1\nline2\nMATCH_A\nline4\nMATCH_B\nline6\nline7\n",
    )
    .unwrap();

    let result = SearchPattern
        .call(
            json!({
                "pattern": "MATCH_",
                "path": dir.path().to_str().unwrap(),
                "context_lines": 2
            }),
            &ctx,
        )
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1, "overlapping context windows should merge into one block");
    let content = matches[0]["content"].as_str().unwrap();
    assert!(content.contains("MATCH_A"), "merged block should contain first match");
    assert!(content.contains("MATCH_B"), "merged block should contain second match");
    assert!(content.contains("line7"), "block should extend to MATCH_B's context_after");
}

#[tokio::test]
async fn search_pattern_context_lines_non_adjacent_matches_separate() {
    let ctx = test_ctx().await;
    let dir = tempdir().unwrap();
    // MATCH at line 2 and line 18; with context=2 the windows don't overlap → two blocks
    let file_content = (1..=20)
        .map(|i| {
            if i == 2 || i == 18 {
                format!("MATCH line{i}")
            } else {
                format!("other line{i}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(dir.path().join("code.rs"), file_content).unwrap();

    let result = SearchPattern
        .call(
            json!({
                "pattern": "MATCH",
                "path": dir.path().to_str().unwrap(),
                "context_lines": 2
            }),
            &ctx,
        )
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2, "non-overlapping windows should produce two separate blocks");
    assert_eq!(matches[0]["match_line"], 2);
    assert_eq!(matches[1]["match_line"], 18);
}
```

### Step 2: Run to confirm they fail

```bash
cargo test search_pattern_context 2>&1 | tail -20
```

Expected: all four FAIL — `search_pattern_context_lines_zero_backward_compat` should PASS already (no new param needed for that path); the other three fail with `"unknown field context_lines"` or similar.

### Step 3: Add `context_lines` to schema

In `impl Tool for SearchPattern`, update `input_schema`:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["pattern"],
        "properties": {
            "pattern": { "type": "string", "description": "Regex pattern" },
            "path": { "type": "string", "description": "File or directory to search (default: project root)" },
            "max_results": { "type": "integer", "default": 50, "description": "Maximum matches to return. Alias: limit" },
            "limit": { "type": "integer", "description": "Alias for max_results" },
            "context_lines": {
                "type": "integer",
                "default": 0,
                "description": "Lines of context before and after each match (max 20). Adjacent matches that share context are merged into one block with a flat multiline content string."
            }
        }
    })
}
```

Update `description`:

```rust
fn description(&self) -> &str {
    "Search the codebase for a regex pattern. Returns matching lines with file and line number. \
     Pass context_lines to see surrounding code — adjacent matches that share context windows \
     are merged into one block."
}
```

### Step 4: Implement `context_lines` logic in `SearchPattern::call`

Replace the entire `call` body with:

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
    let pattern = super::require_str_param(&input, "pattern")?;
    let raw_path = input["path"].as_str().unwrap_or(".");
    let project_root = ctx.agent.project_root().await;
    let security = ctx.agent.security_config().await;
    let search_path = crate::util::path_security::validate_read_path(
        raw_path,
        project_root.as_deref(),
        &security,
    )?;
    let max = input["max_results"]
        .as_u64()
        .or_else(|| input["limit"].as_u64())
        .unwrap_or(50) as usize;
    let context_lines = input["context_lines"]
        .as_u64()
        .unwrap_or(0)
        .min(20) as usize;
    let re = regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| {
            RecoverableError::with_hint(
                format!("invalid regex: {e}"),
                "patterns are full regex syntax — escape metacharacters like \\( \\. \\[ for literals",
            )
        })?;
    let mut matches = vec![];

    let walker = ignore::WalkBuilder::new(&search_path)
        .hidden(true)
        .git_ignore(true)
        .build();
    'outer: for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };

        if context_lines == 0 {
            // Original behavior: emit one entry per matching line
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    matches.push(json!({
                        "file": entry.path().display().to_string(),
                        "line": i + 1,
                        "content": line
                    }));
                    if matches.len() >= max {
                        break 'outer;
                    }
                }
            }
        } else {
            // Context mode: collect lines, merge overlapping windows into blocks
            let file_lines: Vec<&str> = text.lines().collect();
            let n = file_lines.len();
            // Each element: (block_start_idx, first_match_idx, block_end_idx)
            let mut current: Option<(usize, usize, usize)> = None;
            let mut match_count = 0usize;

            for (i, line) in file_lines.iter().enumerate() {
                if !re.is_match(line) {
                    continue;
                }
                match_count += 1;
                let ctx_start = i.saturating_sub(context_lines);
                let ctx_end = (i + context_lines).min(n.saturating_sub(1));

                match current {
                    None => {
                        current = Some((ctx_start, i, ctx_end));
                    }
                    Some((blk_start, blk_first, blk_end)) => {
                        if ctx_start <= blk_end + 1 {
                            // Overlapping or adjacent: extend the current block
                            current = Some((blk_start, blk_first, ctx_end.max(blk_end)));
                        } else {
                            // Non-overlapping: emit the finished block, start a new one
                            let content = file_lines[blk_start..=blk_end].join("\n");
                            matches.push(json!({
                                "file": entry.path().display().to_string(),
                                "match_line": blk_first + 1,
                                "start_line": blk_start + 1,
                                "content": content,
                            }));
                            current = Some((ctx_start, i, ctx_end));
                        }
                    }
                }

                if match_count >= max {
                    break;
                }
            }

            // Emit the last in-flight block
            if let Some((blk_start, blk_first, blk_end)) = current {
                let content = file_lines[blk_start..=blk_end].join("\n");
                matches.push(json!({
                    "file": entry.path().display().to_string(),
                    "match_line": blk_first + 1,
                    "start_line": blk_start + 1,
                    "content": content,
                }));
            }

            if match_count >= max {
                break 'outer;
            }
        }
    }

    Ok(json!({ "matches": matches, "total": matches.len() }))
}
```

### Step 5: Run the four new tests

```bash
cargo test search_pattern_context 2>&1 | tail -20
```

Expected: all four PASS.

### Step 6: Run the full test suite

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass.

### Step 7: Clippy and fmt

```bash
cargo clippy -- -D warnings 2>&1 | tail -20
cargo fmt
```

Expected: no warnings, no diff.

### Step 8: Commit

```bash
git add src/tools/file.rs
git commit -m "feat(search_pattern): add context_lines for merged context blocks around matches"
```

---

## Final verification

```bash
cargo test 2>&1 | grep -E "^test result"
cargo clippy -- -D warnings 2>&1 | grep -c "^error" || true
```

Expected: `test result: ok. N passed; 0 failed` and `0` errors from clippy.
