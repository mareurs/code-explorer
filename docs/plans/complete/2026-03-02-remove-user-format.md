# Remove user_format Module Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Delete `src/tools/user_format.rs` by collapsing its formatting functions into their
respective tool files and creating a minimal shared `src/tools/format.rs` for helpers used
across multiple files.

**Architecture:** Pure refactor — no behavior change. Each task moves code and runs `cargo test`
to confirm nothing broke. The dual-audience `call_content` overrides are removed first (they're
the only live callers of the `render_*_diff` functions). Then the format functions migrate
file-by-file. A new `src/tools/format.rs` holds the three helpers used across multiple
destination files (`truncate_path`, `format_line_range`, `format_overflow`).

**Tech Stack:** Rust, cargo test, no new dependencies.

---

## Destination Map

Before starting, internalize where each function lands:

| Source (`user_format.rs`) | Destination |
|---|---|
| `format_read_file`, `format_list_dir`, `format_search_pattern`, `format_find_file` | `file.rs` |
| `format_list_symbols`, `format_find_symbol`, `format_find_references`, `format_goto_definition`, `format_hover`, `format_replace_symbol`, `format_remove_symbol`, `format_insert_code`, `format_rename_symbol` | `symbol.rs` |
| `format_git_blame` | `git.rs` |
| `format_list_functions`, `format_list_docs` | `ast.rs` |
| `format_semantic_search`, `format_index_project`, `format_index_status`, `format_index_library` | `semantic.rs` |
| `format_list_libraries` | `library.rs` |
| `format_get_config`, `format_activate_project` | `config.rs` |
| `format_read_memory`, `format_list_memories` | `memory.rs` |
| `format_get_usage_stats` | `usage.rs` |
| `format_onboarding`, `format_run_command` | `workflow.rs` |
| `truncate_path`, `format_line_range`, `format_overflow` | new `src/tools/format.rs` |
| `common_path_prefix`, `format_read_file_summary`, `format_search_simple_mode`, `format_search_context_mode` | private in `file.rs` |
| `format_symbol_tree` | private in `symbol.rs` |
| `render_diff_header`, `render_edit_diff`, `render_removal_diff`, `render_insert_diff`, `format_create_file` | **deleted** |

---

### Task 1: Remove dual-audience machinery

Remove the 5 `call_content` overrides, the `render_*` diff functions they depend on,
the server-side `Role::User` filter, and the audience-split tests.

**Files:**
- Modify: `src/tools/symbol.rs` (4 `call_content` overrides)
- Modify: `src/tools/file.rs` (1 `call_content` override + audience tests)
- Modify: `src/server.rs` (`blocks.retain` + `Role` import)
- Modify: `src/tools/user_format.rs` (delete `render_diff_header` + 3 `render_*_diff` fns + `format_create_file`)

**Step 1: Confirm baseline**

```bash
cargo test 2>&1 | grep "test result"
```
Expected: all passing.

**Step 2: Remove the 4 call_content overrides from symbol.rs**

In `src/tools/symbol.rs`, find and delete the `async fn call_content` impl blocks for:
`ReplaceSymbol` (around L1286), `RemoveSymbol` (~L1405), `InsertCode` (~L1519),
`RenameSymbol` (~L1907). Also remove the `use rmcp::model::Role;` import each uses.

Each override looks like:
```rust
async fn call_content(
    &self,
    input: Value,
    ctx: &ToolContext,
) -> anyhow::Result<Vec<rmcp::model::Content>> {
    use rmcp::model::Role;
    // ... builds user_text, json_str ...
    Ok(vec![
        rmcp::model::Content::text(json_str).with_audience(vec![Role::Assistant]),
        rmcp::model::Content::text(user_text).with_audience(vec![Role::User]),
    ])
}
```

Also remove the calls to `user_format::render_diff_header(...)` that are inside those overrides.

**Step 3: Remove the call_content override from file.rs**

In `src/tools/file.rs`, delete the `async fn call_content` impl block for `CreateFile`
(around L765). It builds a ANSI preview and returns two audience-split blocks.

**Step 4: Remove audience-split tests from file.rs**

