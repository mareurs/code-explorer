# Design: Ack-Handle Pattern for Dangerous Commands

**Date:** 2026-03-01
**Status:** Approved

## Problem

When `run_command` detects a dangerous command, it returns a `RecoverableError` asking the agent
to re-run with `acknowledge_risk: true`. The agent must repeat the full command string a second
time. For long commands this wastes tokens; it also feels redundant — the agent already sent the
command once.

## Solution

Replace the error-and-repeat flow with a **handle-based acknowledgment**. The first call stores
the dangerous command in `OutputBuffer` and returns an opaque `@ack_*` handle. The second call is
just the handle — no repeated command string, no extra params.

```
// Round 1
run_command("rm -rf ./dist", cwd: "frontend/", timeout_secs: 10)
→ { "pending_ack": "@ack_a1b2c3d4",
    "reason": "rm with --force or --recursive",
    "hint": "run_command(\"@ack_a1b2c3d4\") to execute" }

// Round 2
run_command("@ack_a1b2c3d4")
→ executes "rm -rf ./dist" in cwd "frontend/" with timeout 10
```

The handle is self-contained: it stores the command, `cwd`, and `timeout_secs` from the original
call. The agent provides no additional parameters in the ack call.

## Design

### New type: `PendingAckCommand`

```rust
struct PendingAckCommand {
    command: String,
    cwd: Option<String>,
    timeout_secs: u64,
}
```

### OutputBuffer changes

A second small map is added inside `OutputBuffer`, alongside the existing content LRU:

```rust
pending_acks: LruCache<String, PendingAckCommand>  // cap: 20
```

Two new public methods:

```rust
/// Store a dangerous command with its execution context. Returns "@ack_<id>".
pub fn store_dangerous(
    &self,
    command: String,
    cwd: Option<String>,
    timeout_secs: u64,
) -> String

/// Retrieve a stored pending ack. Returns None if evicted or unknown.
/// Does not consume the entry — LRU evicts naturally.
pub fn get_dangerous(&self, handle: &str) -> Option<PendingAckCommand>
```

`resolve_refs()` gains a guard: if a `@ack_*` token appears in interpolation position (i.e., not
as the sole command), return a `RecoverableError`:

```
"ack handles cannot be interpolated — use run_command(\"@ack_XYZ\") directly"
```

### `RunCommand::call` changes

Before calling `resolve_refs()`, add an early dispatch check:

```rust
if looks_like_ack_handle(command) {
    let stored = ctx.output_buffer
        .get_dangerous(command)
        .ok_or_else(|| RecoverableError::with_hint(
            "ack handle expired or unknown",
            "Re-run the original command to get a fresh handle.",
        ))?;
    return run_command_inner(
        &stored.command,
        &stored.command,
        stored.timeout_secs,
        /*acknowledge_risk=*/ true,
        stored.cwd.as_deref(),
        /*buffer_only=*/ false,
        &root,
        &security,
        ctx,
    ).await;
}
```

`looks_like_ack_handle(s)` matches the regex `^@ack_[a-z0-9]+$`.

### `run_command_inner` changes

At Step 2 (dangerous command check), replace the `return Err(RecoverableError)` with:

```rust
let handle = ctx.output_buffer.store_dangerous(
    resolved_command.to_string(),
    cwd_param.map(str::to_string),
    timeout_secs,
);
return Ok(json!({
    "pending_ack": handle,
    "reason": reason,
    "hint": format!("run_command(\"{handle}\") to execute")
}));
```

The response shape changes from an error (`{"error":"...","hint":"..."}`) to a structured success.
The `isError` field remains `false` in both cases.

### Backward compatibility

`acknowledge_risk: true` continues to work exactly as before — it bypasses the dangerous check
entirely without any handle indirection. No existing callers break.

The `acknowledge_risk` parameter description in `input_schema()` is updated to note that `@ack_*`
handles are the preferred alternative.

## Token comparison

For a 100-character command:

| | Old flow | New flow |
|---|---|---|
| Round 1 request | 100 chars | 100 chars |
| Round 1 response | ~50 chars (error) | ~80 chars (handle + reason) |
| Round 2 request | ~120 chars (command + flag) | ~20 chars (handle only) |
| **Total** | **~270 chars** | **~200 chars** |

Savings grow with command length. A 500-char command saves ~500 chars net.

## Handle lifetime

Handles are stored in an `LruCache` capped at 20. Eviction is implicit — oldest handles are
dropped when the cap is exceeded. If a handle is evicted before the agent acks it, the error
message directs the agent to re-run the original command.

A cap of 20 is generous — in practice an agent rarely has more than 1–2 dangerous commands
pending simultaneously.

## Tests

1. Dangerous command → response contains `pending_ack` handle (not an error)
2. `run_command("@ack_xyz")` → executes stored command with stored `cwd` and `timeout_secs`
3. Unknown or evicted handle → `RecoverableError` with hint to re-run original
4. `run_command("grep foo @ack_xyz")` → `RecoverableError` (interpolation blocked)
5. `acknowledge_risk: true` → still bypasses check directly (no handle involved)
6. Handle cap: 21st dangerous command evicts the oldest handle

## Files changed

- `src/tools/output_buffer.rs` — `PendingAckCommand`, `store_dangerous`, `get_dangerous`,
  `resolve_refs` guard
- `src/tools/workflow.rs` — early dispatch in `RunCommand::call`, store instead of error in
  `run_command_inner`
