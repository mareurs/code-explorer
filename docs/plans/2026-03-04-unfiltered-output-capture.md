# Unfiltered Output Capture Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** When a command ends with a terminal filter (`grep`, `head`, `tail`, `sed`, etc.), silently capture the unfiltered stream via `tee`, buffer it, and add `unfiltered_output: "@cmd_xxxx"` to the response so the LLM can look wider without re-running the expensive base command.

**Architecture:** Inject `tee /tmp/codescout-unfiltered-XXXX` before the terminal filter stage, run the command unchanged, read and cap the tee file post-execution, store it in `OutputBuffer`, attach the ref to the result JSON.

**Tech Stack:** Rust, tokio, serde_json; touches `src/tools/command_summary.rs` and `src/tools/workflow.rs`.

---

## Background — Key Code Locations

- `src/tools/command_summary.rs:15` — `SUMMARY_LINE_THRESHOLD = 50` (reuse as cap for unfiltered storage)
- `src/tools/command_summary.rs:110-119` — `detect_command_type` (add `detect_terminal_filter` near here)
- `src/tools/workflow.rs:784` — `run_command_inner` signature
- `src/tools/workflow.rs:877-881` — shell spawn (`sh -c resolved_command`) ← inject before here
- `src/tools/workflow.rs:909-1028` — `Ok(Ok(output))` success branch ← attach unfiltered fields here
- `src/tools/output_buffer.rs:89-119` — `OutputBuffer::store(command, stdout, stderr, exit_code) -> String`

`buffer_only` is `true` when the command contains `@cmd_`/`@file_` refs — skip tee injection entirely in that case (data is already captured).

---

## Task 1: `detect_terminal_filter` in command_summary.rs

**Files:**
- Modify: `src/tools/command_summary.rs`

### Step 1: Write the failing tests

Add to the `tests` module at the bottom of `src/tools/command_summary.rs`:

```rust
// -- detect_terminal_filter --

#[test]
fn terminal_filter_grep() {
    let pos = detect_terminal_filter("cargo build 2>&1 | grep error");
    assert!(pos.is_some());
}

#[test]
fn terminal_filter_head() {
    let pos = detect_terminal_filter("cat big_file.log | head -20");
    assert!(pos.is_some());
}

#[test]
fn terminal_filter_tail() {
    let pos = detect_terminal_filter("journalctl | tail -100");
    assert!(pos.is_some());
}

#[test]
fn terminal_filter_no_pipe() {
    assert!(detect_terminal_filter("cargo build").is_none());
}

#[test]
fn terminal_filter_non_filter_pipe() {
    // Second stage is not a known filter
    assert!(detect_terminal_filter("cat file | cargo install").is_none());
}

#[test]
fn terminal_filter_quoted_pipe_ignored() {
    // Pipe inside quotes is not a real pipe
    assert!(detect_terminal_filter("echo 'foo | bar'").is_none());
}

#[test]
fn terminal_filter_nested_filters_last_wins() {
    // cmd | sed | grep  →  finds the grep (last) pipe
    let cmd = "cat file | sed 's/x/y/' | grep foo";
    let pos = detect_terminal_filter(cmd);
    assert!(pos.is_some());
    // The position should be the second pipe (before grep), not the first
    let pipe_pos = pos.unwrap();
    assert!(cmd[pipe_pos + 1..].trim_start().starts_with("grep"));
}

#[test]
fn terminal_filter_returns_pipe_position() {
    let cmd = "cargo build | grep error";
    let pos = detect_terminal_filter(cmd).unwrap();
    // Character at pipe_pos should be '|'
    assert_eq!(&cmd[pos..pos + 1], "|");
}
```

### Step 2: Run tests to confirm they fail

```bash
cargo test -p codescout terminal_filter 2>&1 | head -20
```
Expected: compile error — `detect_terminal_filter` not yet defined.

### Step 3: Implement `detect_terminal_filter`

Add this function to `src/tools/command_summary.rs` just after `detect_command_type` (after line 119):