In `src/tools/file.rs`, delete the test functions:
- `create_file_call_content_returns_two_audience_blocks` (around L2252)
- `edit_file_call_content_shows_path_no_inline_diff` (around L2511) — or any test
  asserting `Role::User` audience

**Step 5: Remove Role::User filter from server.rs**

In `src/server.rs`, revert `call_tool_inner` to use `CallToolResult::success(blocks)` directly:

```rust
let call_result = match result {
    Ok(blocks) => CallToolResult::success(blocks),
    Err(e) => route_tool_error(e),
};
```

Remove the `Role` import from the `use rmcp::model::{...}` block at the top of the file.

**Step 6: Delete render functions from user_format.rs**

In `src/tools/user_format.rs`, delete:
- `pub fn render_diff_header(...)` (~L1274)
- `pub fn render_edit_diff(...)` (~L1282)
- `pub fn render_removal_diff(...)` (~L1308)
- `pub fn render_insert_diff(...)` (~L1334)
- `pub fn format_create_file(...)` (~L2779)

Also delete their tests (search for `render_diff_header_contains_path`,
`render_edit_diff_shows_minus_plus_lines`, `render_removal_diff_marks_all_lines_red`,
`render_insert_diff_marks_all_lines_green` in the `#[cfg(test)]` block).

**Step 7: Run tests**

```bash
cargo test 2>&1 | grep -E "FAILED|test result"
```
Expected: all passing.

**Step 8: Commit**

```bash
git add src/tools/symbol.rs src/tools/file.rs src/server.rs src/tools/user_format.rs
git commit -m "refactor: remove Role::User dual-audience infrastructure"
```

---

### Task 2: Create src/tools/format.rs with shared helpers

`truncate_path`, `format_line_range`, and `format_overflow` are used by format functions
that will land in multiple different tool files. They need a shared home first so subsequent
tasks can update imports cleanly.

**Files:**
- Create: `src/tools/format.rs`
- Modify: `src/tools/mod.rs` (add `pub(crate) mod format;`)
- Modify: `src/tools/user_format.rs` (remove the 3 functions + update internal callers)

**Step 1: Create src/tools/format.rs**

Copy the three functions from `user_format.rs` into the new file.
Also move their unit tests. The file should look like:

```rust
//! Shared formatting helpers used by multiple tool format_compact implementations.

use serde_json::Value;

/// Formats a line range as "L35" (single) or "L35-50" (multi).
pub(crate) fn format_line_range(start: u64, end: u64) -> String {
    // ... copy body from user_format.rs
}

/// Truncates a path to max_len chars, replacing the middle with "…".
pub(crate) fn truncate_path(path: &str, max_len: usize) -> String {
    // ... copy body from user_format.rs
}

/// Formats an overflow object into a hint string like "(+23 more — narrow with path=)".
pub(crate) fn format_overflow(overflow: &Value) -> String {
    // ... copy body from user_format.rs
}

#[cfg(test)]
mod tests {
    use super::*;
    // ... copy the tests for these three functions from user_format.rs
}
```

**Step 2: Register the module in mod.rs**

In `src/tools/mod.rs`, add:
```rust
pub(crate) mod format;
```

**Step 3: Update user_format.rs to use the new module**

In `src/tools/user_format.rs`:
1. Add at the top: `use super::format::{format_line_range, truncate_path, format_overflow};`
2. Delete the three function definitions and their tests.
3. Update any call sites within `user_format.rs` that called these functions directly —
   they now resolve via the `use` statement, so no call-site changes needed.

**Step 4: Run tests**

```bash
cargo test 2>&1 | grep -E "FAILED|test result"
```
Expected: all passing.

**Step 5: Commit**

```bash
git add src/tools/format.rs src/tools/mod.rs src/tools/user_format.rs
git commit -m "refactor: extract shared format helpers to src/tools/format.rs"
```

---

### Task 3: Move file.rs format functions

Move `format_read_file`, `format_list_dir`, `format_search_pattern`, `format_find_file`
plus their private helpers into `file.rs`.

**Files:**
- Modify: `src/tools/file.rs`
- Modify: `src/tools/user_format.rs`

**Step 1: Add import to file.rs**

