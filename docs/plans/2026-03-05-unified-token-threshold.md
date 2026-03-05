# Unified Token-Based Buffering Threshold — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace three independent buffering thresholds (line-count, byte-count) with one
token-based threshold (`MAX_INLINE_TOKENS = 2_500`) applied uniformly across all tool output.

**Architecture:** A single helper `exceeds_inline_limit(text) -> bool` estimates tokens as
`text.len() / 4` and compares against `MAX_INLINE_TOKENS`. All buffering decision points
call this helper instead of their own threshold constants. `TOOL_OUTPUT_BUFFER_THRESHOLD`
is kept as a derived byte constant (`MAX_INLINE_TOKENS * 4`) for byte-budget arithmetic
in truncation code.

**Tech Stack:** Rust, serde_json

**Design doc:** `docs/plans/2026-03-05-unified-token-threshold-design.md`

---

### Task 1: Add `MAX_INLINE_TOKENS` and `exceeds_inline_limit()` to mod.rs

**Files:**
- Modify: `src/tools/mod.rs:33-36`

**Step 1: Add the new constant and helper, update the old constant**

Replace the current `TOOL_OUTPUT_BUFFER_THRESHOLD` definition at line 35:

```rust
// Old:
/// Compact JSON size above which tool output is routed through OutputBuffer.
pub(crate) const TOOL_OUTPUT_BUFFER_THRESHOLD: usize = 5_000;

// New:
/// Maximum estimated tokens for inline tool output.
/// Content exceeding this is buffered and summarized.
/// Token estimate: ~4 bytes per token.
pub(crate) const MAX_INLINE_TOKENS: usize = 2_500;

/// Byte equivalent of MAX_INLINE_TOKENS — used for byte-budget arithmetic
/// in truncation code (run_command buffer-only paths).
pub(crate) const TOOL_OUTPUT_BUFFER_THRESHOLD: usize = MAX_INLINE_TOKENS * 4;

/// Check whether content should be buffered based on estimated token count.
pub(crate) fn exceeds_inline_limit(text: &str) -> bool {
    text.len() / 4 > MAX_INLINE_TOKENS
}
```

**Step 2: Update `call_content` to use `exceeds_inline_limit`**

In the same file, `call_content` around line 271:

```rust
// Old:
if json.len() > TOOL_OUTPUT_BUFFER_THRESHOLD {

// New:
if exceeds_inline_limit(&json) {
```

**Step 3: Run `cargo build` to verify compilation**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles successfully (other files still reference old constants — that's fine,
`TOOL_OUTPUT_BUFFER_THRESHOLD` still exists as a derived constant).

**Step 4: Commit**

```bash
git add src/tools/mod.rs
git commit -m "refactor: add MAX_INLINE_TOKENS and exceeds_inline_limit helper"
```

---

### Task 2: Unify `read_file` buffering in file.rs (primary bug fix)

**Files:**
- Modify: `src/tools/file.rs:375-460`
- Modify: `src/tools/file_summary.rs:5`

**Step 1: Write the failing test — small JSONL file returns content inline**

Add in the `#[cfg(test)] mod tests` block at the end of `src/tools/file.rs`:

```rust
#[tokio::test]
async fn read_file_small_fat_file_returns_content_inline() {
    // Regression: a 10-line JSONL file with ~600 bytes/line (~6KB total,
    // ~1500 tokens) must return content inline, not just a file_id.
    let ctx = test_ctx().await;
    let dir = tempdir().unwrap();
    let file = dir.path().join("data.jsonl");
    let line = format!(
        "{{\"id\":1,\"data\":\"{}\"}}\n",
        "x".repeat(550) // ~570 bytes per line
    );
    let content: String = std::iter::repeat(line.as_str()).take(10).collect();
    assert!(
        content.len() > 5_000,
        "test file must exceed old 5KB threshold"
    );
    assert!(
        content.len() / 4 <= crate::tools::MAX_INLINE_TOKENS,
        "test file must be under new token threshold"
    );
    std::fs::write(&file, &content).unwrap();

    let result = ReadFile
        .call(json!({ "path": file.to_str().unwrap() }), &ctx)
        .await
        .unwrap();

    assert!(
        result.get("content").is_some(),
        "small file should have inline content; got: {}",
        serde_json::to_string_pretty(&result).unwrap()
    );
    assert!(
        result.get("file_id").is_none(),
        "small file should NOT be buffered; got: {}",
        serde_json::to_string_pretty(&result).unwrap()
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test read_file_small_fat_file_returns_content_inline -- --nocapture 2>&1 | tail -20`
Expected: FAIL — currently returns `file_id` without `content`.