```rust
/// Returns the byte offset of the `|` pipe character that separates a command
/// from a terminal filter stage (grep, head, tail, sed, awk, cut, wc, sort,
/// uniq, tr, rg, egrep, fgrep).
///
/// Returns `None` if the command has no pipe, or if the last pipe stage is not
/// a known terminal filter. Pipe characters inside quoted strings are ignored.
pub fn detect_terminal_filter(cmd: &str) -> Option<usize> {
    const TERMINAL_FILTERS: &[&str] = &[
        "grep", "egrep", "fgrep", "rg",
        "head", "tail",
        "sed", "awk",
        "cut", "wc", "sort", "uniq", "tr",
    ];

    let mut last_pipe: Option<usize> = None;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape_next = false;

    for (i, ch) in cmd.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if !in_single => escape_next = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '|' if !in_single && !in_double => last_pipe = Some(i),
            _ => {}
        }
    }

    let pipe_pos = last_pipe?;

    // Extract the first token of the stage after the pipe
    let after_pipe = cmd[pipe_pos + 1..].trim_start();
    let token = after_pipe
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("");

    // Strip any path prefix so "/usr/bin/grep" matches "grep"
    let name = token.rsplit('/').next().unwrap_or(token);

    if TERMINAL_FILTERS.contains(&name) {
        Some(pipe_pos)
    } else {
        None
    }
}
```

### Step 4: Run tests to confirm they pass

```bash
cargo test -p codescout terminal_filter 2>&1 | tail -10
```
Expected: 8 tests, all pass.

### Step 5: Lint and format

```bash
cargo fmt && cargo clippy -- -D warnings 2>&1 | grep -E "error|warning"
```
Expected: no errors.

### Step 6: Commit

```bash
git add src/tools/command_summary.rs
git commit -m "feat: add detect_terminal_filter for piped filter detection"
```

---

## Task 2: Tee injection in `run_command_inner`

**Files:**
- Modify: `src/tools/workflow.rs`

The goal: before the shell spawn, if the command ends with a terminal filter and we're not in buffer-only mode, rewrite it to inject `tee /tmp/codescout-unfiltered-XXXX` before the filter stage.

### Step 1: Write failing integration test

Add to the `tests` module in `src/tools/workflow.rs` (after the existing tests):

```rust
#[tokio::test]
async fn piped_grep_returns_unfiltered_ref() {
    let (dir, ctx) = project_ctx().await;
    // Create a file with several lines; grep for just one
    std::fs::write(
        dir.path().join("items.txt"),
        "apple\nbanana\ncherry\ndates\nelderberry\n",
    )
    .unwrap();
    let result = RunCommand
        .call(
            json!({ "command": "cat items.txt | grep apple" }),
            &ctx,
        )
        .await
        .unwrap();

    // unfiltered_output ref should be present
    assert!(
        result["unfiltered_output"].is_string(),
        "expected unfiltered_output field, got: {result}"
    );
    let ref_id = result["unfiltered_output"].as_str().unwrap();

    // Query the buffer: full content should include banana (filtered out by grep)
    let full = RunCommand
        .call(json!({ "command": format!("cat {ref_id}") }), &ctx)
        .await
        .unwrap();
    let stdout = full["stdout"].as_str().unwrap_or("");
    assert!(stdout.contains("banana"), "unfiltered output missing 'banana': {stdout}");
    assert!(stdout.contains("apple"), "unfiltered output missing 'apple': {stdout}");
}

#[tokio::test]
async fn non_filter_pipe_no_unfiltered_ref() {
    let (_dir, ctx) = project_ctx().await;
    // Second stage is not a known filter — no unfiltered_output
    let result = RunCommand
        .call(json!({ "command": "echo hello | cat" }), &ctx)
        .await
        .unwrap();
    assert!(
        result.get("unfiltered_output").is_none(),
        "unexpected unfiltered_output for non-filter pipe: {result}"
    );
}
```

### Step 2: Run tests to confirm they fail

```bash
cargo test -p codescout piped_grep_returns_unfiltered_ref 2>&1 | tail -15
```
Expected: FAIL — `unfiltered_output` field absent.

### Step 3: Add the import in `run_command_inner`

In `src/tools/workflow.rs`, inside `run_command_inner`, the existing import block at line ~795 is:
```rust
use super::command_summary::{
    count_lines, detect_command_type, needs_summary, ...
```

Add `detect_terminal_filter` to that import:
```rust
use super::command_summary::{
    count_lines, detect_command_type, detect_terminal_filter, needs_summary, summarize_build_output,
    summarize_generic, summarize_test_output, truncate_lines, truncate_lines_and_bytes,
    CommandType, BUFFER_QUERY_INLINE_CAP, SUMMARY_LINE_THRESHOLD,
};
```

