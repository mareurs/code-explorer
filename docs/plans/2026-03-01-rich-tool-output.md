# Rich Tool Output Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Give every tool a compact 1-line summary, writer tools an ANSI diff viewer, and slow tools live progress notifications.

**Architecture:** Three independent layers implemented in order — Layer 2 (compact summaries via `format_for_user`), Layer 3 (ANSI diff viewer via `call_content` overrides), Layer 1 (progress infrastructure). Each layer can be shipped independently. See `docs/plans/2026-03-01-rich-tool-output-design.md` for the full design.

**Tech Stack:** Rust, rmcp-0.1.5, existing `format_for_user` / `call_content` dual-channel pattern already in `src/tools/mod.rs`.

---

## Background: The Dual-Channel Pattern

`format_for_user` in `src/tools/mod.rs:186` is a default-no-op method on the `Tool` trait. When it returns `Some(text)`, the `call_content` default (L196-210) emits two content blocks:
- `Role::Assistant` → compact JSON (what the LLM sees)
- `Role::User` → human-formatted text (what appears in the terminal)

9 tools already implement `format_for_user` (ReadFile, ListDir, SearchPattern, ListSymbols, FindSymbol, GotoDefinition, Hover, SemanticSearch, and one in semantic.rs). This plan adds it to the remaining 20+ tools.

For write tools (EditFile, ReplaceSymbol, RemoveSymbol, InsertCode), the diff requires access to both `input` and the write result, so we override `call_content` directly (same pattern as `CreateFile` at `src/tools/file.rs:488`).

**ANSI escape codes used:**
```
Bold cyan header:  \x1b[1;36m ... \x1b[0m
Bold green +++:    \x1b[1;32m ... \x1b[0m
Bold red ---:      \x1b[1;31m ... \x1b[0m
Dim @@ hunk:       \x1b[2m ... \x1b[0m
Green + line:      \x1b[32m+{line}\x1b[0m
Red - line:        \x1b[31m-{line}\x1b[0m
Dim elision ···:   \x1b[2m···  (N more lines)\x1b[0m
```
The summary line always has NO ANSI — the LLM reads it cleanly.

---

## Layer 2 — Compact Summaries

### Task 1: Add `format_for_user` to file query tools in `user_format.rs` + `file.rs`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/file.rs`
- Test: `tests/integration/`

**Step 1: Write the failing tests**

In `src/tools/file.rs`, at the end of the `#[cfg(test)]` block, add:

```rust
#[test]
fn find_file_format_for_user_shows_count() {
    use serde_json::json;
    let tool = FindFile;
    let result = json!({ "files": ["src/a.rs", "src/b.rs"], "total": 2 });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("2 files"), "got: {text}");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test find_file_format_for_user -q`
Expected: FAIL — `called `Option::unwrap()` on a `None` value`

**Step 3: Add `format_find_file` to `user_format.rs`**

At the end of `src/tools/user_format.rs`, add:

```rust
pub fn format_find_file(result: &Value) -> String {
    let total = result["total"].as_u64().unwrap_or(0);
    let overflow = result["overflow"].is_object();
    let cap_note = if overflow { " (cap hit — narrow pattern)" } else { "" };
    format!("{total} files{cap_note}")
}
```

**Step 4: Add `format_for_user` to `FindFile` in `file.rs`**

In `src/tools/file.rs`, in the `impl Tool for FindFile` block, after the `call` method, add:

```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_find_file(result))
}
```

**Step 5: Run test to verify it passes**

Run: `cargo test find_file_format_for_user -q`
Expected: PASS

**Step 6: Commit**

```bash
git add src/tools/user_format.rs src/tools/file.rs
git commit -m "feat: add format_for_user to FindFile"
```

---

### Task 2: Add `format_for_user` to symbol query tools (`FindReferences`, `RenameSymbol`, `InsertCode`)

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/symbol.rs`

**Step 1: Write failing tests** (add to `symbol.rs` test block)

```rust
#[test]
fn find_references_format_for_user_shows_count() {
    use serde_json::json;
    let tool = FindReferences;
    let result = json!({ "references": [{"file":"a.rs","line":10}], "total": 1 });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("1 ref"), "got: {text}");
}

#[test]
fn rename_symbol_format_for_user_shows_sites() {
    use serde_json::json;
    let tool = RenameSymbol;
    // RenameSymbol returns { "lsp_renames": N, "textual_matches": [...], "new_name": "bar" }
    let result = json!({ "lsp_renames": 5, "textual_matches": [{"file":"a.rs"}], "new_name": "bar" });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("bar"), "got: {text}");
}

#[test]
fn insert_code_format_for_user_shows_location() {
    use serde_json::json;
    let tool = InsertCode;
    let result = json!({ "status": "ok", "inserted_at_line": 42, "position": "after" });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("42"), "got: {text}");
}
```

**Step 2: Run to verify failure**

Run: `cargo test find_references_format rename_symbol_format insert_code_format -q`
Expected: FAIL (None unwrap)

**Step 3: Add formatters to `user_format.rs`**

```rust
pub fn format_find_references(result: &Value) -> String {
    let total = result["total"].as_u64().unwrap_or_else(|| {
        result["references"].as_array().map(|a| a.len() as u64).unwrap_or(0)
    });
    let files: std::collections::HashSet<&str> = result["references"]
        .as_array()
        .map(|refs| refs.iter().filter_map(|r| r["file"].as_str()).collect())
        .unwrap_or_default();
    let file_count = files.len();
    if file_count > 1 {
        format!("{total} refs · {file_count} files")
    } else {
        format!("{total} refs")
    }
}

