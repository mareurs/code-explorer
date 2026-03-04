# Design: Lower OutputBuffer Threshold + Cap Compact Summaries

**Date:** 2026-03-04
**Status:** Approved

## Problem

`call_content` in the `Tool` trait buffers tool output in `OutputBuffer` only when the
serialized JSON exceeds `TOOL_OUTPUT_BUFFER_THRESHOLD` (currently 10,000 bytes). This
threshold is too high:

- `search_pattern` with 50 matches (default) produces ~6–8 KB → **below threshold** →
  LLM sees 50 raw JSON objects inline, no `@tool_ref` for follow-up queries
- `find_file` with 100 paths produces ~5–8 KB → same problem
- The `format_compact` implementations on these tools are only used in the buffer path,
  so the nicely-formatted compact text is wasted for typical-sized outputs

Secondary problem: the compact summary shown alongside a `@tool_ref` is uncapped. A
`format_compact` implementation returning 8 KB of formatted text would fill the context
window almost as badly as the raw JSON it replaced.

## Goals

1. Lower the buffer threshold so typical list-tool outputs get routed through `OutputBuffer`
2. Cap compact summaries at ~2 KB (soft target, line-boundary-aware) to bound inline context use
3. Zero per-tool changes — the fix lives entirely in the `Tool` trait default

## Non-Goals

- Fixing `find_file`'s `format_compact` returning just `"N files"` (tracked separately)
- Changing write operations (`edit_file`, `create_file`, etc.) — they return `json!("ok")`
  which is trivially small and never hits any threshold
- Changing `run_command`'s own buffer path — it routes through `ctx.output_buffer.store()`
  directly, bypassing `call_content` entirely

## Design

### Constants (`src/tools/mod.rs`)

```rust
// Lowered from 10_000 — buffers most real list/search results
pub(crate) const TOOL_OUTPUT_BUFFER_THRESHOLD: usize = 5_000;

// Soft cap for compact summary text; see truncate_compact()
pub(crate) const COMPACT_SUMMARY_MAX_BYTES: usize = 2_000;

// Hard cap — summary will never exceed this regardless of line boundaries
pub(crate) const COMPACT_SUMMARY_HARD_MAX_BYTES: usize = 3_000;
```

### `truncate_compact` helper (`src/tools/mod.rs`)

```rust
/// Truncate a compact summary to fit within the soft cap, preferring whole-line
/// boundaries. If the last whole line ≤ soft_max falls within the hard_max, that
/// line boundary is used. If no whole-line boundary exists within soft_max,
/// truncates at hard_max bytes directly.
///
/// Appends "… (truncated)" when content is cut.
fn truncate_compact(text: &str, soft_max: usize, hard_max: usize) -> String {
    if text.len() <= soft_max {
        return text.to_string();
    }
    // Find the last newline at or before soft_max
    let candidate = text[..soft_max.min(text.len())]
        .rfind('\n')
        .map(|pos| &text[..pos]);

    if let Some(cut) = candidate {
        if cut.len() <= hard_max {
            return format!("{}\n… (truncated)", cut);
        }
    }

    // No usable line boundary — hard truncate
    let end = hard_max.min(text.len());
    format!("{}… (truncated)", &text[..end])
}
```

### `call_content` buffer path (`src/tools/mod.rs`)

```rust
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    let val = self.call(input, ctx).await?;
    let json = serde_json::to_string(&val).unwrap_or_else(|_| val.to_string());

    if json.len() > TOOL_OUTPUT_BUFFER_THRESHOLD {
        let json_len = json.len();
        let ref_id = ctx.output_buffer.store_tool(self.name(), json);
        let raw_summary = self
            .format_compact(&val)
            .unwrap_or_else(|| format!("Result stored in {} ({} bytes)", ref_id, json_len));
        let summary = truncate_compact(
            &raw_summary,
            COMPACT_SUMMARY_MAX_BYTES,
            COMPACT_SUMMARY_HARD_MAX_BYTES,
        );
        return Ok(vec![Content::text(format!(
            "{}\nFull result: {}",
            summary, ref_id
        ))]);
    }

    // Small output — return pretty JSON to the assistant.
    Ok(vec![Content::text(
        serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string()),
    )])
}
```

The small-output path (under 5 KB) is unchanged — still returns pretty JSON inline.

## Impact by Tool

| Tool | Old behavior | New behavior |
|---|---|---|
| `search_pattern` 50 matches (no context) | ~7 KB → inline raw JSON | ~7 KB → buffered, formatted table + `@tool_ref` |
| `search_pattern` with context_lines | Already >10 KB → buffered | Still buffered, earlier |
| `find_file` 100 paths | ~6 KB → inline raw JSON | ~6 KB → buffered, compact + `@tool_ref` |
| `list_symbols` large file | Already >10 KB → buffered | Now buffers at 5 KB |
| `semantic_search` 10 results | ~3–4 KB → inline JSON | Still inline (under 5 KB) |
| Write ops (`edit_file` etc.) | `"ok"` → inline | Still inline (≤ 50 bytes) |
| `run_command` | Own buffer path | Unaffected |

## Truncation Semantics

The soft/hard cap distinction exists because structure matters more than byte count:

- **Soft cap (2 KB):** preferred truncation point, at a whole-line boundary
- **Hard cap (3 KB):** maximum allowed regardless of line structure; prevents pathological
  cases where a single very long line (e.g. a minified JSON string in search results)
  would push the summary over budget

If `format_compact` returns text under 2 KB, it's shown verbatim — no truncation.
If it returns 2.1 KB but the last line boundary is at 1.95 KB, we use 1.95 KB (+ note).
If a single line spans 2–3 KB, we truncate at 3 KB mid-line (rare but bounded).

## Tests

### Unit tests for `truncate_compact`

In `src/tools/mod.rs` test module:

1. `truncate_compact_under_soft_cap` — text ≤ 2 KB returned verbatim
2. `truncate_compact_at_line_boundary` — truncates at last `\n` ≤ 2 KB, appends note
3. `truncate_compact_no_newlines_uses_hard_cap` — no line boundary → hard-cap at 3 KB
4. `truncate_compact_line_boundary_within_hard_cap` — uses line boundary even when > soft_max
5. `truncate_compact_exact_soft_boundary` — text exactly 2 KB returned verbatim

### Integration tests (tool test harnesses)

In existing test modules:

- `search_pattern_5k_plus_is_buffered` — build a result with >5 KB JSON, assert response
  contains `@tool_` ref
- `search_pattern_under_5k_is_inline` — result under 5 KB, assert response is raw JSON
- `compact_summary_is_capped` — inject a `format_compact` returning 4 KB, assert inline
  summary ≤ 3 KB

## Files Changed

- `src/tools/mod.rs` — constants, `truncate_compact`, `call_content` update (~30 lines)

No other files need to change.
