# Ack-Handle Pattern for Dangerous Commands — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the error-and-repeat flow for dangerous `run_command` calls with a two-phase
`@ack_*` handle protocol that saves tokens and eliminates the need to repeat the full command.

**Architecture:** `OutputBuffer` gains a second small LRU map (`pending_acks`) for
`PendingAckCommand` structs. `run_command_inner` stores the dangerous command and returns a handle
instead of an error. `RunCommand::call` detects `@ack_*` handles early, looks up the stored
command, and executes it with all original parameters.

**Tech Stack:** Rust, `std::collections::HashMap`, existing `OutputBuffer` / `BufferInner` pattern,
`run_command_inner` in `src/tools/workflow.rs`.

**Design doc:** `docs/plans/2026-03-01-ack-handle-design.md`

---

### Task 1: Add `PendingAckCommand` struct and `pending_acks` storage to `OutputBuffer`

**Files:**
- Modify: `src/tools/output_buffer.rs:17-41`

This task only adds data types and fields — no new methods yet.

**Step 1: Add `PendingAckCommand` struct after `BufferEntry` (line 25)**

In `src/tools/output_buffer.rs`, after the closing `}` of `BufferEntry` (currently line 25),
insert:

```rust
/// A dangerous command held pending agent acknowledgment.
#[derive(Debug, Clone)]
pub struct PendingAckCommand {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: u64,
}
```

**Step 2: Add `pending_acks` fields to `BufferInner` (lines 36-42)**

Inside `struct BufferInner { … }`, add three new fields:

```rust
struct BufferInner {
    entries: HashMap<String, BufferEntry>,
    order: Vec<String>,
    max_entries: usize,
    counter: u64,
    // --- pending-ack store ---
    pending_acks: HashMap<String, PendingAckCommand>,
    pending_order: Vec<String>,
    max_pending: usize,
}
```

**Step 3: Initialize new fields in `OutputBuffer::new` (lines 46-55)**

Inside `BufferInner { … }` initialization in `new()`, add:

```rust
pending_acks: HashMap::new(),
pending_order: Vec::new(),
max_pending: 20,
```

**Step 4: Verify it compiles**

```bash
cargo build 2>&1 | head -20
```

Expected: compiles clean (no new tests yet).

**Step 5: Commit**

```
git add src/tools/output_buffer.rs
git commit -m "feat(output_buffer): add PendingAckCommand struct and pending_acks store"
```

---

### Task 2: Add `store_dangerous` and `get_dangerous` methods + tests

**Files:**
- Modify: `src/tools/output_buffer.rs` (add methods, add tests)

**Step 1: Write the failing tests first**

In the `#[cfg(test)]` module at the bottom of `src/tools/output_buffer.rs`, add:

```rust
#[test]
fn store_dangerous_returns_ack_handle() {
    let buf = OutputBuffer::new(10);
    let handle = buf.store_dangerous("rm -rf /dist".to_string(), Some("frontend/".to_string()), 30);
    assert!(handle.starts_with("@ack_"), "handle should start with @ack_, got: {handle}");
}

#[test]
fn get_dangerous_returns_stored_command() {
    let buf = OutputBuffer::new(10);
    let handle = buf.store_dangerous("rm -rf /dist".to_string(), Some("frontend/".to_string()), 10);
    let cmd = buf.get_dangerous(&handle).expect("should find stored command");
    assert_eq!(cmd.command, "rm -rf /dist");
    assert_eq!(cmd.cwd, Some("frontend/".to_string()));
    assert_eq!(cmd.timeout_secs, 10);
}

#[test]
fn get_dangerous_returns_none_for_unknown_handle() {
    let buf = OutputBuffer::new(10);
    assert!(buf.get_dangerous("@ack_deadbeef").is_none());
}

#[test]
fn pending_acks_lru_eviction() {
    let buf = OutputBuffer::new(10);
    // Fill beyond the pending cap (20)
    let mut handles = Vec::new();
    for i in 0..21u64 {
        handles.push(buf.store_dangerous(format!("cmd_{}", i), None, 30));
    }
    // First handle should be evicted
    assert!(buf.get_dangerous(&handles[0]).is_none(), "oldest ack should be evicted");
    // Last handle should still be present
    assert!(buf.get_dangerous(&handles[20]).is_some(), "newest ack should survive");
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test store_dangerous get_dangerous pending_acks_lru 2>&1 | tail -20
```

