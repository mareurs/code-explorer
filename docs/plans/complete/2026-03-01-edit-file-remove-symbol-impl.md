# `edit_file` + `remove_symbol` Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `edit_lines` with `edit_file` (old_string/new_string semantics) and add `remove_symbol` to complete the symbol CRUD set.

**Architecture:** `edit_file` is a new Tool struct in `src/tools/file.rs` using simple string matching. `remove_symbol` is a new Tool struct in `src/tools/symbol.rs` modeled closely on `replace_symbol`. `edit_lines` is deleted entirely.

**Tech Stack:** Rust, serde_json, anyhow, async_trait. Tests use tempfile + the `project_ctx()` helper from the existing test infrastructure.

**Design doc:** `docs/plans/2026-03-01-edit-file-remove-symbol-design.md`

---

### Task 1: Add `edit_file` — tests first

**Files:**
- Modify: `src/tools/file.rs` — add `EditFile` struct + impl + tests

**Step 1: Write the `EditFile` struct and trait impl skeleton**

Add after the `EditLines` block (we'll delete `EditLines` in Task 4). The struct goes near line 537.

```rust
pub struct EditFile;

#[async_trait::async_trait]
impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Find and replace text in a file. Requires old_string to match exactly — include enough context to be unique."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path", "old_string", "new_string"],
            "properties": {
                "path": { "type": "string", "description": "File path" },
                "old_string": { "type": "string", "description": "Exact text to find (must match file content including whitespace and indentation)" },
                "new_string": { "type": "string", "description": "Replacement text. Empty string deletes the match." },
                "replace_all": { "type": "boolean", "description": "Replace all occurrences instead of requiring a unique match (default: false)" }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        super::guard_worktree_write(ctx).await?;
        let path = super::require_str_param(&input, "path")?;
        let old_string = super::require_str_param(&input, "old_string")?;
        let new_string = super::require_str_param(&input, "new_string")?;
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        if old_string.is_empty() {
            return Err(super::RecoverableError::with_hint(
                "old_string must not be empty",
                "Use create_file to write new files, or insert_code to add code relative to a symbol.",
            ).into());
        }

        let root = ctx.agent.require_project_root().await?;
        let security = ctx.agent.security_config().await;
        let resolved = crate::util::path_security::validate_write_path(path, &root, &security)?;

        let content = std::fs::read_to_string(&resolved)?;
        let match_count = content.matches(old_string).count();

        if match_count == 0 {
            return Err(super::RecoverableError::with_hint(
                format!("old_string not found in {}", path),
                "Check whitespace and indentation — they must match exactly. Use search_pattern to verify the content.",
            ).into());
        }

        if match_count > 1 && !replace_all {
            // Find line numbers of each match for the error message
            let line_numbers: Vec<usize> = content
                .match_indices(old_string)
                .map(|(byte_offset, _)| {
                    content[..byte_offset].lines().count() + 1
                })
                .collect();
            return Err(super::RecoverableError::with_hint(
                format!(
                    "old_string found {} times (lines {})",
                    match_count,
                    line_numbers.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
                ),
                "Include more surrounding context to make it unique, or pass replace_all: true.",
            ).into());
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        std::fs::write(&resolved, &new_content)?;
        ctx.lsp.notify_file_changed(&resolved).await;

        Ok(json!("ok"))
    }
}
```

**Step 2: Write tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `src/tools/file.rs`:

```rust
#[tokio::test]
async fn edit_file_replaces_unique_match() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "hello world\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "hello",
                "new_string": "goodbye"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result, json!("ok"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "goodbye world\n");
}

#[tokio::test]
async fn edit_file_empty_new_string_deletes() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "aaa bbb ccc\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": " bbb",
                "new_string": ""
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result, json!("ok"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "aaa ccc\n");
}

#[tokio::test]
async fn edit_file_not_found_errors() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "hello world\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "does not exist",
                "new_string": "replacement"
            }),
            &ctx,
        )
        .await;
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not found"), "expected 'not found' in: {err}");
}

#[tokio::test]
async fn edit_file_multiple_matches_without_replace_all_errors() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "foo bar foo baz foo\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "qux"
            }),
            &ctx,
        )
        .await;
    let err = result.unwrap_err().to_string();
    assert!(err.contains("3 times"), "expected '3 times' in: {err}");
    // File must be untouched
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "foo bar foo baz foo\n"
    );
}

#[tokio::test]
async fn edit_file_replace_all_replaces_all_occurrences() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "foo bar foo baz foo\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "qux",
                "replace_all": true
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result, json!("ok"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "qux bar qux baz qux\n"
    );
}

#[tokio::test]
async fn edit_file_empty_old_string_errors() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "",
                "new_string": "something"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_file_multiline_replace() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "fn old() {\n    todo!()\n}\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "fn old() {\n    todo!()\n}",
                "new_string": "fn new_func() {\n    42\n}"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result, json!("ok"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn new_func() {\n    42\n}\n"
    );
}

#[tokio::test]
async fn edit_file_whitespace_sensitive() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "    indented\n").unwrap();

    // Wrong indentation should fail
    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "indented",
                "new_string": "replaced"
            }),
            &ctx,
        )
        .await;
    // "indented" appears once (as substring of "    indented"), so it succeeds
    // This is intentional — exact substring matching, not line matching
    assert!(result.is_ok());
}

#[tokio::test]
async fn edit_file_returns_ok_string() {
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content\n").unwrap();

    let result = EditFile
        .call(
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "content",
                "new_string": "new"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result, json!("ok"));
    assert!(result.is_string(), "response must be a plain string, not an object");
}
```

**Step 3: Run tests**

Run: `cargo test edit_file -- --nocapture`
Expected: All 9 tests pass.

**Step 4: Commit**

```bash
git add src/tools/file.rs
git commit -m "feat: add edit_file tool with old_string/new_string semantics"
```

---

### Task 2: Register `edit_file` and wire security

**Files:**
- Modify: `src/server.rs:23,68` — replace `EditLines` import/registration with `EditFile`
- Modify: `src/util/path_security.rs:290` — replace `"edit_lines"` with `"edit_file"` in `check_tool_access`

**Step 1: Update server.rs import**

In `src/server.rs` line 23, change:
```rust
file::{CreateFile, EditLines, FindFile, ListDir, ReadFile, SearchPattern},
```
to:
```rust
file::{CreateFile, EditFile, FindFile, ListDir, ReadFile, SearchPattern},
```

**Step 2: Update server.rs registration**

In `src/server.rs` line 68, change:
```rust
Arc::new(EditLines),
```
to:
```rust
Arc::new(EditFile),
```

**Step 3: Update check_tool_access**

In `src/util/path_security.rs` line 290, change:
```rust
"create_file" | "edit_lines" | "replace_symbol" | "insert_code" | "rename_symbol" => {
```
to:
```rust
"create_file" | "edit_file" | "replace_symbol" | "insert_code" | "rename_symbol" | "remove_symbol" => {
```

(Add both `edit_file` and `remove_symbol` now since we're touching this line.)

**Step 4: Run full test suite**

Run: `cargo test`
Expected: All tests pass (edit_lines tests will still pass since EditLines struct still exists, just not registered).

**Step 5: Commit**

```bash
git add src/server.rs src/util/path_security.rs
git commit -m "feat: register edit_file, add edit_file+remove_symbol to security gate"
```

---

### Task 3: Add `remove_symbol` — tests first

**Files:**
- Modify: `src/tools/symbol.rs` — add `RemoveSymbol` struct + impl + tests

**Step 1: Write the `RemoveSymbol` struct and trait impl**

Add after the `ReplaceSymbol` impl block (ends around line 1147). Model closely on `ReplaceSymbol::call`.

```rust
pub struct RemoveSymbol;

#[async_trait::async_trait]
impl Tool for RemoveSymbol {
    fn name(&self) -> &str {
        "remove_symbol"
    }

    fn description(&self) -> &str {
        "Delete a symbol (function, struct, impl block, test, etc.) by name. Removes the entire declaration including doc comments and attributes."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "path"],
            "properties": {
                "name_path": { "type": "string", "description": "Symbol name path (e.g. 'MyStruct/my_method', 'tests/old_test')" },
                "path": { "type": "string", "description": "File path" }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        super::guard_worktree_write(ctx).await?;
        let name_path = super::require_str_param(&input, "name_path")?;
        let rel_path = get_path_param(&input, true)?.unwrap();

        let full_path = resolve_write_path(ctx, rel_path).await?;
        let (client, lang) = get_lsp_client(ctx, &full_path).await?;

        let symbols = client.document_symbols(&full_path, &lang).await?;
        let sym = find_symbol_by_name_path(&symbols, name_path).ok_or_else(|| {
            RecoverableError::with_hint(
                format!("symbol not found: {}", name_path),
                "Use list_symbols(path) to see available symbols, or check the name_path spelling.",
            )
        })?;

        let content = std::fs::read_to_string(&full_path)?;
        let lines: Vec<&str> = content.lines().collect();

        let trimmed_start = trim_symbol_start(sym.start_line as usize, &lines);
        let end = (sym.end_line as usize + 1).min(lines.len());

        // Scan backwards from trimmed_start to include doc comments and attributes
        let start = scan_backwards_for_docs(trimmed_start, &lines);

        // Build new lines: everything before the symbol, everything after
        let mut new_lines: Vec<&str> = Vec::new();
        new_lines.extend_from_slice(&lines[..start]);
        new_lines.extend_from_slice(&lines[end..]);

        // Collapse runs of 3+ blank lines down to 1
        let new_lines = collapse_blank_lines(&new_lines);

        write_lines(&full_path, &new_lines, content.ends_with('\n'))?;
        ctx.lsp.notify_file_changed(&full_path).await;
        Ok(json!("ok"))
    }
}
```

**Step 2: Write the `scan_backwards_for_docs` helper**

Add near `trim_symbol_start` (around line 1082):

```rust
/// Scan backwards from `start` to include contiguous doc comments (`///`, `//!`),
/// attributes (`#[...]`), and blank lines between them. Stops at the first line
/// that doesn't match these patterns.
fn scan_backwards_for_docs(start: usize, lines: &[&str]) -> usize {
    let mut s = start;
    while s > 0 {
        let t = lines[s - 1].trim();
        if t.is_empty()
            || t.starts_with("///")
            || t.starts_with("//!")
            || t.starts_with("#[")
        {
            s -= 1;
        } else {
            break;
        }
    }
    s
}
```

**Step 3: Write the `collapse_blank_lines` helper**

Add near the other helpers:

```rust
/// Collapse runs of 3+ consecutive blank lines down to 1 blank line.
fn collapse_blank_lines<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let mut result: Vec<&'a str> = Vec::new();
    let mut blank_run = 0;
    for &line in lines {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                result.push(line);
            }
        } else {
            blank_run = 0;
            result.push(line);
        }
    }
    result
}
```

**Step 4: Write tests**

Add to the `#[cfg(test)] mod tests` block in `src/tools/symbol.rs`. These tests
need LSP, so they go in `tests/symbol_lsp.rs` if they require real LSP, or in
the unit test module if they can use the mock. Use the same pattern as the
existing `replace_symbol_preserves_preceding_close_brace` test.

