# Interactive Sessions — Design

**Date:** 2026-03-01
**Status:** Approved, ready for implementation

## Problem

`run_command` currently calls `.output()` — a blocking wait for process exit. Commands that produce output but wait for stdin (REPLs, debuggers, confirmation prompts) hang until the configured timeout, then fail. There is no way for the agent to interact with a running process.

## Goals

- Allow the agent to start an interactive process (Python REPL, pdb, confirmation prompts)
- Let the agent send input lines and receive the resulting output
- Let the agent cancel the session at any time, releasing all resources

## Non-Goals

- PTY allocation (vim, less, htop, ncurses apps are not supported)
- Auto-detection of interactive mode (explicit flag required)
- Concurrent sends to the same session

## Scope: Use Cases

1. **REPLs** — `python3 -i`, `node`, `irb` (line-based, return output after each expression)
2. **Debuggers** — `python3 -m pdb script.py`, `dlv debug`, `gdb` (stateful prompt-driven)
3. **Confirmation flows** — `npm install`, `apt-get`, interactive git flows (y/n or choice prompts)

## Tool API

### `run_command` (extended)

New optional param: `"interactive": boolean` (default `false`).

When `interactive: true`, the command is spawned with piped stdin/stdout/stderr. After `settle_ms` of quiet initial output, a session handle is returned instead of waiting for exit.

**Input:**
```json
{
  "command": "python3 -i",
  "interactive": true,
  "settle_ms": 150
}
```

**Response:**
```json
{
  "session_id": "@ses_a1b2c3d4",
  "output": "Python 3.11.0 (default, ...)\n>>> ",
  "is_alive": true,
  "exited_with": null,
  "hint": "Use session_send to interact, session_cancel to terminate"
}
```

If the process exits before the initial settle: returns `is_alive: false`, `exited_with: <code>`, no `session_id`.

### `session_send` (new)

Sends a line of input to a running session and waits for the response to settle.

**Input:**
```json
{
  "session_id": "@ses_a1b2c3d4",
  "input": "1 + 1",
  "settle_ms": 150,
  "timeout_secs": 10
}
```

**Response (normal):**
```json
{
  "output": "2\n>>> ",
  "is_alive": true,
  "exited_with": null
}
```

**Response (process exited during/after send):**
```json
{
  "output": "Traceback (most recent call last):\n...\n",
  "is_alive": false,
  "exited_with": 1,
  "hint": "Session has ended. Call session_cancel to free resources."
}
```

**Response (settle timeout):**
```json
{
  "output": "...",
  "is_alive": true,
  "timed_out": true,
  "hint": "Output may be incomplete. Increase timeout_secs or reduce settle_ms."
}
```

### `session_cancel` (new)

Kills the process and frees all resources.

**Input:**
```json
{ "session_id": "@ses_a1b2c3d4" }
```

**Response:** `"ok"` (even if process already exited)

## Architecture

### Session Store

```
SessionStore: Arc<tokio::sync::Mutex<SessionStoreInner>>
  └─ sessions: HashMap<String, Session>      — active sessions
  └─ counter: u64                            — for ID generation
  └─ max_sessions: 5                         — cap to prevent fd exhaustion

Session:
  ├─ id: String                              — "@ses_<8hex>"
  ├─ stdin: ChildStdin                       — write input here
  ├─ child: Child                            — kill / try_wait
  ├─ output: Arc<std::sync::Mutex<String>>   — merged stdout+stderr buffer
  ├─ cursor: usize                           — bytes returned so far
  └─ _readers: [JoinHandle; 2]               — aborted on cancel
```

`SessionStore` is created once per `CodeExplorerServer` instance, stored as `Arc<SessionStore>`, and cloned into every `ToolContext` (same pattern as `OutputBuffer`).

### Session ID Format

`@ses_<8 lowercase hex digits>` — consistent with `@cmd_<hex>` and `@file_<hex>` conventions.

### Background Reader Tasks

Two tokio tasks are spawned per session (one for stdout, one for stderr). Both append to the same `Arc<std::sync::Mutex<String>>` output buffer, giving a merged chronological view.

```rust
// Pseudocode for one reader task
tokio::spawn(async move {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,              // EOF — process exited
            Ok(n) => output.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(_) => break,
        }
    }
});
```

### Settle Detection

After writing input to stdin, `session_send` polls the output buffer in a tight loop:

```
write(input + "\n") to stdin
last_seen_len = session.cursor
last_new_data_at = now()

loop every 10ms:
  current_len = output.lock().len()
  if current_len > last_seen_len:
    last_seen_len = current_len
    last_new_data_at = now()
  if now() - last_new_data_at >= settle_ms:
    break  ← settled
  if now() - start >= timeout_secs:
    break  ← timeout

delta = output[session.cursor .. last_seen_len]
session.cursor = last_seen_len
return delta
```

The `tokio::sync::Mutex` on `SessionStore` is held across this entire loop, ensuring sequential access per session (no concurrent sends).

### Concurrent Access

All session I/O goes through the `SessionStore`'s async mutex. This serializes operations on the same session. Different sessions can be used concurrently since they hold different mutex guards.

## Data Flow

```
run_command(interactive: true)
  ├─ tokio::process::Command::new("sh -c <cmd>")
  │   .stdin(Stdio::piped())
  │   .stdout(Stdio::piped())
  │   .stderr(Stdio::piped())
  ├─ take_stdin(), take_stdout(), take_stderr()
  ├─ spawn reader_task_stdout → Arc<Mutex<String>>
  ├─ spawn reader_task_stderr → same Arc<Mutex<String>>
  ├─ settle wait (initial output)
  ├─ store Session in SessionStore → "@ses_<hex>"
  └─ return { session_id, output, is_alive }

session_send(session_id, input)
  ├─ lock SessionStore (async mutex)
  ├─ write input + "\n" to ChildStdin
  ├─ settle poll loop (10ms intervals until 150ms quiet)
  ├─ child.try_wait() → check exit status
  ├─ update cursor
  └─ return { delta output, is_alive, exited_with? }

session_cancel(session_id)
  ├─ lock SessionStore
  ├─ child.kill().await (best-effort)
  ├─ abort reader JoinHandles
  ├─ remove from sessions map
  └─ return "ok"
```

## Files Changed

| File | Type | Change |
|---|---|---|
| `src/tools/session.rs` | **New** | `SessionStore`, `Session`, `SessionSend`, `SessionCancel` |
| `src/tools/workflow.rs` | Modified | Add `interactive: bool` param + branch to `spawn_interactive()` |
| `src/tools/mod.rs` | Modified | Add `session_store: Arc<SessionStore>` to `ToolContext` |
| `src/server.rs` | Modified | Instantiate `SessionStore`, inject into `ToolContext`, register 2 new tools |

## Error Handling

| Error condition | Response type | Message |
|---|---|---|
| Unknown `session_id` | `RecoverableError` | "Session not found. It may have been cancelled or expired." |
| Process exited, `session_send` called | `RecoverableError` | "Session process has exited. Call session_cancel to free resources." |
| Max sessions (5) reached | `RecoverableError` | "Max concurrent sessions (5) reached. Cancel an existing session first." |
| `session_cancel` on dead process | Best-effort + `"ok"` | Clean up resources, return ok |
| Stdin write fails | `RecoverableError` | "Failed to write to session stdin — process may have exited." |

## Testing Plan

1. **Unit:** `SessionStore` — create, store, retrieve, remove sessions; ID generation; max capacity
2. **Integration:** `cat` as interactive session — send `"hello\n"`, verify echo `"hello\n"` returned
3. **Integration:** `python3 -i` REPL — send expression, verify result; send `exit()`, verify `is_alive: false`
4. **Integration:** Process exits during settle — verify `is_alive: false`, `exited_with` set
5. **Integration:** `session_cancel` mid-session — verify process killed, fd released, follow-up `session_send` returns `RecoverableError`
6. **Integration:** Max session cap — create 5 sessions, verify 6th returns `RecoverableError`

## Known Limitations

1. **`isatty()` programs** — programs that check for a terminal (bare `python3`, many shells) will not enter interactive mode. Use `-i` flag (`python3 -i`) or equivalent.
2. **No PTY** — full-screen TUI apps (vim, less, htop) are not supported.
3. **ANSI escape codes** — color-aware programs may emit escape sequences; output is returned raw.
4. **Stderr ordering** — stdout and stderr are appended to the same buffer by two concurrent readers; interleaving is non-deterministic at the byte level.
5. **Session leak** — sessions that are started but never cancelled consume file descriptors until server restart. The 5-session cap provides a safety bound.