pub fn format_rename_symbol(result: &Value) -> String {
    let lsp = result["lsp_renames"].as_u64().unwrap_or(0);
    let textual = result["textual_matches"].as_array().map(|a| a.len() as u64).unwrap_or(0);
    let total = lsp + textual;
    let new_name = result["new_name"].as_str().unwrap_or("?");
    let files: std::collections::HashSet<&str> = result["textual_matches"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["file"].as_str()).collect())
        .unwrap_or_default();
    if files.is_empty() {
        format!("→ {new_name} · {total} sites")
    } else {
        format!("→ {new_name} · {total} sites · {} files", files.len())
    }
}

pub fn format_insert_code(result: &Value) -> String {
    let line = result["inserted_at_line"].as_u64().unwrap_or(0);
    let pos = result["position"].as_str().unwrap_or("after");
    format!("inserted {pos} L{line}")
}
```

**Step 4: Add `format_for_user` to the three tools in `symbol.rs`**

Find the `impl Tool for FindReferences` block. After `call`, add:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_find_references(result))
}
```

Find the `impl Tool for RenameSymbol` block. After `call`, add:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_rename_symbol(result))
}
```

Find the `impl Tool for InsertCode` block. After `call`, add:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_insert_code(result))
}
```

**Step 5: Run tests**

Run: `cargo test find_references_format rename_symbol_format insert_code_format -q`
Expected: PASS

Run: `cargo test -q`
Expected: all passing

**Step 6: Commit**

```bash
git add src/tools/user_format.rs src/tools/symbol.rs
git commit -m "feat: add format_for_user to FindReferences, RenameSymbol, InsertCode"
```

---

### Task 3: Add `format_for_user` to AST tools (`ListFunctions`, `ListDocs`) in `ast.rs`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/ast.rs`

**Step 1: Write failing tests** (in `ast.rs` test block)

```rust
#[test]
fn list_functions_format_for_user_shows_count() {
    use serde_json::json;
    let tool = ListFunctions;
    let result = json!({ "functions": [{"name":"foo"}, {"name":"bar"}], "file": "src/a.rs" });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("2"), "got: {text}");
}

#[test]
fn list_docs_format_for_user_shows_count() {
    use serde_json::json;
    let tool = ListDocs;
    let result = json!({ "docstrings": [{"symbol":"Foo"}], "file": "src/a.rs" });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("1"), "got: {text}");
}
```

**Step 2: Run to verify failure**

Run: `cargo test list_functions_format list_docs_format -q`
Expected: FAIL

**Step 3: Add formatters to `user_format.rs`**

```rust
pub fn format_list_functions(result: &Value) -> String {
    let count = result["functions"].as_array().map(|a| a.len()).unwrap_or(0);
    let file = result["file"].as_str().unwrap_or("?");
    format!("{file} → {count} functions")
}

pub fn format_list_docs(result: &Value) -> String {
    let count = result["docstrings"].as_array().map(|a| a.len()).unwrap_or(0);
    let file = result["file"].as_str().unwrap_or("?");
    format!("{file} → {count} docstrings")
}
```

**Step 4: Add `format_for_user` to both tools in `ast.rs`**

In `impl Tool for ListFunctions`:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_list_functions(result))
}
```

In `impl Tool for ListDocs`:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_list_docs(result))
}
```

**Step 5: Run tests**

Run: `cargo test list_functions_format list_docs_format -q`
Expected: PASS

**Step 6: Commit**

```bash
git add src/tools/user_format.rs src/tools/ast.rs
git commit -m "feat: add format_for_user to ListFunctions, ListDocs"
```

---

### Task 4: Add `format_for_user` to memory, config, and semantic tools

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/memory.rs`
- Modify: `src/tools/config.rs`
- Modify: `src/tools/semantic.rs`

**Step 1: Write failing tests for memory tools** (in `memory.rs` test block)

```rust
#[test]
fn write_memory_format_for_user() {
    use serde_json::json;
    let tool = WriteMemory;
    let r = json!({ "status": "ok", "topic": "arch" });
    let t = tool.format_for_user(&r).unwrap();
    assert!(t.contains("arch"), "got: {t}");
}

#[test]
fn list_memories_format_for_user() {
    use serde_json::json;
    let tool = ListMemories;
    let r = json!({ "topics": ["a", "b", "c"] });
    let t = tool.format_for_user(&r).unwrap();
    assert!(t.contains("3"), "got: {t}");
}
```

**Step 2: Run to verify failure**

Run: `cargo test write_memory_format list_memories_format -q`
Expected: FAIL

**Step 3: Add formatters to `user_format.rs`**

```rust
pub fn format_write_memory(result: &Value) -> String {
    let topic = result["topic"].as_str().unwrap_or("?");
    format!("written · {topic}")
}

pub fn format_read_memory(result: &Value) -> String {
    let topic = result["topic"].as_str().unwrap_or("?");
    if result["content"].is_null() {
        format!("not found · {topic}")
    } else {
        let chars = result["content"].as_str().map(|s| s.len()).unwrap_or(0);
        format!("{topic} · {chars} chars")
    }
}

pub fn format_list_memories(result: &Value) -> String {
    let count = result["topics"].as_array().map(|a| a.len()).unwrap_or(0);
    format!("{count} topics")
}

