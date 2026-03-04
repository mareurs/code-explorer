# Design: Unfiltered Output Capture for Piped Filter Commands

**Date:** 2026-03-04  
**Status:** Approved

## Problem

When an LLM runs a piped command ending in a filter:

```
cargo build 2>&1 | grep "error"
```

the filtered result is returned, but the full unfiltered output is discarded. If the filter was too narrow or the wrong pattern, the LLM must re-run the expensive base command to see more. There is no way to "look wider" without paying the full execution cost again.

## Solution

Detect when the last pipe stage is a known terminal filter. Silently inject `tee` before the filter to capture the unfiltered output. Buffer it and return a `@cmd_` ref alongside the normal filtered result.

## Trigger Condition

Activate when **all** of these hold:

1. The command string contains at least one `|`
2. The last pipe stage (ignoring flags and arguments) is a known terminal filter:
   `grep`, `egrep`, `fgrep`, `rg`, `head`, `tail`, `sed`, `awk`, `cut`, `wc`, `sort`, `uniq`, `tr`
3. The command is not a buffer-only query (no `@cmd_`/`@file_` refs — those are already captured data)

## Mechanism — Tee Injection

Before the shell spawn, rewrite the command by splicing `tee <tmpfile>` immediately before the terminal filter stage:

```
# Input
cargo build 2>&1 | grep "error"

# Rewritten
cargo build 2>&1 | tee /tmp/codescout-unfiltered-a1b2c3 | grep "error"
```

The shell runs exactly as specified. The filter still operates on the full stream. We just intercept the pipe non-destructively.

Quote-awareness is required when finding the last `|` — pipes inside quoted strings must not be split on.

## Capping — At Read Time

After execution, read the tee temp file:

- If line count ≤ `SUMMARY_LINE_THRESHOLD`: store the full content.
- If line count > `SUMMARY_LINE_THRESHOLD`: store only the first `SUMMARY_LINE_THRESHOLD` lines; set `unfiltered_truncated: true` in the response.

The filter itself ran against the **full** output. Capping only affects what we store in the buffer. This avoids unbounded disk writes affecting filter behavior.

## Response Shape

Normal case:
```json
{
  "stdout": "src/main.rs:42: error[E0499]: ...\n",
  "stderr": "",
  "exit_code": 1,
  "unfiltered_output": "@cmd_a1b2c3"
}
```

Truncated case:
```json
{
  "stdout": "...",
  "exit_code": 1,
  "unfiltered_output": "@cmd_a1b2c3",
  "unfiltered_truncated": true
}
```

When the unfiltered output was not captured (tee unavailable, buffer-only, no terminal filter):
the `unfiltered_output` field is simply absent.

## Implementation

### 1. `detect_terminal_filter(cmd: &str) -> Option<usize>`

New function in `src/tools/command_summary.rs`.

- Walk the command string respecting single- and double-quoted spans.
- Find the last unquoted `|`.
- Extract the first token of the stage after it (strip leading whitespace, take until whitespace).
- If that token matches the known filter list, return the byte offset of the `|`.
- Otherwise return `None`.

### 2. Tee injection in `run_command_inner`

Location: `src/tools/workflow.rs`, before the shell spawn (after dangerous-command and source-file checks).

```
if !buffer_only {
    if let Some(pipe_pos) = detect_terminal_filter(resolved_command) {
        let tmpfile = format!("/tmp/codescout-unfiltered-{}", random_hex(8));
        rewritten = format!(
            "{} | tee {} {}",
            &resolved_command[..pipe_pos],
            tmpfile,
            &resolved_command[pipe_pos+1..]  // includes the | and filter
        );
        unfiltered_tmpfile = Some(tmpfile);
    }
}
```

Track `unfiltered_tmpfile` alongside existing `temp_files` for cleanup.

### 3. Post-execution read + buffer

After the child exits successfully (before returning the JSON result):

```
if let Some(ref path) = unfiltered_tmpfile {
    if let Ok(content) = fs::read_to_string(path) {
        let line_count = content.lines().count();
        let (stored, truncated) = if line_count > SUMMARY_LINE_THRESHOLD {
            (first_n_lines(&content, SUMMARY_LINE_THRESHOLD), true)
        } else {
            (content, false)
        };
        let ref_id = ctx.output_buffer.store(
            original_command.to_string(),
            stored, "", exit_code
        );
        result["unfiltered_output"] = json!(ref_id);
        if truncated {
            result["unfiltered_truncated"] = json!(true);
        }
    }
    let _ = fs::remove_file(path);
}
```

### 4. Cleanup

The temp file is deleted immediately after reading (step 3). If execution fails or panics before step 3, the file is left in `/tmp` — acceptable, OS will clean it up.

## Edge Cases

| Case | Behaviour |
|---|---|
| Buffer-only command (`grep pat @cmd_xxx`) | Skip — no tee injection. Already working with buffered data. |
| `tee` not in `PATH` | Shell error propagates as stderr; `unfiltered_output` field absent. No crash. |
| Filter exits early (`head -5`) | Tee captures partial output up to when head closed the pipe. Ref is partial but still useful. |
| Nested pipes (`cmd \| sed \| grep`) | `detect_terminal_filter` finds the last `|`; only the last stage is a filter. Tee is injected before `grep`. `sed` output — which is the unfiltered-relative-to-grep output — is captured. Correct. |
| Command times out | Temp file may have partial content. Read and buffer what's there; delete file. |
| `stdout` from filter is large (also needs buffering) | Normal `needs_summary` path handles this independently. Both paths can fire. |

## Files Touched

- `src/tools/command_summary.rs` — add `detect_terminal_filter`
- `src/tools/workflow.rs` — tee injection + post-execution capture in `run_command_inner`
- Tests in both files