**Step 3: Write the failing test — large file is still buffered**

```rust
#[tokio::test]
async fn read_file_large_token_count_is_buffered() {
    // A file exceeding MAX_INLINE_TOKENS (~2500 tokens, ~10KB) must still
    // be buffered with a structural summary.
    let ctx = test_ctx().await;
    let dir = tempdir().unwrap();
    let file = dir.path().join("big.py");
    // 150 lines × 100 bytes = 15KB ≈ 3750 tokens → exceeds limit
    let line = format!("# {}\n", "x".repeat(95));
    let content: String = std::iter::repeat(line.as_str()).take(150).collect();
    assert!(
        content.len() / 4 > crate::tools::MAX_INLINE_TOKENS,
        "test file must exceed token threshold"
    );
    std::fs::write(&file, &content).unwrap();

    let result = ReadFile
        .call(json!({ "path": file.to_str().unwrap() }), &ctx)
        .await
        .unwrap();

    assert!(
        result.get("file_id").is_some(),
        "large file should be buffered; got: {}",
        serde_json::to_string_pretty(&result).unwrap()
    );
}
```

**Step 4: Run test to verify it passes (existing behavior still works)**

Run: `cargo test read_file_large_token_count_is_buffered -- --nocapture 2>&1 | tail -10`
Expected: PASS (file is 15KB, exceeds both old and new thresholds).

**Step 5: Implement the fix — unify the two gates in read_file**

In `src/tools/file.rs`, replace the line-count gate at L375 and remove the proactive
byte-size gate at L441:

```rust
// L375 — Old:
if !has_partial_range && line_count > crate::tools::file_summary::FILE_BUFFER_THRESHOLD {

// L375 — New:
if !has_partial_range && crate::tools::exceeds_inline_limit(&text) {
```

Then **delete the entire proactive buffering block** at L441-L453 (the `json_estimate`
check). The `else` branch below it (returning inline content) becomes the only remaining
path. After deletion, the code should flow directly from the exploring-mode overflow
check into the final inline-content return:

```rust
        // (after the exploring-mode overflow block)
        } else {
            let mut result = json!({ "content": text, "total_lines": total_lines });
            if source_tag != "project" {
                result["source"] = json!(source_tag);
            }
            Ok(result)
        }
```

**Step 6: Remove `FILE_BUFFER_THRESHOLD` from `file_summary.rs`**

In `src/tools/file_summary.rs:5`, delete:

```rust
pub const FILE_BUFFER_THRESHOLD: usize = 200;
```

**Step 7: Fix heading-extraction buffering at L269**

```rust
// Old:
if result.content.lines().count() > crate::tools::file_summary::FILE_BUFFER_THRESHOLD {

// New:
if crate::tools::exceeds_inline_limit(&result.content) {
```

**Step 8: Run both tests to verify**

Run: `cargo test read_file_small_fat_file read_file_large_token_count -- --nocapture 2>&1 | tail -20`
Expected: both PASS.

**Step 9: Update existing test `read_file_large_content_returns_file_id_not_inline`**

This test at L4314 creates a 60-line × 100-byte file (6KB) and asserts it's buffered.
Under the new threshold (10KB), 6KB is inline. Update to create a file that exceeds 10KB:

```rust
// Old:
let line = "x".repeat(100);
let lines: Vec<&str> = std::iter::repeat(line.as_str()).take(60).collect();

// New — 120 lines × 100 bytes = 12KB ≈ 3000 tokens > 2500:
let line = "x".repeat(100);
let lines: Vec<&str> = std::iter::repeat(line.as_str()).take(120).collect();
```

**Step 10: Update test `read_file_caps_large_file_in_exploring_mode` if needed**