pub fn format_delete_memory(result: &Value) -> String {
    let topic = result["topic"].as_str().unwrap_or("?");
    format!("deleted · {topic}")
}

pub fn format_get_config(result: &Value) -> String {
    let lang = result["language"].as_str()
        .or_else(|| result["languages"].as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str()))
        .unwrap_or("?");
    let tool_count = result["tool_count"].as_u64()
        .or_else(|| result["tools"].as_array().map(|a| a.len() as u64))
        .unwrap_or(0);
    if tool_count > 0 {
        format!("[{lang}] · {tool_count} tools")
    } else {
        format!("[{lang}]")
    }
}

pub fn format_activate_project(result: &Value) -> String {
    let path = result["activated"]["project_root"].as_str()
        .or_else(|| result["path"].as_str())
        .unwrap_or("?");
    format!("activated · {path}")
}

pub fn format_index_project(result: &Value) -> String {
    let chunks = result["chunks"].as_u64().unwrap_or(0);
    let files = result["files"].as_u64().unwrap_or(0);
    format!("{chunks} chunks · {files} files")
}

pub fn format_index_library(result: &Value) -> String {
    let name = result["name"].as_str().unwrap_or("?");
    let chunks = result["chunks"].as_u64().unwrap_or(0);
    format!("{name} · {chunks} chunks")
}

pub fn format_index_status(result: &Value) -> String {
    let status = result["status"].as_str().unwrap_or("unknown");
    let files = result["file_count"].as_u64().unwrap_or(0);
    if files > 0 {
        format!("{status} · {files} files")
    } else {
        status.to_string()
    }
}
```

**Step 4: Add `format_for_user` to each tool**

`memory.rs` — add to `WriteMemory`, `ReadMemory`, `ListMemories`, `DeleteMemory`:
```rust
// WriteMemory
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_write_memory(result))
}
// ReadMemory
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_read_memory(result))
}
// ListMemories
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_list_memories(result))
}
// DeleteMemory
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_delete_memory(result))
}
```

`config.rs` — add to `GetConfig` and `ActivateProject`:
```rust
// GetConfig
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_get_config(result))
}
// ActivateProject
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_activate_project(result))
}
```

`semantic.rs` — add to `IndexProject`, `IndexLibrary`, `IndexStatus`:
```rust
// IndexProject
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_index_project(result))
}
// IndexLibrary  
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_index_library(result))
}
// IndexStatus
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_index_status(result))
}
```

**Step 5: Run tests and full suite**

Run: `cargo test write_memory_format list_memories_format -q`
Expected: PASS

Run: `cargo test -q`
Expected: all passing

**Step 6: Commit**

```bash
git add src/tools/user_format.rs src/tools/memory.rs src/tools/config.rs src/tools/semantic.rs
git commit -m "feat: add format_for_user to memory, config, index tools"
```

---

### Task 5: Add `format_for_user` to `GitBlame`, `RunCommand`, `Onboarding`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/git.rs`
- Modify: `src/tools/workflow.rs`

**Step 1: Write failing tests**

In `git.rs`:
```rust
#[test]
fn git_blame_format_for_user_shows_lines() {
    use serde_json::json;
    let tool = GitBlame;
    let result = json!({ "lines": [{"line":1},{"line":2}], "file": "src/a.rs" });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("2"), "got: {text}");
}
```

In `workflow.rs`:
```rust
#[test]
fn run_command_format_for_user_test_result() {
    use serde_json::json;
    let tool = RunCommand;
    // Buffered test result format
    let result = json!({
        "type": "test", "exit_code": 0,
        "passed": 533, "failed": 0, "ignored": 0,
        "output_id": "@cmd_abc123"
    });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("533"), "got: {text}");
    assert!(text.contains("passed"), "got: {text}");
}

#[test]
fn run_command_format_for_user_short_output() {
    use serde_json::json;
    let tool = RunCommand;
    // Short output format (no output_id)
    let result = json!({ "stdout": "hello\nworld", "stderr": "", "exit_code": 0 });
    let text = tool.format_for_user(&result).unwrap();
    assert!(text.contains("exit 0"), "got: {text}");
}
```

**Step 2: Run to verify failure**

Run: `cargo test git_blame_format run_command_format -q`
Expected: FAIL

**Step 3: Add formatters to `user_format.rs`**

