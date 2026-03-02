# format_compact / format_for_user_channel Split — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rename `format_for_user` → `format_compact` on the `Tool` trait, add a no-op `format_for_user_channel` default, collapse the dead dual-audience branch in `call_content`, and remove `USER_OUTPUT_ENABLED` from the server.

**Architecture:** Three focused changes — (1) trait + `call_content` in `src/tools/mod.rs`, (2) LSP rename cascades to all 30 tool override sites automatically, (3) dead flag removal in `src/server.rs`. TDD: write the failing test first, then implement.

**Tech Stack:** Rust, `async_trait`, `rmcp::model::Content`, `serde_json`

**Design doc:** `docs/plans/2026-03-02-format-compact-user-channel-design.md`

---

### Task 1: Add failing test for the gap case

The gap: `call_content` with *small* output and `format_compact` returning `Some` currently
returns 2 blocks (compact JSON + user text). The desired behaviour is 1 block (pretty JSON).
This test will FAIL until Task 2 is done.

**Files:**
- Modify: `src/tools/mod.rs` (inside the `#[cfg(test)]` block, after the existing `call_content_passthrough_small_output` test)

**Step 1: Write the failing test**

Add this test after `call_content_passthrough_small_output` (around line 345):

```rust
#[tokio::test]
async fn call_content_small_output_ignores_format_compact() {
    // Even when format_compact returns Some, call_content must return exactly
    // 1 block with pretty JSON — the compact text is NOT injected into small outputs.
    let ctx = bare_ctx().await;
    let result = serde_json::json!({"key": "value"});
    let tool = EchoTool {
        result: result.clone(),
        user_summary: Some("compact summary".to_string()),
    };
    let content = tool
        .call_content(serde_json::json!({}), &ctx)
        .await
        .unwrap();
    assert_eq!(content.len(), 1, "small output must produce exactly 1 block, got: {:?}", content);
    let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
    assert!(text.contains("key"), "block must contain the JSON key, got: {}", text);
    assert!(
        !text.contains("compact summary"),
        "compact summary must NOT appear in small-output block, got: {}",
        text
    );
}
```

**Step 2: Run the test to verify it fails**

```bash
cargo test call_content_small_output_ignores_format_compact 2>&1 | tail -20
```

Expected: FAIL — `assertion failed: content.len() == 1` (currently returns 2 blocks).

---

### Task 2: Update the Tool trait and `call_content`

All changes are in `src/tools/mod.rs`. Do them in this order to keep the file compiling
at each step.

**Files:**
- Modify: `src/tools/mod.rs`

**Step 1: Rename the trait method (LSP rename — cascades to all 30 override sites)**

Use the `rename_symbol` MCP tool:
```
rename_symbol(
  path="src/tools/mod.rs",
  name_path="Tool/format_for_user",
  new_name="format_compact"
)
```

This renames every `fn format_for_user` in every `impl Tool for …` block across the
codebase — including the `EchoTool` in the tests. Verify the rename report shows ~31
occurrences (1 trait default + 30 overrides).

After: check the sweep output for any textual occurrences in comments/strings that LSP
missed and fix them manually.

**Step 2: Add `format_for_user_channel` default on the trait**

In `src/tools/mod.rs`, find the `format_compact` default method you just renamed and add
`format_for_user_channel` directly after it:

```rust
/// Compact plain-text summary used in the buffer path alongside `@tool_*` refs.
/// Return `None` for the generic "Result stored in @tool_xxx (N bytes)" fallback.
fn format_compact(&self, _result: &Value) -> Option<String> {
    None
}

/// Human-readable display text for the MCP user-facing channel.
///
/// Defaults to `format_compact()`. Override for richer display when the user
/// channel differs from the buffer summary.
///
/// Not yet called — wire in `call_content` at the TODO comment when either
/// Claude Code issue #13600 (audience filtering) or #3174 (notifications/message)
/// ships.
fn format_for_user_channel(&self, result: &Value) -> Option<String> {
    self.format_compact(result)
}
```

**Step 3: Simplify `call_content` — remove the dead dual-audience branch**