At the top of `src/tools/file.rs` (with other `use` statements):
```rust
use super::format::{format_line_range, format_overflow, truncate_path};
```

**Step 2: Move functions into file.rs**

Copy these from `user_format.rs` to the bottom of `file.rs` (before `#[cfg(test)]`):
- `pub fn format_read_file(val: &Value) -> String` + its private helper
  `fn format_read_file_summary(val: &Value, file_type: &str) -> String`
- `pub fn format_list_dir(val: &Value) -> String` + its private helper
  `fn common_path_prefix(paths: &[&str]) -> String`
- `pub fn format_search_pattern(val: &Value) -> String` + its private helpers
  `fn format_search_simple_mode(...)` and `fn format_search_context_mode(...)`
- `pub fn format_find_file(result: &Value) -> String`

Change visibility from `pub` to `pub(super)` or `fn` (these are only called from within
`file.rs` via `format_compact`). Actually, just `fn` is fine — private to the module.

Also move their unit tests into `file.rs`'s `#[cfg(test)]` block.

**Step 3: Update format_compact callers in file.rs**

The four `format_compact` impls already call `user_format::format_xxx(result)`.
Change each to just `format_xxx(result)` (now local):
```rust
fn format_compact(&self, result: &Value) -> Option<String> {
    Some(format_read_file(result))
}
```

**Step 4: Remove from user_format.rs**

Delete from `user_format.rs`:
- `format_read_file` + `format_read_file_summary`
- `format_list_dir` + `common_path_prefix`
- `format_search_pattern` + `format_search_simple_mode` + `format_search_context_mode`
- `format_find_file`
- Their tests

Also remove the `use super::user_format;` import from `file.rs` if it's no longer needed
(or narrow it to only the symbols still used — at this point file.rs uses no more
`user_format::` functions).

**Step 5: Run tests**

```bash
cargo test 2>&1 | grep -E "FAILED|test result"
```
Expected: all passing.

**Step 6: Commit**

```bash
git add src/tools/file.rs src/tools/user_format.rs
git commit -m "refactor: move file.rs format functions out of user_format"
```

---

### Task 4: Move symbol.rs format functions

Move 9 format functions + `format_symbol_tree` private helper into `symbol.rs`.

**Files:**
- Modify: `src/tools/symbol.rs`
- Modify: `src/tools/user_format.rs`

**Step 1: Add import to symbol.rs**

```rust
use super::format::{format_line_range, format_overflow, truncate_path};
```

**Step 2: Move functions into symbol.rs**

Copy from `user_format.rs` to the bottom of `symbol.rs` (before `#[cfg(test)]`):
- `format_goto_definition`, `format_hover`
- `format_list_symbols` + private helper `format_symbol_tree`
- `format_find_symbol`, `format_find_references`
- `format_replace_symbol`, `format_remove_symbol`, `format_insert_code`, `format_rename_symbol`

Change `pub fn` to `fn` (private to module). Move their tests too.

**Step 3: Update format_compact callers in symbol.rs**

Change all `user_format::format_xxx(result)` calls in `format_compact` impls to
just `format_xxx(result)`.

**Step 4: Remove from user_format.rs**

Delete the 9 functions + `format_symbol_tree` + their tests from `user_format.rs`.

**Step 5: Run tests**

```bash
cargo test 2>&1 | grep -E "FAILED|test result"
```
Expected: all passing.

**Step 6: Commit**

```bash
git add src/tools/symbol.rs src/tools/user_format.rs
git commit -m "refactor: move symbol.rs format functions out of user_format"
```

---

### Task 5: Move git.rs and ast.rs format functions

**Files:**
- Modify: `src/tools/git.rs`, `src/tools/ast.rs`
- Modify: `src/tools/user_format.rs`

**Step 1: Move format_git_blame into git.rs**

Add import to `git.rs`:
```rust
use super::format::format_line_range;
```

Copy `format_git_blame` (and any private helpers it uses) from `user_format.rs` to the
bottom of `git.rs`. Change `pub fn` → `fn`. Move tests.

Update `format_compact` in `git.rs` to call `format_git_blame(result)` directly.

**Step 2: Move format_list_functions and format_list_docs into ast.rs**