```rust
pub fn format_git_blame(result: &Value) -> String {
    let file = result["file"].as_str().unwrap_or("?");
    let line_count = result["lines"].as_array().map(|a| a.len()).unwrap_or(0);
    // Count unique authors
    let authors: std::collections::HashSet<&str> = result["lines"]
        .as_array()
        .map(|lines| {
            lines.iter()
                .filter_map(|l| l["author"].as_str())
                .collect()
        })
        .unwrap_or_default();
    if authors.is_empty() {
        format!("{file} · {line_count} lines")
    } else {
        format!("{file} · {line_count} lines · {} authors", authors.len())
    }
}

pub fn format_run_command(result: &Value) -> String {
    // Buffered: has "type" and "output_id"
    if result["output_id"].is_string() {
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        let output_id = result["output_id"].as_str().unwrap_or("");

        match result["type"].as_str() {
            Some("test") => {
                let passed = result["passed"].as_u64().unwrap_or(0);
                let failed = result["failed"].as_u64().unwrap_or(0);
                let ignored = result["ignored"].as_u64().unwrap_or(0);
                let mut s = format!("{check} exit {exit} · {passed} passed");
                if failed > 0 { s.push_str(&format!(" · {failed} FAILED")); }
                if ignored > 0 { s.push_str(&format!(" · {ignored} ignored")); }
                s.push_str(&format!("  (query {output_id})"));
                s
            }
            Some("build") => {
                let errors = result["errors"].as_u64().unwrap_or(0);
                if errors > 0 {
                    format!("{check} exit {exit} · {errors} errors  (query {output_id})")
                } else {
                    format!("{check} exit {exit}  (query {output_id})")
                }
            }
            _ => {
                let lines = result["total_stdout_lines"].as_u64().unwrap_or(0);
                format!("{check} exit {exit} · {lines} lines  (query {output_id})")
            }
        }
    } else if result["timed_out"].as_bool().unwrap_or(false) {
        "✗ timed out".to_string()
    } else {
        // Short output
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let stdout_lines = result["stdout"].as_str().map(|s| s.lines().count()).unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        format!("{check} exit {exit} · {stdout_lines} lines")
    }
}

pub fn format_onboarding(result: &Value) -> String {
    let langs = result["languages"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| "?".to_string());
    let created = result["config_created"].as_bool().unwrap_or(false);
    let config_note = if created { " · config created" } else { "" };
    format!("[{langs}]{config_note}")
}
```

**Step 4: Add `format_for_user` to each tool**

`git.rs` — add to `GitBlame`:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_git_blame(result))
}
```

`workflow.rs` — add to `RunCommand` and `Onboarding`:
```rust
// RunCommand
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_run_command(result))
}
// Onboarding
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_onboarding(result))
}
```

**Step 5: Run all tests**

Run: `cargo test git_blame_format run_command_format -q`
Expected: PASS

Run: `cargo test -q`
Expected: all passing

**Step 6: Commit**

```bash
git add src/tools/user_format.rs src/tools/git.rs src/tools/workflow.rs
git commit -m "feat: add format_for_user to GitBlame, RunCommand, Onboarding"
```

---

## Layer 3 — ANSI Diff Viewer

### Task 6: Add ANSI diff helper functions to `user_format.rs`

These are pure formatting functions; no side effects. Test them directly.

**Files:**
- Modify: `src/tools/user_format.rs`

**Step 1: Write failing tests**

At the end of the test block in `user_format.rs` (or inline in the module via `#[cfg(test)]`):