For the helpers, unit tests go inline:

```rust
#[test]
fn scan_backwards_includes_doc_comments() {
    let lines = vec!["other code", "", "/// Doc line 1", "/// Doc line 2", "fn foo() {}"];
    assert_eq!(scan_backwards_for_docs(4, &lines), 2);
}

#[test]
fn scan_backwards_includes_attributes() {
    let lines = vec!["other code", "#[test]", "#[ignore]", "fn foo() {}"];
    assert_eq!(scan_backwards_for_docs(3, &lines), 1);
}

#[test]
fn scan_backwards_stops_at_code() {
    let lines = vec!["let x = 1;", "fn foo() {}"];
    assert_eq!(scan_backwards_for_docs(1, &lines), 1);
}

#[test]
fn scan_backwards_at_start_of_file() {
    let lines = vec!["/// Doc", "fn foo() {}"];
    assert_eq!(scan_backwards_for_docs(1, &lines), 0);
}

#[test]
fn collapse_blank_lines_collapses_triple() {
    let lines = vec!["a", "", "", "", "b"];
    let result = collapse_blank_lines(&lines);
    assert_eq!(result, vec!["a", "", "b"]);
}

#[test]
fn collapse_blank_lines_preserves_single() {
    let lines = vec!["a", "", "b"];
    let result = collapse_blank_lines(&lines);
    assert_eq!(result, vec!["a", "", "b"]);
}
```