This test at L1699 uses a 300-line file. 300 lines × ~8 bytes/line ≈ 2.4KB — might now
be inline. Check: the test generates lines like `"line 1\n"` to `"line 300\n"`,
averaging ~9 bytes/line → ~2.7KB → ~675 tokens. Under new threshold this would NOT be
buffered. Update to ensure it exceeds 10KB:

```rust
// Old:
let content: String = (1..=300).map(|i| format!("line {}\n", i)).collect();

// New — use longer lines to exceed token limit:
let content: String = (1..=300).map(|i| format!("line {:04} {}\n", i, "x".repeat(30))).collect();
```

Verify the content exceeds 10KB with a quick sanity assertion at the start of the test.

**Step 11: Run all read_file tests**

Run: `cargo test --lib tools::file::tests -- --nocapture 2>&1 | tail -20`
Expected: all pass.

**Step 12: Commit**

```bash
git add src/tools/file.rs src/tools/file_summary.rs
git commit -m "fix: unify read_file buffering to token-based threshold

Replace FILE_BUFFER_THRESHOLD (200 lines) and TOOL_OUTPUT_BUFFER_THRESHOLD
(5KB) with exceeds_inline_limit() (~2500 tokens / ~10KB).

A 10-line JSONL file with fat lines now returns content inline instead
of a content-free buffer-only response."
```

---

### Task 3: Unify buffer-ref reading in file.rs

**Files:**
- Modify: `src/tools/file.rs:108,155,163`

**Step 1: Replace byte-size checks with `exceeds_inline_limit`**

At line 108 (json_path extraction from `@tool_*` ref):
```rust
// Old:
let mut result = if content.len() > crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD {

// New:
let mut result = if crate::tools::exceeds_inline_limit(&content) {
```

At line 155 (explicit line range from `@file_*`/`@cmd_*` ref):
```rust
// Old:
if content.len() > crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD {

// New:
if crate::tools::exceeds_inline_limit(&content) {
```

At line 163 (full content from `@file_*`/`@cmd_*` ref):
```rust
// Old:
if text.len() > crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD {

// New:
if crate::tools::exceeds_inline_limit(&text) {
```

**Step 2: Run read_file tests**

Run: `cargo test --lib tools::file::tests 2>&1 | tail -10`
Expected: all pass.

**Step 3: Commit**

```bash
git add src/tools/file.rs
git commit -m "refactor: unify buffer-ref reading to token-based threshold"
```

---

### Task 4: Unify `needs_summary` in command_summary.rs

**Files:**
- Modify: `src/tools/command_summary.rs:15,180-183`

**Step 1: Update `needs_summary` to use token estimation**

```rust
// Old:
pub fn needs_summary(stdout: &str, stderr: &str) -> bool {
    let total_lines = count_lines(stdout) + count_lines(stderr);
    total_lines > SUMMARY_LINE_THRESHOLD
}

// New:
pub fn needs_summary(stdout: &str, stderr: &str) -> bool {
    crate::tools::exceeds_inline_limit(&format!("{stdout}{stderr}"))
}
```

**Step 2: Remove `SUMMARY_LINE_THRESHOLD`**

Delete from `src/tools/command_summary.rs:15`:

```rust
/// Minimum total line count (stdout + stderr) before summarization kicks in.
pub(crate) const SUMMARY_LINE_THRESHOLD: usize = 50;
```

**Step 3: Update `workflow.rs` unfiltered-output truncation**

At L1027-1030, the tee-capture path uses `SUMMARY_LINE_THRESHOLD` to truncate
stored unfiltered output. Replace with `exceeds_inline_limit`:

```rust
// Old:
let (stored, truncated) = if line_count > SUMMARY_LINE_THRESHOLD {
    let capped = content
        .lines()
        .take(SUMMARY_LINE_THRESHOLD)
        .collect::<Vec<_>>()
        .join("\n");
    (capped, true)

// New:
let (stored, truncated) = if crate::tools::exceeds_inline_limit(&content) {
    // Truncate to roughly MAX_INLINE_TOKENS worth of lines
    let mut byte_budget = crate::tools::MAX_INLINE_TOKENS * 4;
    let capped: String = content
        .lines()
        .take_while(|line| {
            if byte_budget == 0 { return false; }
            byte_budget = byte_budget.saturating_sub(line.len() + 1);
            true
        })
        .collect::<Vec<_>>()
        .join("\n");
    (capped, true)
```

