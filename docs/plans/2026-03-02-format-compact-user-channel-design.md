# Design: `format_compact` / `format_for_user_channel` Split

**Date:** 2026-03-02
**Status:** Approved

---

## Problem

`format_for_user` on the `Tool` trait was designed to serve two distinct purposes:

1. **Buffer compact summary** — shown inline alongside `@tool_*` refs when tool output
   exceeds 10 KB. The assistant sees this; it works today.
2. **User UI channel** — intended as the payload for the MCP `Role::User` audience block
   that Claude Code would display in its terminal/UI without polluting the LLM context.

Purpose 2 never worked. Claude Code does not filter by audience (issue #13600), and the
`notifications/message` channel is not displayed (issue #3174). As a workaround,
`USER_OUTPUT_ENABLED = false` in `src/server.rs` strips `Role::User` blocks before they
leave the server. This means `format_for_user` in the small-output path generates text
that is computed and then immediately thrown away.

Additionally, mixing the two purposes in one method name and one method body makes the
`call_content` logic confusing and the intent unclear.

---

## Goal

- Eliminate dead code in the non-buffer path of `call_content`.
- Give the two purposes distinct, clearly-named method hooks.
- Preserve all existing formatting work so it is ready when MCP user channels land.
- Remove `USER_OUTPUT_ENABLED` — it suppresses blocks we will no longer generate.

---

## Design

### Two methods on the `Tool` trait

```rust
/// Compact plain-text summary of this tool's result.
///
/// Used in the buffer path of `call_content`: when serialized output exceeds
/// `TOOL_OUTPUT_BUFFER_THRESHOLD` (10 KB), this text is shown inline alongside
/// the `@tool_*` ref handle so the assistant has a terse overview without
/// loading the full JSON into context.
///
/// Return `None` to use the generic fallback:
/// `"Result stored in @tool_xxx (N bytes)"`.
fn format_compact(&self, _result: &Value) -> Option<String> {
    None
}

/// Human-readable display text for the MCP user-facing channel.
///
/// Defaults to `format_compact()`. Override only if the user-facing display
/// should be richer or different from the buffer summary.
///
/// Not currently called — both potential delivery mechanisms have open issues:
///   - Audience filtering: Claude Code issue #13600
///   - notifications/message display: Claude Code issue #3174
///
/// When either issue ships, wire this method in `call_content` at the
/// `TODO(#13600/#3174)` comment.
fn format_for_user_channel(&self, result: &Value) -> Option<String> {
    self.format_compact(result)
}
```

### `call_content` — simplified

```rust
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    let val = self.call(input, ctx).await?;
    let json = serde_json::to_string(&val).unwrap_or_else(|_| val.to_string());

    // Buffer path: output is large — store in OutputBuffer, return compact summary + ref.
    if json.len() > TOOL_OUTPUT_BUFFER_THRESHOLD {
        let json_len = json.len();
        let ref_id = ctx.output_buffer.store_tool(self.name(), json);
        let summary = self
            .format_compact(&val)
            .unwrap_or_else(|| format!("Result stored in {} ({} bytes)", ref_id, json_len));
        return Ok(vec![Content::text(format!("{}\nFull result: {}", summary, ref_id))]);
    }

    // Small output — return pretty JSON to the assistant.
    // TODO(#13600/#3174): emit self.format_for_user_channel(&val) to user channel here.
    Ok(vec![Content::text(
        serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string()),
    )])
}
```

**Key changes vs today:**
- The `match self.format_for_user(&val)` in the non-buffer path is gone.
- No more `Role::Assistant` / `Role::User` audience blocks from `call_content`.
- Small output always returns a single no-audience pretty-JSON block.

### Remove `USER_OUTPUT_ENABLED`

`src/server.rs` — delete the flag and the `blocks.retain(...)` filter. There are no
`Role::User` blocks to strip anymore. The future wiring will be in `call_content`, not
in the server dispatch layer.

### Migration — all existing `format_for_user` implementations

All ~30 tools that override `fn format_for_user` → rename to `fn format_compact`.
Content of the implementations is unchanged — they continue to produce the same terse
text that was always used as the buffer summary.

Tools may optionally add `fn format_for_user_channel` overrides later for richer display
when the channel exists. No tool is required to do this.

---

## Files Changed

| File | Change |
|---|---|
| `src/tools/mod.rs` | Rename `format_for_user` → `format_compact`; add `format_for_user_channel`; simplify `call_content`; remove `Role` import |
| `src/tools/user_format.rs` | No logic change (formatting functions stay as-is) |
| `src/tools/file.rs` | Rename `format_for_user` → `format_compact` (3 tools) |
| `src/tools/symbol.rs` | Rename `format_for_user` → `format_compact` (10 tools) |
| `src/tools/semantic.rs` | Rename (3 tools) |
| `src/tools/workflow.rs` | Rename (2 tools) |
| `src/tools/git.rs` | Rename (1 tool) |
| `src/tools/library.rs` | Rename (2 tools) |
| `src/tools/memory.rs` | Rename (2 tools) |
| `src/tools/ast.rs` | Rename (2 tools) |
| `src/tools/config.rs` | Rename (2 tools) |
| `src/tools/usage.rs` | Rename (1 tool) |
| `src/server.rs` | Remove `USER_OUTPUT_ENABLED`, `blocks.retain(...)`, and `Role` import |

---

## Testing

TDD is mandatory. Before touching any implementation:

1. **Add a failing test** in `src/tools/mod.rs` for the gap case:
   `call_content` with small output + `format_compact` returning `Some` → must return
   exactly 1 block containing pretty JSON (not the compact text, not 2 blocks).

2. **Verify existing `call_content` tests still pass** after the rename:
   - `call_content_passthrough_small_output`
   - `call_content_buffers_large_output`
   - `call_content_uses_format_for_user_in_compact_text` → rename test to
     `call_content_uses_format_compact_in_buffer_summary`
   - `call_content_generic_fallback_without_format_for_user` → rename accordingly

3. **`cargo clippy` catches dead-code** on `USER_OUTPUT_ENABLED` after removal — confirm
   clean compile with no unused import warnings for `Role`.

---

## Non-Goals

- No changes to `user_format.rs` formatting logic.
- No wiring of `format_for_user_channel` to any delivery mechanism (that waits for #13600/#3174).
- No per-tool changes beyond the mechanical rename.