**Step 5: Run tests**

Run: `cargo test scan_backwards collapse_blank -- --nocapture`
Expected: All 6 helper tests pass.

**Step 6: Commit**

```bash
git add src/tools/symbol.rs
git commit -m "feat: add remove_symbol tool — deletes symbols by name including docs/attrs"
```

---

### Task 4: Register `remove_symbol`

**Files:**
- Modify: `src/server.rs` — add import and registration

**Step 1: Update import**

In `src/server.rs` around line 29, change:
```rust
    symbol::{
        FindReferences, FindSymbol, GotoDefinition, Hover, InsertCode, ListSymbols, RenameSymbol,
        ReplaceSymbol,
    },
```
to:
```rust
    symbol::{
        FindReferences, FindSymbol, GotoDefinition, Hover, InsertCode, ListSymbols, RemoveSymbol,
        RenameSymbol, ReplaceSymbol,
    },
```

**Step 2: Add registration**

After `Arc::new(ReplaceSymbol),` (around line 78), add:
```rust
Arc::new(RemoveSymbol),
```

**Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass.

**Step 4: Commit**

```bash
git add src/server.rs
git commit -m "feat: register remove_symbol in server"
```

---

### Task 5: Delete `edit_lines` entirely

**Files:**
- Modify: `src/tools/file.rs` — remove `EditLines` struct, impl, and all `edit_lines_*` tests
- Modify: `src/server.rs` — remove any lingering import (already replaced in Task 2)

