# Buffer-Only Inline Truncation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** When a `run_command` buffer-ref query (e.g. `sed @cmd_A`) produces output > 50 lines, return the first 50 lines inline with per-stream truncation metadata instead of a `RecoverableError`.

**Architecture:** Add a `truncate_lines` helper to `command_summary.rs`, then replace the `if buffer_only { return Err(...) }` block in `run_command_inner` with line-based truncation logic (stderr priority: up to 20 lines; stdout: remaining budget up to 30). No new buffer ref is created, preserving the anti-loop invariant.

**Tech Stack:** Rust, `serde_json`, existing `count_lines` + `SUMMARY_LINE_THRESHOLD` from `command_summary.rs`

---

### Task 1: Add `truncate_lines` helper to `command_summary.rs`

**Files:**
- Modify: `src/tools/command_summary.rs` (after `count_lines` at L255)

**Step 1: Write the failing tests**

Add inside the existing `#[cfg(test)]` module (after the `count_lines_normal` test at L495):

```rust
#[test]
fn truncate_lines_short_returns_unchanged() {
    let text = "a\nb\nc";
    let (out, shown, total) = truncate_lines(text, 10);
    assert_eq!(out, text);
    assert_eq!(shown, 3);
    assert_eq!(total, 3);
}

#[test]
fn truncate_lines_exact_limit_not_truncated() {
    let text: String = (1..=5).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let (out, shown, total) = truncate_lines(&text, 5);
    assert_eq!(shown, 5);
    assert_eq!(total, 5);
    assert_eq!(out, text);
}

#[test]
fn truncate_lines_long_truncates_correctly() {
    let text: String = (1..=10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let (out, shown, total) = truncate_lines(&text, 3);
    assert_eq!(shown, 3);
    assert_eq!(total, 10);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "line 1");
    assert_eq!(lines[2], "line 3");
}

#[test]
fn truncate_lines_empty_string() {
    let (out, shown, total) = truncate_lines("", 10);
    assert_eq!(out, "");
    assert_eq!(shown, 0);
    assert_eq!(total, 0);
}
```

**Step 2: Run to verify tests fail**

Run: `cargo test truncate_lines -- --nocapture 2>&1 | head -20`
Expected: compile error — `truncate_lines` not found.

**Step 3: Add `truncate_lines` after `count_lines` (L255 in `command_summary.rs`)**

Insert this function immediately after the `count_lines` function body:

```rust
/// Truncate `text` to at most `max_lines` lines.
///
/// Returns `(truncated_text, lines_shown, lines_total)`.
/// When `lines_total <= max_lines`, `text` is returned unchanged and
/// `lines_shown == lines_total`.
pub(crate) fn truncate_lines(text: &str, max_lines: usize) -> (String, usize, usize) {
    let total = count_lines(text);
    if total <= max_lines {
        return (text.to_string(), total, total);
    }
    let truncated = text
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");
    (truncated, max_lines, total)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test truncate_lines -- --nocapture`
Expected: 4 tests pass.

**Step 5: Commit**

```
git add src/tools/command_summary.rs
git commit -m "feat: add truncate_lines helper to command_summary"
```

---

### Task 2: Replace buffer-only error with inline truncated response

**Files:**
- Modify: `src/tools/workflow.rs` (the `if buffer_only { return Err(...) }` block at L604–624)

**Context:** The import at the top of `run_command_inner` (L506) already pulls in `count_lines` and `SUMMARY_LINE_THRESHOLD`. You need to add `truncate_lines` to that import.

**Step 1: Write failing tests**

In `workflow.rs`, rename and rewrite the two existing tests, and add three new ones.

**Replace** `run_command_buffer_only_above_threshold_returns_error` (L1418–1434) with:

```rust
#[tokio::test]
async fn run_command_buffer_only_above_threshold_truncates_inline() {
    // SUMMARY_LINE_THRESHOLD + 1 lines — strictly above the limit.
    // Must now return Ok with truncated content, NOT an error.
    let (_dir, ctx) = project_ctx().await;
    let content: String = (1..=SUMMARY_LINE_THRESHOLD + 1)
        .map(|i| format!("{i}\n"))
        .collect();
    let id = ctx.output_buffer.store("cmd".into(), content, "".into(), 0);
    let result = RunCommand
        .call(json!({ "command": format!("cat {}", id) }), &ctx)
        .await
        .expect("expected Ok with truncated inline output");
    assert_eq!(result["truncated"], true, "should be truncated: {:?}", result);
    assert_eq!(result["stdout_shown"], SUMMARY_LINE_THRESHOLD,
        "stdout_shown should equal threshold: {:?}", result);
    assert_eq!(result["stdout_total"], SUMMARY_LINE_THRESHOLD + 1,
        "stdout_total should be full count: {:?}", result);
    assert!(result.get("output_id").is_none(),
        "must not create a new buffer ref: {:?}", result);
}
```

**Replace** `run_command_buffer_only_large_output_returns_error_not_new_ref` (L1464–1497) with:

```rust
#[tokio::test]
async fn run_command_buffer_only_large_output_no_new_ref() {
    // Regression: `sed @cmd_A` that reproduces the full large buffer must
    // return truncated inline content, NOT a new @cmd_B reference.
    let (_dir, ctx) = project_ctx().await;

    let large_content: String = (1..=100).map(|i| format!("{i}\n")).collect();
    let id = ctx
        .output_buffer
        .store("original_cmd".into(), large_content, "".into(), 0);

    let result = RunCommand
        .call(
            json!({ "command": format!("sed -n '1,100p' {}", id) }),
            &ctx,
        )
        .await
        .expect("expected Ok with truncated inline output");

    // Must NOT have an output_id (would mean a new buffer ref was created).
    assert!(
        result.get("output_id").is_none(),
        "must not create a new buffer ref: {:?}",
        result
    );
    // Must be truncated.
    assert_eq!(result["truncated"], true, "should be truncated: {:?}", result);
    // stdout_total should reflect the full 100 lines.
    assert_eq!(result["stdout_total"], 100usize, "stdout_total: {:?}", result);
}
```

**Add** three new tests after the two above:

```rust
#[tokio::test]
async fn run_command_buffer_only_stderr_gets_priority() {
    // stderr = 25 lines (> 20 cap) + stdout = 60 lines.
    // Expected: stderr_shown = 20, stdout_shown = 30 (50 - 20).
    let (_dir, ctx) = project_ctx().await;
    let stdout: String = (1..=60).map(|i| format!("out{i}\n")).collect();
    let stderr: String = (1..=25).map(|i| format!("err{i}\n")).collect();
    let id = ctx.output_buffer.store("cmd".into(), stdout, stderr, 0);
    let result = RunCommand
        .call(json!({ "command": format!("cat {}", id) }), &ctx)
        .await
        .expect("expected Ok");
    assert_eq!(result["stderr_shown"], 20usize, "stderr_shown: {:?}", result);
    assert_eq!(result["stderr_total"], 25usize, "stderr_total: {:?}", result);
    assert_eq!(result["stdout_shown"], 30usize, "stdout_shown: {:?}", result);
    assert_eq!(result["stdout_total"], 60usize, "stdout_total: {:?}", result);
    assert_eq!(result["truncated"], true);
}

#[tokio::test]
async fn run_command_buffer_only_short_stderr_gives_budget_to_stdout() {
    // stderr = 10 lines (< 20 cap) + stdout = 60 lines.
    // Expected: stderr_shown = 10, stdout_shown = 40 (50 - 10).
    let (_dir, ctx) = project_ctx().await;
    let stdout: String = (1..=60).map(|i| format!("out{i}\n")).collect();
    let stderr: String = (1..=10).map(|i| format!("err{i}\n")).collect();
    let id = ctx.output_buffer.store("cmd".into(), stdout, stderr, 0);
    let result = RunCommand
        .call(json!({ "command": format!("cat {}", id) }), &ctx)
        .await
        .expect("expected Ok");
    assert_eq!(result["stdout_shown"], 40usize, "stdout_shown: {:?}", result);
    assert_eq!(result["stdout_total"], 60usize, "stdout_total: {:?}", result);
    assert_eq!(result["truncated"], true);
}

#[tokio::test]
async fn run_command_buffer_only_within_limit_no_truncation_fields() {
    // combined ≤ 50 lines — must NOT add truncated/shown/total fields.
    let (_dir, ctx) = project_ctx().await;
    let stdout: String = (1..=30).map(|i| format!("out{i}\n")).collect();
    let stderr: String = (1..=15).map(|i| format!("err{i}\n")).collect();
    let id = ctx.output_buffer.store("cmd".into(), stdout, stderr, 0);
    let result = RunCommand
        .call(json!({ "command": format!("cat {}", id) }), &ctx)
        .await
        .expect("expected Ok");
    // exactly 45 lines — below threshold, buffer_only short path
    // (needs_summary returns false, so we fall through to the short-output branch)
    assert!(result.get("truncated").is_none(), "no truncated field: {:?}", result);
    assert!(result.get("stdout_shown").is_none(), "no stdout_shown: {:?}", result);
    assert!(result.get("output_id").is_none(), "no buffer ref: {:?}", result);
}
```

