# Design: buffer-only inline truncation

**Date:** 2026-03-01  
**Status:** Approved

## Problem

When an agent queries a buffer ref (`sed @cmd_A`, `grep @file_B`) and the
result is still > 50 lines, the current code returns a `RecoverableError`:

```
"Command output is still 84 lines — too large to return inline."
```

This forces an extra round-trip and discards data the agent already has. The
original error existed to prevent an infinite loop (buffer → new buffer →
repeat), but returning truncated *inline* text avoids the loop just as well,
while being directly useful.

## Behavior change

| | Before | After |
|---|---|---|
| `buffer_only` + output > 50 lines | `RecoverableError` | Truncated `Ok` response with per-stream metadata |
| No new buffer ref created | ✓ | ✓ (unchanged — no loop risk) |
| Non-`buffer_only` path | unchanged | unchanged |

## Truncation algorithm

stderr gets a priority budget of 20 lines; the remainder (up to 30) goes to
stdout. If stderr is short, stdout inherits the surplus.

```
stderr_shown = min(actual_stderr_lines, 20)
stdout_budget = SUMMARY_LINE_THRESHOLD − stderr_shown   // 50 − stderr_shown
stdout_shown  = min(actual_stdout_lines, stdout_budget)
truncated     = (stderr_shown < actual_stderr_lines) OR (stdout_shown < actual_stdout_lines)
```

**Rationale for 30/20 split:** when a command produces errors, stderr content
is more actionable than log noise in stdout. Prioritising up to 20 stderr lines
ensures the agent sees the most relevant signal first.

## Response shape

### Truncated case

```json
{
  "stdout": "<first stdout_shown lines>",
  "stderr": "<first stderr_shown lines>",
  "exit_code": 1,
  "truncated": true,
  "stdout_shown": 30,
  "stdout_total": 74,
  "stderr_shown": 20,
  "stderr_total": 35,
  "hint": "Output capped at 50 lines (stdout 30/74, stderr 20/35). Narrow with: grep 'keyword' @ref, sed -n '1,49p' @ref"
}
```

### Non-truncated case (combined ≤ 50 lines)

Extra fields are omitted — response is identical to a normal short-output
result. No `truncated`, `*_shown`, or `*_total` fields.

```json
{
  "stdout": "...",
  "stderr": "...",
  "exit_code": 0
}
```

## New helper: `truncate_lines`

Line-based sibling to the existing byte-based `truncate_output`. Lives in
`src/tools/command_summary.rs`:

```rust
/// Truncate `text` to at most `max_lines` lines.
/// Returns (truncated_text, lines_shown, lines_total).
/// When total ≤ max_lines the text is returned unchanged and shown == total.
pub fn truncate_lines(text: &str, max_lines: usize) -> (String, usize, usize) {
    let total = count_lines(text);
    if total <= max_lines {
        return (text.to_string(), total, total);
    }
    let truncated = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    (truncated, max_lines, total)
}
```

## Implementation sites

### `src/tools/workflow.rs` — `run_command_inner`

Replace the `if buffer_only { return Err(...) }` block (currently ~L604–620)
with the truncation logic:

```rust
if buffer_only {
    let stderr_shown_max = 20usize;
    let stderr_shown = count_lines(&raw_stderr).min(stderr_shown_max);
    let stdout_budget = SUMMARY_LINE_THRESHOLD - stderr_shown;

    let (stdout_out, stdout_shown, stdout_total) =
        truncate_lines(&raw_stdout, stdout_budget);
    let (stderr_out, stderr_shown_actual, stderr_total) =
        truncate_lines(&raw_stderr, stderr_shown_max);

    let was_truncated = stdout_shown < stdout_total
        || stderr_shown_actual < stderr_total;

    let mut result = json!({
        "stdout": stdout_out,
        "stderr": stderr_out,
        "exit_code": exit_code,
    });
    if was_truncated {
        result["truncated"] = json!(true);
        result["stdout_shown"] = json!(stdout_shown);
        result["stdout_total"] = json!(stdout_total);
        if stderr_total > 0 {
            result["stderr_shown"] = json!(stderr_shown_actual);
            result["stderr_total"] = json!(stderr_total);
        }
        result["hint"] = json!(format!(
            "Output capped at {SUMMARY_LINE_THRESHOLD} lines \
             (stdout {stdout_shown}/{stdout_total}\
             {stderr_extra}). Narrow with: grep 'keyword' @ref, \
             sed -n '1,{max}p' @ref",
            stderr_extra = if stderr_total > 0 {
                format!(", stderr {stderr_shown_actual}/{stderr_total}")
            } else {
                String::new()
            },
            max = SUMMARY_LINE_THRESHOLD - 1,
        ));
    }
    return Ok(result);
}
```

### `src/tools/command_summary.rs`

Add `truncate_lines` function and export it alongside `count_lines`.

## Tests

### `src/tools/workflow.rs` — tests to update

| Old name | New name | Change |
|---|---|---|
| `run_command_buffer_only_above_threshold_returns_error` | `run_command_buffer_only_above_threshold_truncates_inline` | Flip `is_err()` → `is_ok()`; assert `truncated == true`, no `output_id` |
| `run_command_buffer_only_large_output_returns_error_not_new_ref` | `run_command_buffer_only_large_output_no_new_ref` | Flip; assert no `output_id`; assert `stdout_shown`/`stdout_total` |

### New tests in `workflow.rs`

| Test | What it verifies |
|---|---|
| `run_command_buffer_only_stderr_gets_priority` | stderr=25 lines → `stderr_shown=20`, `stdout_shown=30` |
| `run_command_buffer_only_short_stderr_gives_budget_to_stdout` | stderr=10 → `stdout_shown=40` |
| `run_command_buffer_only_no_stderr_full_stdout_budget` | stderr=0 → `stdout_shown=50` |
| `run_command_buffer_only_within_limit_no_truncation_fields` | combined ≤ 50 → no `truncated` field present |

### New tests in `command_summary.rs`

| Test | What it verifies |
|---|---|
| `truncate_lines_short_returns_unchanged` | ≤ max → text unchanged, shown==total |
| `truncate_lines_long_truncates_correctly` | > max → first N lines returned, correct counts |
| `truncate_lines_empty_string` | empty input → (empty, 0, 0) |
| `truncate_lines_exact_limit` | exactly max lines → not truncated |

## Documentation updates

| File | Change |
|---|---|
| `src/prompts/server_instructions.md` | Update anti-pattern note: buffer queries now return ≤ 50 lines inline with truncation info; still recommend targeted queries |
| `docs/manual/src/concepts/output-buffers.md` | Add section describing truncated inline response for oversized buffer queries |
| `docs/FEATURES.md` | Update one-liner: "buffer queries return ≤ 50 lines inline with truncation metadata" |