Note: also add `SUMMARY_LINE_THRESHOLD` to the import (needed in Task 3).

### Step 4: Add tee injection before the spawn

The spawn is at `src/tools/workflow.rs:877`. Add this block immediately **before** the `#[cfg(unix)] let child = ...` spawn (i.e., between Step 4 work dir resolution and Step 5 execute):

```rust
// --- Step 4.5: Tee injection for terminal filter commands ---
// When the last pipe stage is a known filter (grep, head, tail, sed, awk, etc.),
// inject `tee /tmp/codescout-unfiltered-XXXX` before the filter so we can
// surface the unfiltered output as a buffer ref.
let (effective_command, unfiltered_tmpfile) = if !buffer_only {
    if let Some(pipe_pos) = detect_terminal_filter(resolved_command) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmpfile = format!("/tmp/codescout-unfiltered-{nanos:016x}");
        let cmd = format!(
            "{} | tee {} | {}",
            resolved_command[..pipe_pos].trim_end(),
            tmpfile,
            resolved_command[pipe_pos + 1..].trim_start()
        );
        (cmd, Some(tmpfile))
    } else {
        (resolved_command.to_string(), None)
    }
} else {
    (resolved_command.to_string(), None)
};
```

Then update the spawn lines (currently `.arg(resolved_command)`) to use `effective_command`:

```rust
#[cfg(unix)]
let child = tokio::process::Command::new("sh")
    .arg("-c")
    .arg(&effective_command)   // ← was: resolved_command
    .current_dir(&work_dir)
    .output();

#[cfg(windows)]
let child = tokio::process::Command::new("cmd")
    .arg("/C")
    .arg(&effective_command)   // ← was: resolved_command
    .current_dir(&work_dir)
    .output();
```

### Step 5: Run tests to confirm they still compile (but the new test still fails)

```bash
cargo test -p codescout non_filter_pipe_no_unfiltered_ref 2>&1 | tail -10
```
Expected: PASS — no injection for non-filter pipe.

```bash
cargo test -p codescout piped_grep_returns_unfiltered_ref 2>&1 | tail -15
```
Expected: FAIL — tee runs but `unfiltered_output` field not yet attached to result.

### Step 6: Commit intermediate state

```bash
git add src/tools/workflow.rs
git commit -m "feat: inject tee before terminal filter stage in run_command"
```

---

## Task 3: Post-execution read, cap, buffer, and attach field

**Files:**
- Modify: `src/tools/workflow.rs`

### Step 1: Write the truncation test

Add to `tests` in `src/tools/workflow.rs`:

```rust
#[tokio::test]
async fn unfiltered_truncated_when_over_threshold() {
    let (dir, ctx) = project_ctx().await;
    // Write SUMMARY_LINE_THRESHOLD + 10 lines; grep for just one
    let content: String = (0..60).map(|i| format!("line{i}\n")).collect();
    std::fs::write(dir.path().join("big.txt"), &content).unwrap();
    let result = RunCommand
        .call(
            json!({ "command": "cat big.txt | grep line0" }),
            &ctx,
        )
        .await
        .unwrap();
    // truncated flag should be set (60 lines > SUMMARY_LINE_THRESHOLD=50)
    assert_eq!(
        result["unfiltered_truncated"],
        json!(true),
        "expected truncated flag: {result}"
    );
}
```

### Step 2: Run to confirm it fails

```bash
cargo test -p codescout unfiltered_truncated 2>&1 | tail -10
```
Expected: FAIL — field absent.

### Step 3: Add post-execution capture logic

In `src/tools/workflow.rs`, inside the `Ok(Ok(output)) =>` branch (around line 910), **after** `let exit_code = output.status.code().unwrap_or(-1);` and **before** the `if needs_summary(...)` block, add:

```rust
// --- Step 6.5: Read tee capture and store as unfiltered_output ref ---
let unfiltered_ref: Option<(String, bool)> = if let Some(ref tmpfile) = unfiltered_tmpfile {
    let capture = std::fs::read_to_string(tmpfile).ok();
    let _ = std::fs::remove_file(tmpfile); // always clean up
    capture.map(|content| {
        let line_count = count_lines(&content);
        let (stored, truncated) = if line_count > SUMMARY_LINE_THRESHOLD {
            let capped = content
                .lines()
                .take(SUMMARY_LINE_THRESHOLD)
                .collect::<Vec<_>>()
                .join("\n");
            (capped, true)
        } else {
            (content, false)
        };
        let ref_id = ctx.output_buffer.store(
            original_command.to_string(),
            stored,
            String::new(),
            exit_code,
        );
        (ref_id, truncated)
    })
} else {
    None
};
```

### Step 4: Attach the fields to both result paths

The success branch currently ends with two `return Ok(...)` / `Ok(...)` paths. Refactor the `Ok(Ok(output))` branch to compute `result` as a `Value` first, then inject, then return:

**Replace the end of the `Ok(Ok(output))` branch** — the part that currently reads:

```rust
            if needs_summary(&raw_stdout, &raw_stderr) {
                if buffer_only {
                    // ... truncated inline path ... return Ok(result)
                }
                let output_id = ctx.output_buffer.store(...);
                // ...
                let summary = rebuild_buffered_summary(cmd_summary, &output_id);
                Ok(summary)
            } else {
                // Short output — return directly
                let result = json!({...});
                Ok(result)
            }
```

**With:**

```rust
            let mut result = if needs_summary(&raw_stdout, &raw_stderr) {
                if buffer_only {
                    // buffer-only truncated inline path — return early, no tee involved
                    // ... (keep existing buffer-only block unchanged, just change to `return Ok(...)`)
                    return Ok(result); // keep existing early return
                }
                let output_id = ctx.output_buffer.store(
                    original_command.to_string(),
                    raw_stdout.clone(),
                    raw_stderr.clone(),
                    exit_code,
                );
                let cmd_type = detect_command_type(original_command);
                let cmd_summary = match cmd_type {
                    CommandType::Test => summarize_test_output(&raw_stdout, &raw_stderr, exit_code),
                    CommandType::Build => summarize_build_output(&raw_stdout, &raw_stderr, exit_code),
                    CommandType::Generic => summarize_generic(&raw_stdout, &raw_stderr, exit_code),
                };
                rebuild_buffered_summary(cmd_summary, &output_id)
            } else {
                json!({
                    "stdout": raw_stdout,
                    "stderr": raw_stderr,
                    "exit_code": exit_code,
                })
            };

            // Attach unfiltered_output ref if we captured via tee
            if let Some((ref ref_id, truncated)) = unfiltered_ref {
                result["unfiltered_output"] = json!(ref_id);
                if truncated {
                    result["unfiltered_truncated"] = json!(true);
                }
            }

            Ok(result)
```

### Step 5: Run all three new tests

```bash
cargo test -p codescout piped_grep_returns_unfiltered_ref non_filter_pipe_no_unfiltered_ref unfiltered_truncated 2>&1 | tail -20
```
Expected: all 3 PASS.

### Step 6: Run full test suite

```bash
cargo test 2>&1 | tail -10
```
Expected: all existing tests still pass.

### Step 7: Lint and format

```bash
cargo fmt && cargo clippy -- -D warnings
```
Expected: clean.

### Step 8: Commit

```bash
git add src/tools/workflow.rs
git commit -m "feat: attach unfiltered_output ref to piped-filter command results"
```

---

## Task 4: Final verification

### Step 1: Manual smoke test

```bash
cargo run -- start --project . &
# (in another terminal or via mcp client)
# run_command("cargo build 2>&1 | grep error")
# verify response contains unfiltered_output: "@cmd_xxxx"
# run_command("cat @cmd_xxxx") to see full build output
```

### Step 2: Full test suite + lint

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```
Expected: all pass, clean lint.

### Step 3: Commit if anything was adjusted

```bash
git add -p
git commit -m "fix: cleanup from smoke test"
```

---

## Summary of Changes

| File | What changes |
|---|---|
| `src/tools/command_summary.rs` | Add `detect_terminal_filter(cmd: &str) -> Option<usize>` + 8 unit tests |
| `src/tools/workflow.rs` | Import `detect_terminal_filter` + `SUMMARY_LINE_THRESHOLD`; add tee injection before spawn; add post-execution read/cap/buffer/attach logic; 3 integration tests |
