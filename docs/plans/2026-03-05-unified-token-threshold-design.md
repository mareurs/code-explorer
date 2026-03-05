# Unified Token-Based Buffering Threshold

**Date:** 2026-03-05
**Status:** Approved

## Problem

`read_file` on a 10-line `.jsonl` file returns `{file_id, total_lines}` with no content.
The file has few lines but each line is a large JSON object, exceeding the 5KB byte threshold.

Root cause: two conflicting buffering gates—`FILE_BUFFER_THRESHOLD` (200 lines) and
`TOOL_OUTPUT_BUFFER_THRESHOLD` (5,000 bytes)—apply independently. A small-line-count
file with fat lines passes the first gate but hits the second, producing a content-free
buffer-only response.

The same dual-standard problem exists across `run_command` (uses a third threshold:
`SUMMARY_LINE_THRESHOLD` at 50 lines) and `github.rs`.

## Design

Replace three separate buffering triggers with one token-based threshold.

### New constant and helper

```rust
/// Maximum estimated tokens for inline tool output.
/// Content exceeding this is buffered and summarized.
pub(crate) const MAX_INLINE_TOKENS: usize = 2_500; // ~10KB at ~4 bytes/token

pub(crate) fn exceeds_inline_limit(text: &str) -> bool {
    text.len() / 4 > MAX_INLINE_TOKENS
}
```

### Thresholds removed

| Old Constant | Value | Replaced By |
|---|---|---|
| `TOOL_OUTPUT_BUFFER_THRESHOLD` | 5,000 bytes | `MAX_INLINE_TOKENS` via `exceeds_inline_limit()` |
| `FILE_BUFFER_THRESHOLD` | 200 lines | `MAX_INLINE_TOKENS` via `exceeds_inline_limit()` |
| `SUMMARY_LINE_THRESHOLD` | 50 lines | `MAX_INLINE_TOKENS` via `exceeds_inline_limit()` |

### Thresholds kept (formatting, not buffering decisions)

- `BUFFER_QUERY_INLINE_CAP` (100 lines) — inline display cap for buffer queries
- `COMPACT_SUMMARY_MAX_BYTES` (2K) / `COMPACT_SUMMARY_HARD_MAX_BYTES` (3K) — summary size caps

### Per-site changes

1. **`src/tools/mod.rs`** — define `MAX_INLINE_TOKENS` + `exceeds_inline_limit()`.
   Keep `TOOL_OUTPUT_BUFFER_THRESHOLD` as `MAX_INLINE_TOKENS * 4` for byte-budget references.
   `call_content` L271: use `exceeds_inline_limit(&json)`.

2. **`src/tools/file.rs:375`** — replace `line_count > FILE_BUFFER_THRESHOLD` with
   `exceeds_inline_limit(&text)`. Same behavior: buffer as `@file_*` + structural summary.

3. **`src/tools/file.rs:441`** — remove proactive byte-size buffering block entirely
   (now redundant with unified check at L375).

4. **`src/tools/file.rs:108,155,163`** — buffer-ref reading: replace
   `content.len() > TOOL_OUTPUT_BUFFER_THRESHOLD` with `exceeds_inline_limit(&content)`.

5. **`src/tools/command_summary.rs:182`** — `needs_summary()`: replace
   `total_lines > SUMMARY_LINE_THRESHOLD` with token estimate from combined byte length.

6. **`src/tools/workflow.rs`** — buffer-only byte budgets: use `MAX_INLINE_TOKENS * 4`.

7. **`src/tools/github.rs:49`** — `maybe_buffer`: use `exceeds_inline_limit(&content)`.

8. **`src/tools/file_summary.rs:5`** — remove `FILE_BUFFER_THRESHOLD`.

### Testing

- Existing tests updated for new threshold values.
- New test: 10-line JSONL file at 6KB (~1.5K tokens) → content returned inline.
- New test: large source file at 15KB (~3.75K tokens) → buffered with summary.
- Verify run_command summarization tests still pass.
