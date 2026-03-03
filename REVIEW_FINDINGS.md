# Tooling Code Review Findings

## High

### OutputBuffer buffer-only classification can bypass command safety checks
- **Location:** `src/tools/output_buffer.rs:368-461` and `476-488`, plus `src/tools/workflow.rs:680-725`
- **Issue:** `buffer_only` is computed as true unless an argument starts with `/` or `./`. This misses relative paths like `src/foo.rs`, so `run_command_inner` skips dangerous/source-file checks.
- **Impact:** Relative-path commands can avoid safety gating, weakening protections against unintended source access or risky commands.
- **Fix:** Treat any argument that looks like a path (e.g., contains `/` or `..`, or parses as a path) as **not** buffer-only. Apply the same logic in both `resolve_refs` and `is_buffer_only` so they stay consistent.

## Medium

### read_file does not validate line range
- **Location:** `src/tools/file.rs:70-112` (`ReadFile::call`)
- **Issue:** `start_line`/`end_line` are accepted without validation; `0` or `end < start` silently produce empty or incorrect output.
- **Impact:** Users get confusing results and may miss content or think a file is empty.
- **Fix:** Add a guard requiring `start_line >= 1` and `end_line >= start_line`. Return a `RecoverableError` with a hint (mirrors `goto_definition` behavior).

## Low

### search_pattern context mode mixes counts
- **Location:** `src/tools/file.rs:308-437` (`SearchPattern::call`)
- **Issue:** In context mode, `total` counts match lines, but the result list contains merged context blocks, so `total` can exceed `matches.len()`.
- **Impact:** Output is confusing and makes pagination or “results count” interpretation inconsistent.
- **Fix:** Either report both counts (e.g., `match_count` and `block_count`) or align `total` with the number of blocks returned.