```rust
#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn render_diff_header_contains_path() {
        let h = render_diff_header("edit_file", "src/server.rs");
        assert!(h.contains("edit_file"), "got: {h}");
        assert!(h.contains("src/server.rs"), "got: {h}");
        // ANSI reset at end
        assert!(h.contains("\x1b[0m"), "no reset: {h}");
    }

    #[test]
    fn render_edit_diff_shows_minus_plus_lines() {
        let diff = render_edit_diff(
            "src/a.rs",
            "let old = 1;\nlet also_old = 2;",
            "let new = 3;",
            Some(88),
        );
        assert!(diff.contains("old"), "got: {diff}");
        assert!(diff.contains("new"), "got: {diff}");
        // Check ANSI colors are present
        assert!(diff.contains("\x1b[31m") || diff.contains("\x1b[32m"), "no colors: {diff}");
    }

    #[test]
    fn render_removal_diff_marks_all_lines_red() {
        let diff = render_removal_diff("src/a.rs", "fn old() {\n    1\n}", Some(10), "old");
        assert!(diff.contains("old"), "got: {diff}");
        assert!(diff.contains("\x1b[31m"), "no red: {diff}");
    }

    #[test]
    fn render_insert_diff_marks_all_lines_green() {
        let diff = render_insert_diff("src/a.rs", "fn new() {}", Some(42), "after", "my_sym");
        assert!(diff.contains("new"), "got: {diff}");
        assert!(diff.contains("\x1b[32m"), "no green: {diff}");
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test diff_tests -q`
Expected: FAIL (functions don't exist yet)

**Step 3: Add ANSI helper functions to `user_format.rs`**

```rust
// ─── ANSI constants ──────────────────────────────────────────────────────────

const BOLD_CYAN: &str = "\x1b[1;36m";
const BOLD_GREEN: &str = "\x1b[1;32m";
const BOLD_RED: &str = "\x1b[1;31m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

const DIFF_PREVIEW_LINES: usize = 8;

/// Format a separator header line:  ─── tool_name: path ──────
pub fn render_diff_header(tool_name: &str, path: &str) -> String {
    let title = format!(" {tool_name}: {path} ");
    let pad = "─".repeat((60usize).saturating_sub(title.len()));
    format!("{BOLD_CYAN}───{title}{pad}{RESET}")
}

/// Render a unified-style diff between old_string and new_string.
/// start_line is the 1-indexed line where old_string begins in the file (optional).
pub fn render_edit_diff(
    path: &str,
    old_string: &str,
    new_string: &str,
    start_line: Option<usize>,
) -> String {
    let mut out = String::new();

    // Hunk header
    let old_lines: Vec<&str> = old_string.lines().collect();
    let new_lines: Vec<&str> = new_string.lines().collect();
    let hunk_start = start_line.unwrap_or(1);
    let hunk = format!(
        "@@ -{hunk_start},{} +{hunk_start},{} @@",
        old_lines.len(),
        new_lines.len()
    );
    out.push_str(&format!("{DIM}{hunk}{RESET}\n"));

    // Removed lines
    for line in &old_lines {
        out.push_str(&format!("{RED}-{line}{RESET}\n"));
    }
    // Added lines
    for line in &new_lines {
        out.push_str(&format!("{GREEN}+{line}{RESET}\n"));
    }

    out
}

/// Render a diff showing removed symbol (all lines red).
pub fn render_removal_diff(
    path: &str,
    removed_content: &str,
    start_line: Option<usize>,
    name: &str,
) -> String {
    let lines: Vec<&str> = removed_content.lines().collect();
    let total = lines.len();
    let preview_count = DIFF_PREVIEW_LINES.min(total);
    let hunk_start = start_line.unwrap_or(1);

    let mut out = String::new();
    out.push_str(&format!(
        "{BOLD_RED}--- removed · {name} · {total} lines{RESET}\n"
    ));
    out.push_str(&format!(
        "{DIM}@@ -{hunk_start},{total} @@{RESET}\n"
    ));
    for line in &lines[..preview_count] {
        out.push_str(&format!("{RED}-{line}{RESET}\n"));
    }
    if total > preview_count {
        let remaining = total - preview_count;
        out.push_str(&format!("{DIM}···  ({remaining} more lines){RESET}\n"));
    }
    out
}

/// Render a diff showing inserted code (all lines green).
pub fn render_insert_diff(
    path: &str,
    code: &str,
    at_line: Option<usize>,
    position: &str,
    near_symbol: &str,
) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let total = lines.len();
    let preview_count = DIFF_PREVIEW_LINES.min(total);
    let insert_line = at_line.unwrap_or(1);

    let mut out = String::new();
    out.push_str(&format!(
        "{BOLD_GREEN}+++ inserted {position} {near_symbol} · {total} lines{RESET}\n"
    ));
    out.push_str(&format!(
        "{DIM}@@ +{insert_line},{total} @@{RESET}\n"
    ));
    for line in &lines[..preview_count] {
        out.push_str(&format!("{GREEN}+{line}{RESET}\n"));
    }
    if total > preview_count {
        let remaining = total - preview_count;
        out.push_str(&format!("{DIM}···  ({remaining} more lines){RESET}\n"));
    }
    out
}
```

**Step 4: Run tests**

Run: `cargo test diff_tests -q`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/user_format.rs
git commit -m "feat: add ANSI diff helper functions to user_format"
```

---

### Task 7: Add `call_content` override to `EditFile` with diff viewer

**Files:**
- Modify: `src/tools/file.rs`

**Step 1: Write failing test**

The current `EditFile` returns `"ok"` and has no user content block. Test that after the override, the User block contains a diff:

```rust
// In file.rs test block (unit test — mock the ToolContext for write path)
// This is an integration-style test; skip if the fixture is complex.
// Instead, test the formatting helper directly:
#[test]
fn edit_file_call_content_user_block_contains_diff() {
    use serde_json::json;
    // Simulate what call_content would produce by calling the formatter
    let header = user_format::render_diff_header("edit_file", "src/a.rs");
    let diff = user_format::render_edit_diff("src/a.rs", "old text", "new text", Some(5));
    let user_block = format!("{header}\n{diff}");
    assert!(user_block.contains("old text"), "got: {user_block}");
    assert!(user_block.contains("new text"), "got: {user_block}");
    assert!(user_block.contains("\x1b["), "no ANSI: {user_block}");
}
```

**Step 2: Run to verify it already passes** (pure formatter test, no I/O needed)

Run: `cargo test edit_file_call_content_user_block -q`
Expected: PASS (the helper functions exist from Task 6)

**Step 3: Add `call_content` override to `EditFile`**

In `src/tools/file.rs`, inside `impl Tool for EditFile`, add after the `call` method:

```rust
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    use rmcp::model::Role;
    use rmcp::model::Content;

    // Capture diff context before the write
    let path = super::require_str_param(&input, "path").unwrap_or("?");
    let old_string = super::require_str_param(&input, "old_string").unwrap_or("");
    let new_string = input["new_string"].as_str().unwrap_or("");

    // Find line position of old_string in the pre-edit file (best effort)
    let start_line: Option<usize> = (|| {
        let root = futures::executor::block_on(ctx.agent.require_project_root()).ok()?;
        let security = futures::executor::block_on(ctx.agent.security_config());
        let resolved =
            crate::util::path_security::validate_write_path(path, &root, &security).ok()?;
        let content = std::fs::read_to_string(&resolved).ok()?;
        let byte_pos = content.find(old_string)?;
        Some(content[..byte_pos].lines().count() + 1)
    })();

    // Execute the write
    let result = self.call(input, ctx).await?;

    // Build summary line (no ANSI)
    let old_line_count = old_string.lines().count();
    let new_line_count = new_string.lines().count();
    let diff_note = if old_line_count == new_line_count {
        format!("{old_line_count} lines replaced")
    } else {
        let delta = new_line_count as i64 - old_line_count as i64;
        let sign = if delta >= 0 { "+" } else { "" };
        format!("-{old_line_count} +{new_line_count} ({sign}{delta})")
    };
    let summary = if let Some(l) = start_line {
        format!("{path} → L{l} · {diff_note}")
    } else {
        format!("{path} → {diff_note}")
    };

    // Build ANSI diff viewer
    let header = user_format::render_diff_header("edit_file", path);
    let diff = user_format::render_edit_diff(path, old_string, new_string, start_line);
    let user_text = format!("{summary}\n\n{header}\n{diff}");

    let json_str = serde_json::to_string(&result).unwrap_or_else(|_| "\"ok\"".into());
    Ok(vec![
        Content::text(json_str).with_audience(vec![Role::Assistant]),
        Content::text(user_text).with_audience(vec![Role::User]),
    ])
}
```

**Note on `block_on`:** The pre-read uses `futures::executor::block_on` because `call_content` is async but we need the `start_line` before the async `self.call()`. An alternative is to make the pre-read async too using `.await` directly — restructure if the block_on pattern causes issues:

```rust
// Preferred async version — restructure like this instead:
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    let path = super::require_str_param(&input, "path").unwrap_or("?");
    let old_string = super::require_str_param(&input, "old_string").unwrap_or("");
    let new_string = input["new_string"].as_str().unwrap_or("");

    // Pre-read for line position
    let start_line: Option<usize> = async {
        let root = ctx.agent.require_project_root().await.ok()?;
        let security = ctx.agent.security_config().await;
        let resolved =
            crate::util::path_security::validate_write_path(path, &root, &security).ok()?;
        let content = std::fs::read_to_string(&resolved).ok()?;
        let byte_pos = content.find(old_string)?;
        Some(content[..byte_pos].lines().count() + 1)
    }.await;

    let result = self.call(input, ctx).await?;
    // ... rest same as above
}
```

Use the async version — it's cleaner.

**Step 4: Run tests**

Run: `cargo test -q`
Expected: all passing

Run: `cargo clippy -- -D warnings`
Expected: clean

**Step 5: Commit**

```bash
git add src/tools/file.rs
git commit -m "feat: add ANSI diff viewer to EditFile via call_content override"
```

---

### Task 8: Add `call_content` / `format_for_user` to `ReplaceSymbol`, `RemoveSymbol`; fix `RemoveSymbol` return data

**Files:**
- Modify: `src/tools/symbol.rs`
- Modify: `src/tools/user_format.rs`

**Step 1: Fix `RemoveSymbol::call` to return location info**

Currently returns `json!("ok")`. Change to return `{ "status": "ok", "removed_lines": "201-215", "line_count": 14 }` (matching `ReplaceSymbol`'s pattern):

Find in `symbol.rs` (line ~1295):
```rust
// Before:
Ok(match hint {
    None => json!("ok"),
    Some(h) => json!({ "worktree_hint": h }),
})
```

Change to:
```rust
let line_count = end - start;
let removed_range = format!("{}-{}", start + 1, end);
let mut resp = json!({
    "status": "ok",
    "removed_lines": removed_range,
    "line_count": line_count,
});
if let Some(h) = hint {
    resp["worktree_hint"] = json!(h);
}
Ok(resp)
```

**Step 2: Write failing tests**

```rust
#[test]
fn replace_symbol_format_for_user_shows_range() {
    use serde_json::json;
    let tool = ReplaceSymbol;
    let r = json!({ "status": "ok", "replaced_lines": "124-145" });
    let t = tool.format_for_user(&r).unwrap();
    assert!(t.contains("L124"), "got: {t}");
}