Expected: compile error — `store_dangerous` and `get_dangerous` do not exist yet.

**Step 3: Add `store_dangerous` method**

After the `store_tool` method (around line 168), add:

```rust
/// Store a dangerous command pending acknowledgment.
///
/// Returns an opaque `@ack_<8hex>` handle. The handle carries the full
/// execution context so the ack call needs no extra parameters.
pub fn store_dangerous(
    &self,
    command: String,
    cwd: Option<String>,
    timeout_secs: u64,
) -> String {
    let mut inner = self.inner.lock().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    inner.counter = inner.counter.wrapping_add(1);
    let id = format!("@ack_{:08x}", now.wrapping_add(inner.counter) as u32);

    // Evict oldest if at capacity
    if inner.pending_acks.len() >= inner.max_pending {
        if let Some(oldest) = inner.pending_order.first().cloned() {
            inner.pending_order.remove(0);
            inner.pending_acks.remove(&oldest);
        }
    }

    inner
        .pending_acks
        .insert(id.clone(), PendingAckCommand { command, cwd, timeout_secs });
    inner.pending_order.push(id.clone());
    id
}
```

**Step 4: Add `get_dangerous` method**

Immediately after `store_dangerous`:

```rust
/// Retrieve a stored pending ack by handle.
///
/// Does not consume the entry — LRU eviction handles cleanup.
/// Returns `None` if the handle is unknown or has been evicted.
pub fn get_dangerous(&self, handle: &str) -> Option<PendingAckCommand> {
    let inner = self.inner.lock().unwrap();
    inner.pending_acks.get(handle).cloned()
}
```

**Step 5: Run tests to verify they pass**

```bash
cargo test store_dangerous get_dangerous pending_acks_lru 2>&1 | tail -20
```

Expected: 4 tests pass.

**Step 6: Commit**

```
git add src/tools/output_buffer.rs
git commit -m "feat(output_buffer): add store_dangerous / get_dangerous for ack-handle protocol"
```

---

### Task 3: Guard `@ack_*` refs in `resolve_refs` + test

**Files:**
- Modify: `src/tools/output_buffer.rs:179-255`

If the agent accidentally writes `grep foo @ack_abc123`, `resolve_refs` must reject it — ack
handles are not content buffers and cannot be interpolated.

**Step 1: Write the failing test**

In the test module, add:

```rust
#[test]
fn resolve_refs_rejects_ack_handle_interpolation() {
    let buf = OutputBuffer::new(10);
    let handle = buf.store_dangerous("rm -rf /dist".to_string(), None, 30);
    // Trying to use the ack handle as a content ref in a command should error
    let result = buf.resolve_refs(&format!("grep pattern {handle}"));
    assert!(result.is_err(), "interpolating an @ack_ handle should return an error");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("ack handle"), "error should mention 'ack handle', got: {msg}");
}
```

**Step 2: Run to verify it fails**

```bash
cargo test resolve_refs_rejects_ack_handle 2>&1 | tail -10
```

Expected: FAIL — currently `resolve_refs` tries to look up `@ack_*` as a content ref and returns
"buffer reference not found", which is the wrong error message.

**Step 3: Add the guard in `resolve_refs`**

`resolve_refs` builds its regex at line 180:
```rust
let re = Regex::new(r"@(?:cmd|file|tool)_[0-9a-f]{8}(\.err)?").expect("valid regex");
```

Add an `@ack_` guard check **before** this regex line:

```rust
// Guard: @ack_* handles are for deferred execution, not content interpolation.
let ack_re = Regex::new(r"@ack_[0-9a-f]{8}").expect("valid regex");
if ack_re.is_match(command) {
    return Err(RecoverableError::with_hint(
        "ack handle cannot be used for interpolation",
        "Use run_command(\"@ack_<id>\") directly to execute a pending acknowledgment.",
    )
    .into());
}
```

**Step 4: Run to verify it passes**

```bash
cargo test resolve_refs_rejects_ack_handle 2>&1 | tail -10
```

