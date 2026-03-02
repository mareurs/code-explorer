# Design: CWD Path Stripping for Token Efficiency

**Date:** 2026-03-02  
**Status:** Approved

## Problem

Every tool that touches a file emits absolute paths in its output:

```json
{"file": "/home/user/work/myproject/src/tools/mod.rs", "line": 42}
```

The prefix `/home/user/work/myproject/` is identical in every response and carries
zero information — agents always operate within the project root. It wastes tokens
on every tool call that returns file locations (find_symbol, search_pattern,
list_symbols, find_file, semantic_search, git_blame, goto_definition, etc.).

## Goal

Strip the absolute project root from all MCP tool output so agents see:

```json
{"file": "src/tools/mod.rs", "line": 42}
```

## Approach

### Single intercept point: `call_tool` in `src/server.rs`

All tool output — success, recoverable error, and fatal error — flows through
`call_tool` before being returned over MCP. Strip the project root from the
assembled `CallToolResult` content blocks at that single point.

```rust
// Assemble result — success or error both produce a CallToolResult.
let call_result = match result {
    Ok(mut blocks) => {
        if !USER_OUTPUT_ENABLED {
            blocks.retain(|b| b.audience() != Some(&vec![Role::User]));
        }
        CallToolResult::success(blocks)
    }
    Err(e) => route_tool_error(e),
};

// Strip the absolute project root from all output to reduce token usage.
// Agents work exclusively within the project directory; relative paths carry
// all necessary information. The full root (e.g. /home/user/work/project) is
// a long repeated token-waster that appears in every "file" field and error
// message. Buffer content (@tool_xxx refs) is covered here too: it only
// re-enters the pipeline through run_command, which also passes through
// call_tool.
Ok(strip_project_root_from_result(call_result, &root_prefix))
```

### Why this covers everything

| Output type | Path |
|---|---|
| Small tool result (pretty-printed JSON) | `call_content` → `call_tool` strip ✓ |
| Large tool result summary (`format_compact`) | `call_content` → `call_tool` strip ✓ |
| Error messages (`route_tool_error`) | assembled before strip ✓ |
| Buffer queries (`run_command("grep @ref")`) | `run_command` → `call_tool` strip ✓ |

`OutputBuffer` stored content is never returned directly — it only re-enters the
pipeline when an agent queries a `@ref` handle via `run_command`. That `run_command`
call passes through `call_tool`, so the grep/tail/sed output is stripped there.
No changes needed to `OutputBuffer`.

### Implementation

**New helper function** (in `server.rs` or a small `util` module):

```rust
fn strip_project_root_from_result(
    mut result: CallToolResult,
    root_prefix: &str, // "{project_root}/" — trailing slash included
) -> CallToolResult {
    // Walk content blocks and replace the absolute prefix in text blocks.
    // This is a simple string replace; no JSON parsing needed because the
    // replacement ("src/foo.rs") is valid wherever the original ("/abs/src/foo.rs")
    // appeared — in JSON string values, error messages, and plain text alike.
    for block in &mut result.content {
        if let Content::Text(ref mut t) = block {
            t.text = t.text.replace(root_prefix, "");
        }
    }
    result
}
```

**Getting the root prefix** in `call_tool`:

```rust
let root_prefix = self
    .agent
    .project_root()
    .await
    .map(|p| format!("{}/", p.display()))
    .unwrap_or_default();
```

When no project is active (no root set), `root_prefix` is empty and the replace
is a no-op — no guard needed.

## What does NOT need changing

- `OutputBuffer` — buffer content is never returned directly to agents
- Individual tool implementations — no per-tool changes needed
- `format_compact` / `format_for_user_channel` implementations
- `route_tool_error` — its output arrives at `call_tool` before stripping

## Trade-offs

**Pro:** One location, full coverage, zero per-tool maintenance burden.

**Con:** String replace on serialized output is "blind" — it doesn't parse JSON
structure. In practice this is harmless: the project root is a long unique path
that would only appear as a file reference, never as coincidental content.

**Edge case — `activate_project`:** The root is read dynamically from
`self.agent.project_root().await` at each `call_tool` invocation, so switching
projects mid-session works correctly.

## Testing

- Unit test for `strip_project_root_from_result`: verify prefix stripped from
  text content blocks, non-matching strings unchanged, empty prefix is no-op.
- Integration test: call any file-emitting tool (e.g. `find_file`) and assert
  the returned paths do not contain the absolute project root.