**Step 4: Remove `SUMMARY_LINE_THRESHOLD` import from workflow.rs tests**

At L1247:
```rust
// Old:
use crate::tools::command_summary::{BUFFER_QUERY_INLINE_CAP, SUMMARY_LINE_THRESHOLD};

// New:
use crate::tools::command_summary::BUFFER_QUERY_INLINE_CAP;
```

Remove any other references to `SUMMARY_LINE_THRESHOLD` in test code. Search for them
with `grep -n SUMMARY_LINE_THRESHOLD src/tools/workflow.rs` and update each test to
use `crate::tools::MAX_INLINE_TOKENS` or `crate::tools::exceeds_inline_limit` instead.

**Step 5: Run all workflow and command_summary tests**

Run: `cargo test --lib tools::workflow::tests tools::command_summary::tests 2>&1 | tail -20`
Expected: all pass.

**Step 6: Commit**

```bash
git add src/tools/command_summary.rs src/tools/workflow.rs
git commit -m "refactor: unify run_command summarization to token-based threshold

Replace SUMMARY_LINE_THRESHOLD (50 lines) with exceeds_inline_limit()."
```

---

### Task 5: Unify github.rs `maybe_buffer`

**Files:**
- Modify: `src/tools/github.rs:49`

**Step 1: Update `maybe_buffer`**

```rust
// Old:
pub(crate) fn maybe_buffer(content: String, tool_name: &str, ctx: &ToolContext) -> Value {
    if content.len() > TOOL_OUTPUT_BUFFER_THRESHOLD {

// New:
pub(crate) fn maybe_buffer(content: String, tool_name: &str, ctx: &ToolContext) -> Value {
    if crate::tools::exceeds_inline_limit(&content) {
```

Remove the `TOOL_OUTPUT_BUFFER_THRESHOLD` import if it's no longer used (check line 6):

```rust
// Check if TOOL_OUTPUT_BUFFER_THRESHOLD is still referenced in github.rs tests.
// If only in tests, keep the import. If nowhere, remove it.
```

**Step 2: Update github tests referencing old threshold**

The test at L1302 (`assert!(diff.len() < TOOL_OUTPUT_BUFFER_THRESHOLD)`) and L1314-1315
create content relative to `TOOL_OUTPUT_BUFFER_THRESHOLD`. Update to use
`MAX_INLINE_TOKENS * 4` or just hardcode values that make sense for the new threshold.

**Step 3: Run github tests**

Run: `cargo test --lib tools::github::tests 2>&1 | tail -10`
Expected: all pass.

**Step 4: Commit**

```bash
git add src/tools/github.rs
git commit -m "refactor: unify github maybe_buffer to token-based threshold"
```

---

### Task 6: Final verification and cleanup

**Files:**
- All modified files

**Step 1: Search for any remaining references to old constants**

Run: `grep -rn 'FILE_BUFFER_THRESHOLD\|SUMMARY_LINE_THRESHOLD' src/`
Expected: zero matches (both constants fully removed).

Run: `grep -rn 'TOOL_OUTPUT_BUFFER_THRESHOLD' src/`
Expected: only in `mod.rs` (definition as derived constant), `workflow.rs` (byte-budget
arithmetic), and any test assertions. No decision-point uses outside of byte-budget math.

**Step 2: Run full test suite**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test 2>&1 | tail -20`
Expected: all ~932 tests pass, no warnings, clean format.

**Step 3: Update comments referencing old thresholds**

Search for stale comments mentioning "200 lines", "5 KB", "5,000", "50 lines" in the
context of buffering decisions. Update to reference `MAX_INLINE_TOKENS` / `~2500 tokens`.

Run: `grep -rn '200 lines\|5.KB\|5,000\|50 lines' src/tools/ | grep -i 'buffer\|threshold\|summary'`

Fix any hits.

**Step 4: Final test run and commit**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: all green.

```bash
git add -A
git commit -m "chore: clean up stale threshold references in comments"
```