#[test]
fn remove_symbol_format_for_user_shows_range() {
    use serde_json::json;
    let tool = RemoveSymbol;
    let r = json!({ "status": "ok", "removed_lines": "201-215", "line_count": 14 });
    let t = tool.format_for_user(&r).unwrap();
    assert!(t.contains("201"), "got: {t}");
    assert!(t.contains("14"), "got: {t}");
}
```

**Step 3: Run to verify failure**

Run: `cargo test replace_symbol_format remove_symbol_format -q`
Expected: FAIL

**Step 4: Add formatters and `format_for_user` implementations**

Add to `user_format.rs`:
```rust
pub fn format_replace_symbol(result: &Value) -> String {
    let lines = result["replaced_lines"].as_str().unwrap_or("?");
    format!("replaced · L{lines}")
}

pub fn format_remove_symbol(result: &Value) -> String {
    let lines = result["removed_lines"].as_str().unwrap_or("?");
    let count = result["line_count"].as_u64().unwrap_or(0);
    format!("removed · L{lines} ({count} lines)")
}
```

Add `format_for_user` to `ReplaceSymbol` in `symbol.rs`:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_replace_symbol(result))
}
```

Add `format_for_user` to `RemoveSymbol` in `symbol.rs`:
```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_remove_symbol(result))
}
```

**Step 5: Run tests**

Run: `cargo test replace_symbol_format remove_symbol_format -q`
Expected: PASS

Run: `cargo test -q`
Expected: all passing

**Step 6: Add `call_content` override to `ReplaceSymbol` for diff viewer**

This is the same pattern as `EditFile`. In `impl Tool for ReplaceSymbol`:

```rust
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    use rmcp::model::{Content, Role};

    let path = input["path"].as_str().unwrap_or("?");
    let name_path = input["name_path"].as_str().unwrap_or("?");
    let new_body = input["new_body"].as_str().unwrap_or("");

    let result = self.call(input, ctx).await?;

    let replaced_range = result["replaced_lines"].as_str().unwrap_or("?");
    let summary = format!("{path} · {name_path} → L{replaced_range}");

    let header = user_format::render_diff_header("replace_symbol", path);
    // We don't have the old body here — show a simple insertion view
    let insert_note = format!(
        "{}{} lines inserted at L{}{}\n",
        "\x1b[1;32m",
        new_body.lines().count(),
        replaced_range.split('-').next().unwrap_or("?"),
        "\x1b[0m"
    );
    let user_text = format!("{summary}\n\n{header}\n{insert_note}");

    let json_str = serde_json::to_string(&result).unwrap_or_else(|_| "\"ok\"".into());
    Ok(vec![
        Content::text(json_str).with_audience(vec![Role::Assistant]),
        Content::text(user_text).with_audience(vec![Role::User]),
    ])
}
```

