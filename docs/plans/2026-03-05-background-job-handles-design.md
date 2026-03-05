# Background Job Handles Design

**Date:** 2026-03-05
**Status:** Approved

## Problem

`run_in_background: true` currently spawns a process and returns a raw log file path.
The LLM must construct queries manually (`tail -50 /tmp/codescout-bg-XYZ.log`) and has
no integration with the existing `@ref` buffer system. There is also no initial output
snapshot — the caller gets nothing useful immediately.

## Design

### Warm Return

When `run_in_background: true`:
1. Spawn the process with stdout+stderr redirected to a temp log file
2. Wait 5 seconds (captures fast failures and boot output)
3. Read the log, compute tail-50
4. Store a `@bg_*` handle pointing to the log file
5. Return immediately with the tail snapshot + handle + hint

The process continues running independently after step 5.

### Response Shape

```json
{
  "output_id": "@bg_abc123",
  "stdout": "<last 50 lines of log so far>",
  "hint": "Process running. Output captured in @bg_abc123 — use run_command(\"tail -50 @bg_abc123\") or grep/cat as needed."
}
```

### `@bg_*` Ref Type

`@bg_*` refs are always-fresh: every time `resolve_refs` encounters one, it reads the
log file from disk at that moment (no snapshot). This gives the LLM a live window into
a running process.

Contrast with `@file_*` (snapshot + mtime-based refresh) and `@cmd_*` (immutable
snapshot). `@bg_*` is the only ref type with no caching.

## Changes

### `src/tools/output_buffer.rs`

- Add `background_jobs: IndexMap<String, PathBuf>` to `BufferInner`
- `store_background(log_path: PathBuf) -> String` — generates `@bg_<hex>` id, inserts into map, respects LRU cap
- `get_background(id: &str) -> Option<&PathBuf>` — lookup
- `resolve_refs` — new `@bg_*` branch: read file from disk → write to temp file → substitute path (same temp-file cleanup contract as `@cmd_*`)
- `RecoverableError` if log file is unreadable at query time

### `src/tools/workflow.rs`

Replace the current fire-and-forget background branch in `run_command_inner` with:
- Spawn with stdout+stderr → log file (using `tempfile::Builder::keep()`)
- `tokio::time::sleep(Duration::from_secs(5)).await`
- Read log, compute tail-50
- `ctx.output_buffer.store_background(log_path)`
- Return `{output_id, stdout: tail_50, hint}`

Also: guard `run_in_background + buffer_only` combination with a `RecoverableError`.

## Error Handling

| Case | Behaviour |
|---|---|
| Process fails within 5s | Error output appears in tail-50 naturally; `@bg_*` handle still works |
| Log file unreadable at query time | `RecoverableError`: "background job log unavailable: \<path\>" |
| `@bg_*` handle evicted from buffer | Same as expired `@cmd_*` — RecoverableError, re-run the command |
| Log file on disk after eviction | Not deleted — OS cleans `/tmp` normally |
| `run_in_background + buffer_only` | `RecoverableError`: "run_in_background cannot be used with buffer queries" |

## Tests

### `output_buffer.rs`

- `store_background_returns_bg_prefix` — id starts with `@bg_`
- `resolve_refs_bg_reads_fresh_from_disk` — write A → resolve → get A; write B → resolve → get B
- `resolve_refs_bg_missing_file_errors` — non-existent path → RecoverableError

### `workflow.rs`

- `run_in_background_returns_bg_handle` — `echo hello` with `run_in_background: true` → `output_id` starts with `@bg_`, `stdout` contains "hello", hint mentions handle