**Step 2: Run to verify tests fail**

Run: `cargo test run_command_buffer_only -- --nocapture 2>&1 | grep -E "FAILED|error"`
Expected: the two renamed tests fail (old tests are gone, new ones fail because the error path still exists).

**Step 3: Implement the truncation in `run_command_inner`**

In `src/tools/workflow.rs`, update the import at the top of `run_command_inner` (around L505):

```rust
use super::command_summary::{
    count_lines, detect_command_type, needs_summary, summarize_build_output, summarize_generic,
    summarize_test_output, truncate_lines, CommandType, SUMMARY_LINE_THRESHOLD,
};
```

Then **replace** the `if buffer_only { ... return Err(...) }` block (L604–624, the 21-line block starting with `if buffer_only {`) with:

```rust
if buffer_only {
    // Truncate to SUMMARY_LINE_THRESHOLD lines (stderr priority: up to 20,
    // remainder goes to stdout) and return inline. Do NOT create a new buffer
    // ref — that would cause an infinite query loop.
    const STDERR_BUDGET: usize = 20;
    let stderr_budget = STDERR_BUDGET.min(count_lines(&raw_stderr));
    let stdout_budget = SUMMARY_LINE_THRESHOLD - stderr_budget;

    let (stdout_out, stdout_shown, stdout_total) =
        truncate_lines(&raw_stdout, stdout_budget);
    let (stderr_out, stderr_shown, stderr_total) =
        truncate_lines(&raw_stderr, STDERR_BUDGET);

    let was_truncated =
        stdout_shown < stdout_total || stderr_shown < stderr_total;

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
            result["stderr_shown"] = json!(stderr_shown);
            result["stderr_total"] = json!(stderr_total);
        }
        let stderr_note = if stderr_total > 0 {
            format!(", stderr {stderr_shown}/{stderr_total}")
        } else {
            String::new()
        };
        result["hint"] = json!(format!(
            "Output capped at {SUMMARY_LINE_THRESHOLD} lines \
             (stdout {stdout_shown}/{stdout_total}{stderr_note}). \
             Narrow with: grep 'keyword' @ref, \
             sed -n '1,{}p' @ref",
            SUMMARY_LINE_THRESHOLD - 1,
        ));
    }
    return Ok(result);
}
```

**Step 4: Run the new tests**

Run: `cargo test run_command_buffer_only -- --nocapture`
Expected: all buffer_only tests pass.

**Step 5: Run the full test suite**

Run: `cargo test`
Expected: all tests pass. If `execute_shell_command_output_truncated` or similar tests now fail, investigate before proceeding.