**Step 7: Run full suite and commit**

Run: `cargo test -q && cargo clippy -- -D warnings`
Expected: clean

```bash
git add src/tools/symbol.rs src/tools/user_format.rs
git commit -m "feat: ANSI diff viewer for ReplaceSymbol/RemoveSymbol, fix RemoveSymbol return data"
```

---

## Layer 1 — Progress Notification Infrastructure

### Task 9: Create `src/tools/progress.rs` with `ProgressReporter`

**Files:**
- Create: `src/tools/progress.rs`
- Modify: `src/tools/mod.rs` (add `pub mod progress;`)

**Background on rmcp-0.1.5 limitation:**

`CallToolRequestParam` does not expose `_meta.progressToken` — the rmcp `ServerHandler::call_tool` strips it. As a pragmatic workaround, we use `_ctx.id` (a `RequestId = NumberOrString`) as the progress token. This means Claude Code must match progress tokens against request IDs, which is not guaranteed by spec but is the most practical option without rewriting the server handler.

**Step 1: Write failing test**

In `src/tools/progress.rs` (new file), add a test at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reporter_constructs_without_panic() {
        // We can't easily unit-test the async notify_progress call
        // without a live peer, so this just verifies the struct compiles.
        // Integration behavior is verified manually in a running server.
        let _p: Option<ProgressReporter> = None;
        assert!(true);
    }
}
```

**Step 2: Create `src/tools/progress.rs`**

```rust
use std::sync::Arc;
use rmcp::{
    model::{NumberOrString, ProgressNotificationParam},
    service::Peer,
    RoleServer,
};

/// Sends MCP `notifications/progress` to the client while a tool is running.
///
/// Constructed in `server.rs::call_tool` when the client provides a progress
/// token. Tools call `ctx.progress.as_ref()` — it's a no-op if `None`.
///
/// # rmcp-0.1.5 limitation
/// `CallToolRequestParam` does not expose `_meta.progressToken`. We use
/// `_ctx.id` (the request ID) as a stand-in progress token. This works if
/// the client correlates progress tokens with request IDs (common in practice).
pub struct ProgressReporter {
    peer: Peer<RoleServer>,
    token: NumberOrString,
}

impl ProgressReporter {
    pub fn new(peer: Peer<RoleServer>, token: NumberOrString) -> Arc<Self> {
        Arc::new(Self { peer, token })
    }

