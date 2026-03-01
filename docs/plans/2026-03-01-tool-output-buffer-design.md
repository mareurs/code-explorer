# Tool Output Auto-Buffering Design

**Date:** 2026-03-01  
**Status:** Approved, ready for implementation

## Goal

Route all large tool outputs through `OutputBuffer` automatically, so agents receive a compact summary + `@tool_xxx` ref handle instead of a raw JSON wall. Addresses token budget blow-out from tools like `list_symbols` returning 13k+ tokens.

## Architecture

Three files change; zero per-tool changes required (overrides are optional for better summaries):

| File | Change |
|------|--------|
| `src/tools/mod.rs` | Add auto-buffering to `call_content()` default impl |
| `src/tools/output_buffer.rs` | Add `store_tool()` method; update `resolve_refs` regex |
| `src/prompts/server_instructions.md` | Document `@tool_xxx` refs for agents |

`ReadFile` is excluded — it already overrides `call_content` with its own buffering.  
`run_command` is unaffected — its responses are already compact.

**Threshold:** 10,000 bytes of compact JSON.

## Stdout/Stderr Strategy

Tools produce a single JSON value — no stderr concept. Buffer storage:
- `stdout` = full JSON result (queryable via `jq`/`grep` on `@tool_xxx`)
- `stderr` = `None` (no stderr for tools)
- `exit_code` = `None`
- `command` = tool name

The `format_for_user()` method serves as the single compact summary, covering both result info and any diagnostic/warning content (overflow hints, staleness warnings, etc.).

`@tool_xxx.err` refs are not created for tool outputs.

## Components

### A. `OutputBuffer::store_tool(tool_name: &str, json: &str) -> String`

New method (~10 lines). Mirrors `store_file()`. Generates `@tool_xxxx` ref, stores JSON as `stdout`, returns the ref handle.

### B. `Tool::call_content()` default (modified)

```
let json = to_string(&val);   // compact JSON
if json.len() > 10_000:
    ref_id = output_buffer.store_tool(self.name(), &json)
    summary = format_for_user(&val)
              .unwrap_or("Result stored in {ref_id} ({N} bytes)")
    return single Content::text("{summary}\nFull result: {ref_id}")  // both audiences
else:
    existing behavior unchanged
```

### C. `format_for_user()` overrides (optional, per-tool)

Default returns `None` → generic fallback message used.

Priority overrides (5 tools most likely to exceed threshold):
- `ListSymbols` → "Found N symbols across X files (M shown). Full result: @ref"
- `FindSymbol` → "Found N matches across X files. Full result: @ref"
- `SemanticSearch` → "Found N results. Full result: @ref"
- `SearchPattern` → "Found N matches in X files. Full result: @ref"
- `FindReferences` → "Found N references. Full result: @ref"

### D. `resolve_refs` regex (1-line change)

`@(?:cmd|file)_` → `@(?:cmd|file|tool)_`

## Data Flow

**Large response (buffered):**
```
call_content()
  → call() → Value
  → to_string() → compact JSON (> 10k)
  → output_buffer.store_tool("list_symbols", json) → "@tool_a1b2c3"
  → format_for_user(&val) → Some("Found 200 symbols (47 shown)")
  → return Content::text("Found 200 symbols (47 shown)\nFull result: @tool_a1b2c3")

Agent queries:
  → run_command("jq '.symbols[].name' @tool_a1b2c3")
  → resolve_refs() substitutes @tool_a1b2c3 → /tmp/ce-buf-a1b2c3
  → command runs against temp file
```

**Small response (unchanged):**
```
call_content()
  → call() → Value
  → to_string() → compact JSON (≤ 10k)
  → existing behavior: pretty-print, or split by audience if format_for_user exists
```

## Error Handling

- **`call()` fails** — propagates as `Result::Err` before any buffer logic. No entry stored.
- **`to_string()` fails** — impossible for a valid `Value`; propagated via `?`.
- **Stale buffer ref** — LRU holds 20 entries. Evicted ref passes through `resolve_refs` as literal, command fails with "no such file". Same behavior as `@cmd_xxx`.
- **No partial buffering** — entire JSON is buffered or none of it is.

The threshold is checked against **compact** JSON size (`to_string`, not `to_string_pretty`) — the comparison matches what the assistant would receive without buffering.

## Testing

### `output_buffer.rs` unit tests (2 new)
- `store_tool_generates_tool_ref` — ref starts with `@tool_`, `entry.stdout = json`, `entry.stderr = None`
- `resolve_refs_handles_tool_ref` — `@tool_xxxx` replaced with temp file path

### `mod.rs` unit tests (4 new, minimal mock `Tool`)
- `call_content_buffers_large_output` — JSON > 10k → compact Content, buffer has 1 entry
- `call_content_passthrough_small_output` — JSON ≤ 10k → full JSON, buffer empty
- `call_content_uses_format_for_user_in_compact` — override present → summary text in response
- `call_content_generic_fallback_without_format_for_user` — no override → generic "N bytes" message

### Per-tool override tests (one each for 5 priority tools)
- `list_symbols_format_for_user_includes_count` — returned string mentions symbol count
- Similar for `find_symbol`, `semantic_search`, `search_pattern`, `find_references`

No new integration tests needed — existing tests cover unchanged small-output paths.