**Step 6: Commit**

```
git add src/tools/workflow.rs src/tools/command_summary.rs
git commit -m "feat: truncate buffer-only run_command output inline instead of erroring

When a buffer-ref query (sed @cmd_A, grep @file_B) produces > 50 lines,
return the first 50 lines inline with truncation metadata instead of a
RecoverableError. Stderr gets priority (up to 20 lines); remaining budget
goes to stdout. No new buffer ref is created, preserving anti-loop invariant."
```

---

### Task 3: Update server instructions prompt

**Files:**
- Modify: `src/prompts/server_instructions.md`

**Step 1: Find the relevant section**

The anti-pattern note about buffer queries is around L144–146. Current text:

> 8. **Don't inline-pipe `run_command` output.** Run the command bare, then query the buffer in a follow-up: `cargo test` → `grep FAILED @cmd_id`. Never `cargo test 2>&1 | grep FAILED`.

**Step 2: Add a note about inline truncation**

After rule 8, add rule 9 (or extend rule 8 with a note):

Add after that bullet:

```markdown
9. **Buffer queries return ≤ 50 lines inline.** When querying a `@ref` (e.g. `grep pattern @cmd_id`), output above 50 lines is truncated inline — check `truncated`/`stdout_total` fields to see if you need a narrower query. Do NOT create pipes like `grep @ref | head` — run the targeted command directly.
```

**Step 3: Verify the file looks correct**

Run: `cargo test` (no Rust changes; just a sanity check)

**Step 4: Commit**

```
git add src/prompts/server_instructions.md
git commit -m "docs: note inline truncation behavior for buffer-only run_command"
```

---

### Task 4: Update output-buffers concept doc

**Files:**
- Modify: `docs/manual/src/concepts/output-buffers.md`

**Step 1: Read the current file**

Use `read_file("docs/manual/src/concepts/output-buffers.md")` to find the section about querying buffers.

**Step 2: Add a section after the targeted-query paragraph**

Find the sentence ending with "The exploration is transparent and auditable." (around L61) and add a new paragraph after it:

```markdown
**When a buffer query still returns too much, you get 50 lines inline.**
If `grep @ref` or `sed @ref` produces more than 50 lines, code-explorer
returns the first 50 lines inline with truncation metadata rather than
creating another `@ref` handle (which would cause an infinite loop).
The response includes `truncated: true`, `stdout_shown`/`stdout_total`
(and `stderr_shown`/`stderr_total` when stderr is non-empty) so the AI
can decide whether to refine further. Stderr lines are prioritised —
up to 20 stderr lines are shown, with the remaining budget going to stdout.
```

**Step 3: Commit**

```
git add docs/manual/src/concepts/output-buffers.md
git commit -m "docs: describe inline truncation for oversized buffer queries"
```

---

### Task 5: Update FEATURES.md one-liner

**Files:**
- Modify: `docs/FEATURES.md`

**Step 1: Find the current line**

Current text (around L12):
> - `run_command` output > 50 lines → stored in buffer, returns smart summary + `@cmd_xxxx` handle

**Step 2: Update to reflect truncation behavior**

Change to:
> - `run_command` output > 50 lines → stored in buffer, returns smart summary + `@cmd_xxxx` handle; buffer queries capped at 50 lines inline with `truncated`/`shown`/`total` metadata

**Step 3: Commit**

```
git add docs/FEATURES.md
git commit -m "docs: update FEATURES.md for buffer-only inline truncation"
```

---

### Task 6: Final verification

**Step 1: Full test suite + lint**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: all pass, zero warnings.

**Step 2: Smoke test manually (optional but recommended)**

Run: `cargo run -- start --project . 2>/dev/null &`
Then use Claude Code to run `run_command("seq 1 100")` to get a buffer ref, then `run_command("cat @cmd_xxxx")` — expect truncated inline output with `truncated: true`.

**Step 3: Done**

All tasks complete. The change is safe to push.