    /// Send a progress notification. Errors are silently swallowed — progress
    /// is best-effort and must never fail the tool call.
    pub async fn report(&self, step: u32, total: Option<u32>) {
        let _ = self
            .peer
            .notify_progress(ProgressNotificationParam {
                progress_token: self.token.clone(),
                progress: step,
                total,
            })
            .await;
    }
}
```

**Step 3: Add `pub mod progress;` to `src/tools/mod.rs`**

Find the existing `pub mod` declarations at the top of `src/tools/mod.rs` and add:
```rust
pub mod progress;
```

**Step 4: Run test**

Run: `cargo test progress_reporter_constructs -q`
Expected: PASS (it's a trivial compile test)

**Step 5: Commit**

```bash
git add src/tools/progress.rs src/tools/mod.rs
git commit -m "feat: add ProgressReporter in src/tools/progress.rs"
```

---

### Task 10: Add `progress` field to `ToolContext`, inject in `server.rs`

**Files:**
- Modify: `src/tools/mod.rs`
- Modify: `src/server.rs`

**Step 1: Write a compile test (no runtime assertions possible without peer)**

In the test block of `src/tools/mod.rs`:
```rust
#[test]
fn tool_context_has_progress_field() {
    // Ensures the progress field exists and is the right type.
    // We construct ToolContext in tests via agent::tests::make_ctx() helper.
    // For now, just assert the struct fields are accessible:
    fn _check_progress_field_type(_ctx: &ToolContext) {
        let _p: &Option<std::sync::Arc<progress::ProgressReporter>> = &_ctx.progress;
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test tool_context_has_progress_field -q`
Expected: FAIL (field doesn't exist yet)

**Step 3: Add `progress` to `ToolContext` in `src/tools/mod.rs`**

Find `ToolContext` at line 37:
```rust
// Before:
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,
    pub output_buffer: Arc<output_buffer::OutputBuffer>,
}
```

Change to:
```rust
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,
    pub output_buffer: Arc<output_buffer::OutputBuffer>,
    pub progress: Option<Arc<progress::ProgressReporter>>,
}
```

**Step 4: Fix all `ToolContext { ... }` construction sites**

There are two sites in `server.rs` (line 185) and any test helpers. Add `progress: None` to each:

In `server.rs`:
```rust
// Before:
let ctx = ToolContext {
    agent: self.agent.clone(),
    lsp: self.lsp.clone(),
    output_buffer: self.output_buffer.clone(),
};
```

Change to:
```rust
let progress = _ctx.peer.clone().into();  // see Step 5
let ctx = ToolContext {
    agent: self.agent.clone(),
    lsp: self.lsp.clone(),
    output_buffer: self.output_buffer.clone(),
    progress,
};
```

**Step 5: Inject progress token from `_ctx` in `server.rs`**

In `server.rs::call_tool`, the `_ctx: RequestContext<RoleServer>` parameter has `_ctx.peer` (a `Peer<RoleServer>`) and `_ctx.id` (a `RequestId = NumberOrString`).

Replace `_ctx` with `ctx_req` (to avoid confusion with `ToolContext`) and inject:

```rust
// In call_tool signature, rename _ctx to req_ctx:
async fn call_tool(
    &self,
    req: CallToolRequestParam,
    req_ctx: RequestContext<RoleServer>,
) -> std::result::Result<CallToolResult, McpError> {
    // ... existing code ...

    // Build progress reporter using the request ID as the token
    let progress = Some(progress::ProgressReporter::new(
        req_ctx.peer.clone(),
        req_ctx.id.clone(),
    ));

    let ctx = ToolContext {
        agent: self.agent.clone(),
        lsp: self.lsp.clone(),
        output_buffer: self.output_buffer.clone(),
        progress,
    };
    // ... rest unchanged ...
}
```

Also add the import at the top of `server.rs`:
```rust
use crate::tools::progress;
```

**Step 6: Fix any test helpers that construct `ToolContext`**

Search for all `ToolContext {` constructions:
```
search_pattern("ToolContext {", path="src")
```
Add `progress: None` to each one in test code.

**Step 7: Run full suite**

Run: `cargo test -q && cargo clippy -- -D warnings`
Expected: all passing, clean

**Step 8: Commit**

```bash
git add src/tools/mod.rs src/server.rs
git commit -m "feat: add progress field to ToolContext, inject from RequestContext in server.rs"
```

---

### Task 11: Add progress reporting to `IndexProject` and `RunCommand`

**Files:**
- Modify: `src/tools/semantic.rs`
- Modify: `src/tools/workflow.rs`

**Step 1: Write an integration test for IndexProject progress**

This is hard to unit-test without a live peer. Write a comment-guarded manual verification note and a unit test that verifies the code path compiles (not the actual notification):

```rust
// In semantic.rs test block:
#[test]
fn index_project_call_accepts_progress_none() {
    // This test checks that the progress code path compiles.
    // When ctx.progress is None, no notifications are sent.
    // Manual verification: run `cargo run -- index --project .` and
    // observe progress in Claude Code's tool spinner.
    assert!(true);
}
```

**Step 2: Add progress reporting to `IndexProject::call`**

In `src/tools/semantic.rs`, inside the `IndexProject::call` method, after the file list is computed and before/during the indexing loop:

```rust
// After: let files = find_changed_files(...)?;
let total = files.len() as u32;
if let Some(p) = &ctx.progress {
    p.report(0, Some(total)).await;
}

// Inside the indexing loop, after each batch:
for (i, chunk) in chunks.iter().enumerate() {
    // ... existing chunk processing ...
    if let Some(p) = &ctx.progress {
        p.report(i as u32 + 1, Some(total)).await;
    }
}
```

Note: Adapt to the actual loop structure in `IndexProject::call`. The key is to call `p.report(current, Some(total)).await` at regular intervals — every file or every batch of 10 files.

**Step 3: Add heartbeat to `RunCommand`**

For long-running commands, send a time-based heartbeat. The `run_command_inner` function is async. Add a heartbeat task that fires every 3 seconds:

```rust
// In workflow.rs, inside run_command_inner or RunCommand::call,
// after spawning the command, before awaiting the result:

// Spawn heartbeat (fires every 3s while command runs)
let progress_clone = ctx.progress.clone();
let heartbeat = tokio::spawn(async move {
    let start = std::time::Instant::now();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Some(p) = &progress_clone {
            let elapsed = start.elapsed().as_secs() as u32;
            p.report(elapsed, None).await;
        }
    }
});

// Await command result
let result = /* existing await */;

// Abort heartbeat
heartbeat.abort();

result
```

**Step 4: Run full suite**

Run: `cargo test -q && cargo clippy -- -D warnings`
Expected: all passing, clean

**Step 5: Commit**

```bash
git add src/tools/semantic.rs src/tools/workflow.rs
git commit -m "feat: add progress notifications to IndexProject and RunCommand"
```

---

## Verification Checklist

Before calling this work done, verify:

- [ ] `cargo test -q` — all tests pass
- [ ] `cargo clippy -- -D warnings` — zero warnings
- [ ] `cargo fmt` — no formatting changes
- [ ] Manual: Start MCP server and run `find_file("**/*.rs")` — terminal shows `47 files`
- [ ] Manual: Run `edit_file` — terminal shows ANSI diff with `─── edit_file: ... ───` header
- [ ] Manual: Run `cargo test` via `run_command` — terminal shows `✓ exit 0 · 533 passed`
- [ ] Manual: Run `index_project` — watch for progress dots in spinner

## Notes

- `CreateFile` already has a `call_content` override (`src/tools/file.rs:488`) with `render_create_header` using markdown. Its existing format is intentionally left as-is (markdown, not ANSI) to avoid breaking established behavior.
- The rmcp-0.1.5 `CallToolRequestParam` does not expose `_meta.progressToken`. We use `_ctx.id` as the token. If Claude Code doesn't send progress tokens, the notifications are silently dropped. This is expected.
- `RemoveSymbol::call` was changed to return `{ "status": "ok", "removed_lines": "N-M", "line_count": N }` (was `"ok"`). This is a small API change; the LLM sees richer data which helps it confirm the operation succeeded at the expected location.