Copy both from `user_format.rs` to `ast.rs`. Check if they use any shared helpers
(add the import if needed). Change to private `fn`. Move tests.

Update `format_compact` in `ast.rs`.

**Step 3: Remove from user_format.rs**

Delete `format_git_blame`, `format_list_functions`, `format_list_docs` + their tests.

**Step 4: Run tests**

```bash
cargo test 2>&1 | grep -E "FAILED|test result"
```
Expected: all passing.

**Step 5: Commit**

```bash
git add src/tools/git.rs src/tools/ast.rs src/tools/user_format.rs
git commit -m "refactor: move git.rs and ast.rs format functions out of user_format"
```

---

### Task 6: Move semantic.rs and library.rs format functions

**Files:**
- Modify: `src/tools/semantic.rs`, `src/tools/library.rs`
- Modify: `src/tools/user_format.rs`

**Step 1: Move into semantic.rs**

Functions: `format_semantic_search`, `format_index_project`, `format_index_status`,
`format_index_library` (check which of these is actually called from semantic.rs vs library.rs
— see destination map). Add shared helper imports as needed. Move tests.

**Step 2: Move into library.rs**

Functions: `format_list_libraries`, `format_index_library` (whichever belongs here per
the destination map). Add imports. Move tests.

**Step 3: Remove from user_format.rs. Run tests. Commit.**

```bash
git add src/tools/semantic.rs src/tools/library.rs src/tools/user_format.rs
git commit -m "refactor: move semantic.rs and library.rs format functions out of user_format"
```

---

### Task 7: Move remaining tool format functions

**Files:**
- Modify: `src/tools/workflow.rs`, `src/tools/memory.rs`, `src/tools/config.rs`, `src/tools/usage.rs`
- Modify: `src/tools/user_format.rs`

**Step 1: workflow.rs** — move `format_onboarding`, `format_run_command`.
**Step 2: memory.rs** — move `format_read_memory`, `format_list_memories`.
**Step 3: config.rs** — move `format_get_config`, `format_activate_project`.
**Step 4: usage.rs** — move `format_get_usage_stats`.

For each: add shared helper imports, change to private `fn`, update `format_compact` callers,
move tests, delete from `user_format.rs`.

**Step 5: Run tests**

```bash
cargo test 2>&1 | grep -E "FAILED|test result"
```
Expected: all passing.

**Step 6: Commit**

```bash
git add src/tools/workflow.rs src/tools/memory.rs src/tools/config.rs src/tools/usage.rs src/tools/user_format.rs
git commit -m "refactor: move workflow/memory/config/usage format functions out of user_format"
```

---

### Task 8: Delete user_format.rs

By now `user_format.rs` should be empty of format functions. Verify, then delete.

**Step 1: Confirm user_format.rs is empty of live functions**

```bash
cargo build 2>&1 | grep "dead_code\|unused\|warning"
```

If there are any remaining `pub fn` in user_format.rs, track them down and move them
per the destination map before proceeding.

**Step 2: Remove mod declaration from tools/mod.rs**

In `src/tools/mod.rs`, delete the line:
```rust
pub(crate) mod user_format;
```
(or however it is declared — search for `user_format` in mod.rs)

**Step 3: Remove the file**

```bash
rm src/tools/user_format.rs
```

**Step 4: Run build + tests**

```bash
cargo build 2>&1 | grep "error"
cargo test 2>&1 | grep -E "FAILED|test result"
```
Expected: clean build, all tests passing.

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: delete user_format.rs — formatting logic now lives in tool files"
```

---

### Task 9: Update project memory

**Step 1: Update mcp-user-output-channels memory**

Use `write_memory("mcp-user-output-channels", ...)` to update the memory reflecting that:
- The dual-audience infrastructure has been removed
- `user_format.rs` no longer exists
- Format functions live in their respective tool files
- `src/tools/format.rs` contains shared helpers
- The path forward when Claude Code fixes #13600/#3174 is to add a `format_for_user` method
  to the `Tool` trait

**Step 2: Run final verification**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```
Expected: clean.

**Step 3: Final commit**

```bash
git add .
git commit -m "docs: update mcp-user-output-channels memory after user_format removal"
```