Replace the non-buffer path at the end of `call_content` (currently the `match
self.format_compact(&val)` block) with:

```rust
// Small output — return pretty JSON to the assistant.
// TODO(#13600/#3174): emit self.format_for_user_channel(&val) to user channel here.
Ok(vec![Content::text(
    serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string()),
)])
```

The entire `match self.format_compact(&val) { Some(...) => ..., None => ... }` block
is replaced by the single `Ok(...)` above.

**Step 4: Remove the `Role` import from `src/tools/mod.rs`**

Line 27 currently reads:
```rust
use rmcp::model::{Content, Role};
```

Change to:
```rust
use rmcp::model::Content;
```

(`Role` is no longer referenced anywhere in `mod.rs` after Step 3.)

**Step 5: Rename the two affected tests**

In the `#[cfg(test)]` block, rename:
- `call_content_uses_format_for_user_in_compact_text`
  → `call_content_uses_format_compact_in_buffer_summary`
- `call_content_generic_fallback_without_format_for_user`
  → `call_content_generic_fallback_without_format_compact`

Also update the comment inside the second test from:
```
// No format_for_user → generic fallback message with byte count and ref
```
to:
```
// No format_compact → generic fallback message with byte count and ref
```

**Step 6: Run all call_content tests**

```bash
cargo test call_content 2>&1 | tail -30
```

Expected: all 5 pass — including the new `call_content_small_output_ignores_format_compact`.

**Step 7: Full build + lint**

```bash
cargo build 2>&1 | tail -20
cargo clippy -- -D warnings 2>&1 | tail -20
```

Expected: clean — no unused import warnings, no dead-code warnings.

**Step 8: Commit**

```bash
git add src/tools/mod.rs src/tools/file.rs src/tools/symbol.rs src/tools/semantic.rs \
        src/tools/workflow.rs src/tools/git.rs src/tools/library.rs src/tools/memory.rs \
        src/tools/ast.rs src/tools/config.rs src/tools/usage.rs
git commit -m "refactor(tools): rename format_for_user → format_compact, add format_for_user_channel"
```

---

### Task 3: Remove `USER_OUTPUT_ENABLED` from `src/server.rs`

**Files:**
- Modify: `src/server.rs`

**Step 1: Delete the flag and filter**

Find and delete these lines in `src/server.rs`:

```rust
/// When false, user-audience content blocks are stripped before sending to the
/// MCP client. Flip to `true` once Claude Code implements proper audience
/// filtering (i.e. LLM context no longer receives Role::User-only blocks).
const USER_OUTPUT_ENABLED: bool = false;
```

And in the `call_tool` dispatch, replace:

```rust
Ok(mut blocks) => {
    if !USER_OUTPUT_ENABLED {
        blocks.retain(|b| b.audience() != Some(&vec![Role::User]));
    }
    Ok(CallToolResult::success(blocks))
}
```

with:

```rust
Ok(blocks) => Ok(CallToolResult::success(blocks)),
```

**Step 2: Remove `Role` from the server import**

Find the `use rmcp::model::{...}` line in `src/server.rs` and remove `Role` from it.
The exact line will look something like:
```rust
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, ListToolsResult, PaginatedRequestParam,
    Role, ...
};
```
Remove `Role,` (and trailing comma if needed for formatting).

**Step 3: Build + clippy**

```bash
cargo build 2>&1 | tail -20
cargo clippy -- -D warnings 2>&1 | tail -20
```

Expected: clean.

**Step 4: Full test suite**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass (860+).

**Step 5: Format**

```bash
cargo fmt
```

**Step 6: Commit**

```bash
git add src/server.rs
git commit -m "refactor(server): remove USER_OUTPUT_ENABLED — no Role::User blocks generated anymore"
```

---

### Task 4: Final verification

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

Expected: formatted, lint-clean, all tests green.

Check that `format_for_user` no longer appears anywhere in `src/`:

```bash
grep -r "format_for_user" src/
```

Expected: no output (the name is gone — only `format_compact` and `format_for_user_channel` remain).