Expected: PASS.

**Step 5: Run full test suite to check for regressions**

```bash
cargo test 2>&1 | tail -20
```

Expected: all existing tests still pass.

**Step 6: Commit**

```
git add src/tools/output_buffer.rs
git commit -m "feat(output_buffer): reject @ack_* handles in resolve_refs interpolation"
```

---

### Task 4: Change `run_command_inner` Step 2 to return a handle instead of an error

**Files:**
- Modify: `src/tools/workflow.rs:565-576`

**Step 1: Write the failing integration test**

In the `#[cfg(test)]` module at the bottom of `src/tools/workflow.rs`, add a unit test that calls
`run_command_inner` directly with a dangerous command:

```rust
#[tokio::test]
async fn dangerous_command_returns_ack_handle() {
    use crate::tools::output_buffer::OutputBuffer;
    use std::sync::Arc;
    // Build a minimal ToolContext — just enough for the dangerous-check path.
    // (Copy the pattern used in other workflow tests in this file.)
    let buf = Arc::new(OutputBuffer::new(10));
    // ... set up ctx with a real or mock agent ...

    // For now, assert on the response shape by examining the returned JSON.
    // The exact setup depends on existing test helpers in this module.
    // If no helper exists, add a minimal one (see existing tests for patterns).
    let result = run_command_inner(
        "rm -rf /dist",
        "rm -rf /dist",
        30,
        false,          // acknowledge_risk = false
        None,
        false,          // buffer_only = false
        Path::new("/tmp"),
        &Default::default(),
        &ctx,
    )
    .await
    .expect("should return Ok, not Err");

    assert!(result.get("pending_ack").is_some(), "should have pending_ack key");
    assert!(result["pending_ack"].as_str().unwrap().starts_with("@ack_"));
    assert!(result.get("reason").is_some());
    assert!(result.get("hint").is_some());
}
```

> Note: Look at existing tests in the `tests` module (line 847+) to see how `ToolContext` is
> constructed for unit tests. Replicate that pattern exactly.

**Step 2: Run the test to verify it fails**

```bash
cargo test dangerous_command_returns_ack_handle 2>&1 | tail -20
```

Expected: FAIL — currently returns `Err(RecoverableError)`.

**Step 3: Replace the error return with handle storage**

In `run_command_inner`, Step 2 currently reads (lines 566–576):

```rust
// --- Step 2: Dangerous command speed bump ---
if !buffer_only && !acknowledge_risk {
    if let Some(reason) = is_dangerous_command(resolved_command, security) {
        return Err(super::RecoverableError::with_hint(
            format!("dangerous command blocked: {}", reason),
            "Re-run with acknowledge_risk: true if you are certain this is safe.",
        )
        .into());
    }
}
```

Replace the inner `return Err(…)` block with:

```rust
if let Some(reason) = is_dangerous_command(resolved_command, security) {
    let handle = ctx.output_buffer.store_dangerous(
        resolved_command.to_string(),
        cwd_param.map(str::to_string),
        timeout_secs,
    );
    return Ok(serde_json::json!({
        "pending_ack": handle,
        "reason": reason,
        "hint": format!("run_command(\"{handle}\") to execute")
    }));
}
```

**Step 4: Run the test to verify it passes**

```bash
cargo test dangerous_command_returns_ack_handle 2>&1 | tail -20
```

Expected: PASS.

**Step 5: Run full suite**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass. (Any test that previously asserted `is_dangerous_command` returns `Err`
will need to be updated to assert the new `pending_ack` shape.)

**Step 6: Commit**

```
git add src/tools/workflow.rs
git commit -m "feat(run_command): store dangerous command as ack handle instead of returning error"
```

---

### Task 5: Add early dispatch in `RunCommand::call` + tests

**Files:**
- Modify: `src/tools/workflow.rs:493-523`

**Step 1: Write the failing integration test**

In the test module, add:

```rust
#[tokio::test]
async fn ack_handle_executes_stored_command() {
    // 1. Store a safe-enough command via store_dangerous (bypassing detection)
    let buf = Arc::new(OutputBuffer::new(10));
    let handle = buf.store_dangerous("echo hello".to_string(), None, 30);

    // 2. Call RunCommand::call with the handle as the command string
    let tool = RunCommand;
    let input = serde_json::json!({ "command": handle });
    // ... set up ctx with buf injected ...
    let result = tool.call(input, &ctx).await.expect("ack call should succeed");

    // 3. Assert the output contains "hello"
    assert!(result["stdout"].as_str().unwrap_or("").contains("hello"));
}

#[tokio::test]
async fn ack_handle_unknown_returns_recoverable_error() {
    let tool = RunCommand;
    let input = serde_json::json!({ "command": "@ack_deadbeef" });
    // ... set up ctx ...
    let result = tool.call(input, &ctx).await;
    // Should be Ok(json with "error" key) — RecoverableError
    let val = result.expect("RecoverableError is Ok");
    assert!(val.get("error").is_some());
    assert!(val["error"].as_str().unwrap().contains("expired"));
}
```

**Step 2: Run to verify they fail**

```bash
cargo test ack_handle_executes_stored ack_handle_unknown 2>&1 | tail -20
```

Expected: FAIL — `RunCommand::call` currently passes `@ack_*` to `resolve_refs`, which rejects it
(after Task 3) with an "ack handle cannot be interpolated" error.

**Step 3: Add a helper function `looks_like_ack_handle`**

Near `looks_like_file_read` (around line 533), add:

```rust
/// Returns true when `command` is a bare `@ack_<8hex>` handle.
fn looks_like_ack_handle(command: &str) -> bool {
    let s = command.trim();
    if !s.starts_with("@ack_") {
        return false;
    }
    let suffix = &s[5..]; // after "@ack_"
    suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit())
}
```

**Step 4: Add early dispatch at the top of `RunCommand::call`**

In `RunCommand::call`, before `resolve_refs`, insert:

```rust
// --- Early dispatch: @ack_* handle ---
if looks_like_ack_handle(command) {
    let stored = ctx
        .output_buffer
        .get_dangerous(command)
        .ok_or_else(|| {
            super::RecoverableError::with_hint(
                "ack handle expired or unknown",
                "Re-run the original command to get a fresh handle.",
            )
        })?;
    return run_command_inner(
        &stored.command,
        &stored.command,
        stored.timeout_secs,
        true, // acknowledge_risk
        stored.cwd.as_deref(),
        false, // buffer_only
        &root,
        &security,
        ctx,
    )
    .await;
}
```

Note: `root` and `security` must be initialized before this block — move their initialization
above the early dispatch if they are currently below.

**Step 5: Run the tests to verify they pass**

```bash
cargo test ack_handle_executes_stored ack_handle_unknown 2>&1 | tail -20
```

Expected: PASS.

**Step 6: Run full suite**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass.

**Step 7: Commit**

```
git add src/tools/workflow.rs
git commit -m "feat(run_command): early dispatch @ack_* handles in RunCommand::call"
```

---

### Task 6: Update schema description + cleanup + final verification

**Files:**
- Modify: `src/tools/workflow.rs:468-492` (input_schema)

**Step 1: Update `acknowledge_risk` description in `input_schema`**

Find the `acknowledge_risk` property (around line 486):

```rust
"acknowledge_risk": {
    "type": "boolean",
    "description": "Bypass speed bump for dangerous commands. Required after a destructive command is detected."
}
```

Replace with:

```rust
"acknowledge_risk": {
    "type": "boolean",
    "description": "Bypass dangerous-command check directly. Prefer the @ack_* handle protocol: \
                    when a dangerous command is detected, re-run with the returned handle \
                    (e.g. run_command(\"@ack_a1b2c3d4\")) instead of repeating the full command."
}
```

**Step 2: Run clippy**

```bash
cargo clippy -- -D warnings 2>&1 | tail -20
```

Expected: clean.

**Step 3: Run fmt**

```bash
cargo fmt
```

**Step 4: Run full test suite one final time**

```bash
cargo test 2>&1 | tail -30
```

Expected: all tests pass.

**Step 5: Final commit**

```
git add src/tools/workflow.rs
git commit -m "docs(run_command): update acknowledge_risk schema to reference @ack_* protocol"
```