**Step 1: Remove `EditLines` struct and impl**

Delete `pub struct EditLines;` (line 537) through the closing `}` of
`impl Tool for EditLines` (line 664).

**Step 2: Remove all `edit_lines_*` tests**

Delete tests from `edit_lines_replace_single_line` (line 1518) through
`edit_lines_expected_content_multiline_mismatch_errors` (line 1834).

**Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass. No references to EditLines remain.

**Step 4: Verify no stale references**

Run: `cargo clippy -- -D warnings`
Run: `grep -r "EditLines\|edit_lines" src/`
Expected: No matches except possibly in comments/docs.

**Step 5: Commit**

```bash
git add src/tools/file.rs
git commit -m "refactor: remove edit_lines tool — replaced by edit_file"
```

---

### Task 6: Update server instructions and CLAUDE.md

**Files:**
- Modify: `src/prompts/server_instructions.md` — replace edit_lines references with edit_file, add remove_symbol
- Modify: `CLAUDE.md` — update tool list and references

**Step 1: Update server_instructions.md**

In the "Edit code" section (around line 49-52), replace the `edit_lines` line:
```markdown
- `edit_lines(path, start_line, delete_count, new_text, expected_content?)` — ...
```
with:
```markdown
- `edit_file(path, old_string, new_string, replace_all?)` — find-and-replace: locates old_string in the file and replaces it with new_string. Must match exactly (whitespace-sensitive). Fails if not found; fails if multiple matches unless replace_all is true. Empty new_string deletes the match.
- `remove_symbol(name_path, path)` — delete a symbol entirely, including its doc comments and attributes
```

In the rules section (around line 112), replace:
```markdown
6. **Prefer symbol edits** (`replace_symbol`, `insert_code`, `rename_symbol`) over `edit_lines` for code files.
```
with:
```markdown
6. **Prefer symbol edits** (`replace_symbol`, `insert_code`, `remove_symbol`, `rename_symbol`) for code. Use `edit_file` when symbol tools don't fit.
```

**Step 2: Update CLAUDE.md**

Replace `edit_lines` references:
- Line 27: change `edit_lines` to `edit_file` in the tool misbehavior list
- Line 56: change `edit_lines` to `edit_file` in the file.rs description
- Line 82: change `edit_lines` to `edit_file` in the "No Echo" principle

**Step 3: Update docs/TODO-tool-misbehaviors.md**

Add a note to BUG-001:
```markdown
**Status:** ✅ SUPERSEDED — `edit_lines` removed; replaced by `edit_file` (old_string/new_string)
```

**Step 4: Run tests one final time**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: All green.

**Step 5: Commit**

```bash
git add src/prompts/server_instructions.md CLAUDE.md docs/TODO-tool-misbehaviors.md
git commit -m "docs: update instructions for edit_file and remove_symbol"
```

---

### Task 7: Integration smoke test

**Step 1: Build and run the server**

Run: `cargo build`
Expected: Clean build.

**Step 2: Verify tool list**

Run: `echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -- start --project . 2>/dev/null | head -50`
Expected: `edit_file` and `remove_symbol` appear in tool list. `edit_lines` does not.

**Step 3: Commit all remaining changes**

If anything was missed, commit now.

```bash
git add -A
git commit -m "chore: final cleanup for edit_file + remove_symbol"
```
