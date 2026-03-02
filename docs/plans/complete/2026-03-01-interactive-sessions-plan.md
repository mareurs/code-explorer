# Interactive Sessions Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `run_command(interactive: true)` + `session_send` + `session_cancel` tools so the agent can interact with long-running processes (REPLs, debuggers, confirmation prompts) instead of waiting for them to exit.

**Architecture:** A `SessionStore` (analogous to `OutputBuffer`) holds running `Child` processes with piped I/O. Two background tokio tasks drain stdout/stderr into a shared `Arc<Mutex<String>>` buffer. `session_send` writes to stdin and polls the buffer until a configurable quiet window elapses (settle detection), then returns the output delta. Sessions persist until `session_cancel` or server restart.

**Tech Stack:** tokio (async process, sync::Mutex, time::sleep), std::sync::Mutex (for output buffer accessed from sync reader tasks), async_trait, serde_json

**Design doc:** `docs/plans/2026-03-01-interactive-sessions-design.md`

---

## Known Limitation (v1)

The `SessionStore` uses a single `tokio::sync::Mutex`. `session_send` holds it for the full settle window (up to `timeout_secs`). This serializes all concurrent session operations. With max 5 sessions and sequential interactive use, this is acceptable. A follow-up can use per-session mutexes.

---

## Task 1: Create `src/tools/session.rs` — Core Types

**Files:**
- Create: `src/tools/session.rs`

### Step 1: Write the failing test

Add to `src/tools/session.rs`:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::process::{Child, ChildStdin};
use tokio::task::JoinHandle;

pub struct SessionIo {
    pub stdin: ChildStdin,
    pub child: Child,
}

pub struct Session {
    pub id: String,
    pub io: SessionIo,
    pub output: Arc<StdMutex<String>>,
    pub cursor: usize,
    pub _readers: (JoinHandle<()>, JoinHandle<()>),
}

pub struct SessionStoreInner {
    pub sessions: HashMap<String, Session>,
    pub counter: u64,
    pub max_sessions: usize,
}

pub struct SessionStore {
    pub sessions: tokio::sync::Mutex<SessionStoreInner>,
}

impl SessionStore {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: tokio::sync::Mutex::new(SessionStoreInner {
                sessions: HashMap::new(),
                counter: 0,
                max_sessions,
            }),
        }
    }
}

/// Wait until `settle_ms` consecutive milliseconds pass with no new output in `buf`
/// starting from `start_cursor`. Returns `(new_end, timed_out)`.
/// Polls every 10ms.
pub async fn wait_for_settle(
    buf: &Arc<StdMutex<String>>,
    start_cursor: usize,
    settle_ms: u64,
    timeout_secs: u64,
) -> (usize, bool) {
    let settle_dur = Duration::from_millis(settle_ms);
    let max_wait = Duration::from_secs(timeout_secs);
    let start = Instant::now();
    let mut last_seen_len = start_cursor;
    let mut last_new_at = Instant::now();

    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;

        let current_len = buf.lock().unwrap().len();
        if current_len > last_seen_len {
            last_seen_len = current_len;
            last_new_at = Instant::now();
        }

        if last_new_at.elapsed() >= settle_dur {
            return (last_seen_len, false);
        }

        if start.elapsed() >= max_wait {
            return (last_seen_len, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_store_starts_empty() {
        let store = SessionStore::new(5);
        // Can't block in sync test, but can verify the struct builds
        assert!(std::sync::Arc::new(store).sessions.try_lock().is_ok());
    }

    #[tokio::test]
    async fn session_store_respects_capacity() {
        let store = SessionStore::new(2);
        let guard = store.sessions.lock().await;
        assert_eq!(guard.max_sessions, 2);
        assert!(guard.sessions.is_empty());
    }

    #[tokio::test]
    async fn wait_for_settle_returns_after_quiet_window() {
        let buf = Arc::new(StdMutex::new(String::new()));
        let buf_clone = Arc::clone(&buf);

        // Writer appends after 50ms then stops
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            buf_clone.lock().unwrap().push_str("hello");
            tokio::time::sleep(Duration::from_millis(20)).await;
            buf_clone.lock().unwrap().push_str(" world");
            // then stops — settle should trigger after 150ms of silence
        });

        let (end, timed_out) = wait_for_settle(&buf, 0, 150, 5).await;
        assert!(!timed_out);
        assert_eq!(end, "hello world".len());
    }

    #[tokio::test]
    async fn wait_for_settle_times_out_when_output_never_stops() {
        let buf = Arc::new(StdMutex::new(String::new()));
        let buf_clone = Arc::clone(&buf);

        // Writer keeps appending — should trigger timeout
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                buf_clone.lock().unwrap().push('x');
            }
        });

        let (_, timed_out) = wait_for_settle(&buf, 0, 150, 1).await;
        assert!(timed_out);
    }
}
```

### Step 2: Run test to verify it fails

```
cargo test session_store --lib
```

Expected: FAIL — `session.rs` doesn't exist yet as a module

### Step 3: Declare the module in `src/tools/mod.rs`

In `src/tools/mod.rs`, add after the last existing `pub mod` line (currently line 21: `pub mod workflow`):

```rust
pub mod session;
```

### Step 4: Run tests to verify they pass

```
cargo test -p code-explorer session --lib
```

Expected: all 4 session tests PASS

### Step 5: Run full test suite to check no regressions

```
cargo test --lib
```

Expected: all existing tests still pass + 4 new ones

### Step 6: Commit

```
git add src/tools/session.rs src/tools/mod.rs
git commit -m "feat(session): add SessionStore types and wait_for_settle"
```

---

## Task 2: Add `session_store` to `ToolContext` and `CodeExplorerServer`

**Files:**
- Modify: `src/tools/mod.rs` — add field to `ToolContext`
- Modify: `src/server.rs` — add field to `CodeExplorerServer`, instantiate, inject

### Step 1: Write the failing test

In `src/server.rs`, in the existing test module, add:

```rust
#[tokio::test]
async fn server_has_session_store() {
    // Verify ToolContext construction includes session_store.
    // This test will fail to compile until the field is added.
    let agent = crate::agent::Agent::for_test().await;
    let server = CodeExplorerServer::from_parts(agent, crate::lsp::MockLspProvider::new_arc()).await;
    // If this compiles and runs, session_store is wired up
    let ctx = server.make_test_ctx();
    let _store = ctx.session_store;
}
```

Note: Look at the existing server tests to understand how `Agent::for_test()` and `MockLspProvider` are constructed — follow the same pattern. If `make_test_ctx()` doesn't exist, just verify the field exists via `let _ = server.session_store.clone()`.

### Step 2: Run to see compile error

```
cargo test server_has_session_store
```

Expected: compile error — `session_store` field doesn't exist

### Step 3: Add `session_store` to `ToolContext` in `src/tools/mod.rs`

In `src/tools/mod.rs`, modify the `ToolContext` struct (around line 37) to add the new field:

```rust
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,
    pub output_buffer: Arc<output_buffer::OutputBuffer>,
    pub session_store: Arc<session::SessionStore>,   // ← add this
}
```

### Step 4: Add `session_store` to `CodeExplorerServer` in `src/server.rs`

**a)** In `src/server.rs`, add to the `CodeExplorerServer` struct (around line 40):

```rust
#[derive(Clone)]
pub struct CodeExplorerServer {
    agent: Agent,
    lsp: Arc<dyn LspProvider>,
    output_buffer: Arc<crate::tools::output_buffer::OutputBuffer>,
    session_store: Arc<crate::tools::session::SessionStore>,   // ← add this
    tools: Vec<Arc<dyn Tool>>,
    instructions: String,
}
```

**b)** In `from_parts` (around line 108), after the `output_buffer` line, add:

```rust
let output_buffer = Arc::new(crate::tools::output_buffer::OutputBuffer::new(20));
let session_store = Arc::new(crate::tools::session::SessionStore::new(5));  // ← add
Self {
    agent,
    lsp,
    output_buffer,
    session_store,   // ← add
    tools,
    instructions,
}
```

**c)** In `call_tool` (around line 174), update the `ToolContext` construction:

```rust
let ctx = ToolContext {
    agent: self.agent.clone(),
    lsp: self.lsp.clone(),
    output_buffer: self.output_buffer.clone(),
    session_store: self.session_store.clone(),   // ← add
};
```

### Step 5: Fix any remaining compile errors, then run tests

```
cargo build 2>&1 | head -30
```

Fix any errors. Then:

```
cargo test --lib
```

Expected: all tests pass (including the new server test)

### Step 6: Commit

```
git add src/tools/mod.rs src/server.rs
git commit -m "feat(session): add session_store to ToolContext and CodeExplorerServer"
```

---

## Task 3: Implement `SessionSend` Tool

**Files:**
- Modify: `src/tools/session.rs` — add `SessionSend` struct + `Tool` impl

### Step 1: Write the failing integration test

Add to the `#[cfg(test)]` block in `src/tools/session.rs`:

```rust
#[tokio::test]
async fn session_send_cat_echoes_input() {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    // Manually set up a session using `cat` (always available)
    let mut child = tokio::process::Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cat must be available");

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let output = Arc::new(StdMutex::new(String::new()));

    let out_clone = Arc::clone(&output);
    let _reader_out = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => out_clone.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
    });

    let err_clone = Arc::clone(&output);
    let _reader_err = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => err_clone.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
    });

    let session = Session {
        id: "@ses_test0001".to_string(),
        io: SessionIo { stdin, child },
        output: Arc::clone(&output),
        cursor: 0,
        _readers: (_reader_out, _reader_err),
    };

    let store = Arc::new(SessionStore::new(5));
    {
        let mut guard = store.sessions.lock().await;
        guard.sessions.insert(session.id.clone(), session);
    }

    // Simulate session_send: write "hello" and read echo
    {
        let mut guard = store.sessions.lock().await;
        let s = guard.sessions.get_mut("@ses_test0001").unwrap();
        let cursor = s.cursor;
        tokio::io::AsyncWriteExt::write_all(&mut s.io.stdin, b"hello\n").await.unwrap();
        let (new_cursor, timed_out) = wait_for_settle(&s.output, cursor, 150, 5).await;
        assert!(!timed_out);
        let delta = s.output.lock().unwrap()[cursor..new_cursor].to_string();
        s.cursor = new_cursor;
        assert_eq!(delta, "hello\n");
    }

    // Clean up
    let mut guard = store.sessions.lock().await;
    if let Some(mut s) = guard.sessions.remove("@ses_test0001") {
        let _ = s.io.child.kill().await;
    }
}
```

### Step 2: Run to verify it fails

```
cargo test session_send_cat_echoes_input --lib
```

Expected: compile error — `SessionIo`, `Session` don't have the right shape yet (check what's missing)

Fix the `Session` struct in the previous task if needed. Then:

Expected: test PASSES (this is the core mechanism test, before the full tool is wired up)

### Step 3: Implement the `SessionSend` tool

Add to `src/tools/session.rs`:

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use super::{ToolContext, Tool, RecoverableError};

pub struct SessionSend;

#[async_trait]
impl Tool for SessionSend {
    fn name(&self) -> &str {
        "session_send"
    }

    fn description(&self) -> &str {
        "Send a line of input to a running interactive session and return the response. \
        The session must have been started with run_command(interactive: true). \
        Use session_cancel when done to free resources."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["session_id", "input"],
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "Session handle from run_command (e.g. \"@ses_a1b2c3d4\")"
                },
                "input": {
                    "type": "string",
                    "description": "Line to send to the process stdin. A newline is appended automatically."
                },
                "settle_ms": {
                    "type": "integer",
                    "description": "Milliseconds of silence before returning output (default: 150)",
                    "default": 150
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Maximum seconds to wait for output (default: 10)",
                    "default": 10
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        use tokio::io::AsyncWriteExt;

        let session_id = super::require_str_param(&input, "session_id")?;
        let user_input = super::require_str_param(&input, "input")?;
        let settle_ms = input["settle_ms"].as_u64().unwrap_or(150);
        let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(10);

        let mut guard = ctx.session_store.sessions.lock().await;

        let session = guard.sessions.get_mut(session_id).ok_or_else(|| {
            RecoverableError::with_hint(
                "Session not found",
                "It may have been cancelled. Use run_command(interactive: true) to start a new session.",
            )
        })?;

        // Check if process already exited before writing
        if let Ok(Some(status)) = session.io.child.try_wait() {
            return Ok(json!({
                "output": "",
                "is_alive": false,
                "exited_with": status.code(),
                "hint": "Session process has exited. Call session_cancel to free resources."
            }));
        }

        let start_cursor = session.cursor;
        let output_ref = Arc::clone(&session.output);

        // Write input to stdin
        session.io.stdin
            .write_all(format!("{}\n", user_input).as_bytes())
            .await
            .map_err(|e| {
                RecoverableError::with_hint(
                    &format!("Failed to write to session stdin: {e}"),
                    "The process may have exited unexpectedly.",
                )
            })?;

        // Wait for settle (holds store lock — see known limitation in design doc)
        let (new_cursor, timed_out) = wait_for_settle(&output_ref, start_cursor, settle_ms, timeout_secs).await;

        // Check exit status after settle
        let exited = session.io.child.try_wait().ok().flatten();
        let is_alive = exited.is_none();

        // Extract delta
        let delta = {
            let buf = output_ref.lock().unwrap();
            buf[start_cursor..new_cursor].to_string()
        };

        session.cursor = new_cursor;

        let mut result = json!({
            "output": delta,
            "is_alive": is_alive,
            "exited_with": exited.and_then(|s| s.code()),
        });

        if timed_out {
            result["timed_out"] = json!(true);
            result["hint"] = json!(
                "Output may be incomplete. Increase timeout_secs or reduce settle_ms."
            );
        } else if !is_alive {
            result["hint"] = json!(
                "Session has ended. Call session_cancel to free resources."
            );
        }

        Ok(result)
    }
}
```

### Step 4: Run tests

```
cargo test session --lib
```

Expected: all session tests pass

### Step 5: Verify clippy

```
cargo clippy -- -D warnings 2>&1 | grep "^error"
```

Expected: no errors

### Step 6: Commit

```
git add src/tools/session.rs
git commit -m "feat(session): implement SessionSend tool"
```

---

## Task 4: Implement `SessionCancel` Tool

**Files:**
- Modify: `src/tools/session.rs` — add `SessionCancel` struct + `Tool` impl

### Step 1: Write the failing test

Add to `#[cfg(test)]` in `src/tools/session.rs`:

```rust
#[tokio::test]
async fn session_cancel_kills_process_and_removes_session() {
    use std::process::Stdio;

    let mut child = tokio::process::Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cat must be available");

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let output = Arc::new(StdMutex::new(String::new()));

    let r1 = tokio::spawn(async move { drop(stdout) });
    let r2 = tokio::spawn(async move { drop(stderr) });

    let session = Session {
        id: "@ses_cancel01".to_string(),
        io: SessionIo { stdin, child },
        output,
        cursor: 0,
        _readers: (r1, r2),
    };

    let store = Arc::new(SessionStore::new(5));
    {
        let mut guard = store.sessions.lock().await;
        guard.sessions.insert("@ses_cancel01".to_string(), session);
    }

    // Cancel the session
    {
        let mut guard = store.sessions.lock().await;
        let mut s = guard.sessions.remove("@ses_cancel01").unwrap();
        drop(guard);
        s._readers.0.abort();
        s._readers.1.abort();
        let _ = s.io.child.kill().await;
    }

    // Verify session is gone from store
    let guard = store.sessions.lock().await;
    assert!(!guard.sessions.contains_key("@ses_cancel01"));
}
```

### Step 2: Run to verify it fails (will compile-error or fail assertion)

```
cargo test session_cancel_kills --lib
```

### Step 3: Implement `SessionCancel`

Add to `src/tools/session.rs` (after `SessionSend`):

```rust
pub struct SessionCancel;

#[async_trait]
impl Tool for SessionCancel {
    fn name(&self) -> &str {
        "session_cancel"
    }

    fn description(&self) -> &str {
        "Terminate an interactive session, kill the process, and free all resources. \
        Always call this when done with a session, even if the process has already exited."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["session_id"],
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "Session handle from run_command (e.g. \"@ses_a1b2c3d4\")"
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let session_id = super::require_str_param(&input, "session_id")?;

        let mut session = {
            let mut guard = ctx.session_store.sessions.lock().await;
            guard.sessions.remove(session_id).ok_or_else(|| {
                RecoverableError::with_hint(
                    "Session not found",
                    "It may have already been cancelled.",
                )
            })?
        }; // Store lock released here

        // Clean up resources outside the store lock
        session._readers.0.abort();
        session._readers.1.abort();
        let _ = session.io.child.kill().await; // best-effort; may already be dead

        Ok(json!("ok"))
    }
}
```

### Step 4: Run tests

```
cargo test session --lib
```

Expected: all session tests pass

### Step 5: Commit

```
git add src/tools/session.rs
git commit -m "feat(session): implement SessionCancel tool"
```

---

## Task 5: Add `interactive` Mode to `run_command`

**Files:**
- Modify: `src/tools/workflow.rs` — add `interactive` param, `spawn_interactive` helper

### Step 1: Write the failing integration test

Add to `#[cfg(test)]` in `src/tools/workflow.rs` (look at existing test structure there to match the pattern — likely uses `TestContext` or similar):

```rust
#[tokio::test]
async fn run_command_interactive_returns_session_id() {
    // Spawn `cat` interactively — it waits for stdin immediately
    let input = serde_json::json!({
        "command": "cat",
        "interactive": true,
        "settle_ms": 150
    });

    // Build a test ToolContext (follow the pattern of existing workflow tests)
    let ctx = make_test_ctx().await;  // use whatever helper exists in tests

    let result = RunCommand.call(input, &ctx).await.unwrap();

    // cat produces no startup output but should be alive
    assert!(result["session_id"].as_str().unwrap().starts_with("@ses_"));
    assert_eq!(result["is_alive"], true);

    // Clean up
    let session_id = result["session_id"].as_str().unwrap().to_string();
    let cancel = crate::tools::session::SessionCancel;
    cancel.call(json!({ "session_id": session_id }), &ctx).await.unwrap();
}
```

Note: look at the existing tests in `workflow.rs` to understand how `make_test_ctx()` (or equivalent) works. If none exists, build one using `Agent::default_for_test()` and the project root.

### Step 2: Run to see it fail

```
cargo test run_command_interactive --lib
```

Expected: compile error — `interactive` param doesn't exist yet

### Step 3: Update `run_command`'s `input_schema`

In `src/tools/workflow.rs`, find the `impl Tool for RunCommand` block and its `input_schema` method. Add two new properties:

```rust
"interactive": {
    "type": "boolean",
    "description": "If true, keep the process running and return a session handle for interaction via session_send. Default: false.",
    "default": false
},
"settle_ms": {
    "type": "integer",
    "description": "When interactive: true, milliseconds of silence before returning initial output (default: 150)",
    "default": 150
},
```

### Step 4: Add the `spawn_interactive` helper function

Add this free function to `src/tools/workflow.rs` (before or after `run_command_inner`):

```rust
async fn spawn_interactive(
    command: &str,
    settle_ms: u64,
    cwd_param: Option<&str>,
    root: &std::path::Path,
    security: &crate::util::path_security::PathSecurityConfig,
    ctx: &ToolContext,
) -> anyhow::Result<Value> {
    use std::process::Stdio;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::io::AsyncReadExt;
    use crate::tools::session::{Session, SessionIo, wait_for_settle};

    // 1. Session cap check
    {
        let guard = ctx.session_store.sessions.lock().await;
        if guard.sessions.len() >= guard.max_sessions {
            return Err(crate::tools::RecoverableError::with_hint(
                "Max concurrent sessions reached",
                &format!(
                    "Cancel an existing session with session_cancel before starting a new one. Max: {}",
                    guard.max_sessions
                ),
            ).into());
        }
    }

    // 2. Shell mode check (mirrors run_command_inner)
    match security.shell_command_mode.as_deref() {
        Some("disabled") => {
            return Err(crate::tools::RecoverableError::with_hint(
                "Shell commands are disabled by security config",
                "Set shell_command_mode to \"warn\" or \"unrestricted\" in project.toml",
            ).into());
        }
        _ => {}
    }

    // 3. Resolve working directory (mirrors run_command_inner logic)
    let work_dir = if let Some(cwd) = cwd_param {
        let candidate = root.join(cwd);
        let canonical = candidate
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("cwd '{}' is invalid: {}", cwd, e))?;
        if !canonical.starts_with(root) {
            anyhow::bail!("cwd '{}' escapes the project root", cwd);
        }
        canonical
    } else {
        root.to_path_buf()
    };

    // 4. Spawn with piped I/O
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&work_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn '{}': {}", command, e))?;

    let stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // 5. Shared output buffer + reader tasks
    let output = Arc::new(StdMutex::new(String::new()));

    let out_clone = Arc::clone(&output);
    let reader_stdout = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => out_clone.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
    });

    let err_clone = Arc::clone(&output);
    let reader_stderr = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => err_clone.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
    });

    // 6. Initial settle wait
    let (cursor, _timed_out) = wait_for_settle(&output, 0, settle_ms, 5).await;

    // 7. Check if process exited during startup
    let exited = child.try_wait()?;
    if let Some(status) = exited {
        reader_stdout.abort();
        reader_stderr.abort();
        let initial_output = output.lock().unwrap()[..cursor].to_string();
        return Ok(serde_json::json!({
            "output": initial_output,
            "is_alive": false,
            "exited_with": status.code(),
        }));
    }

    // 8. Generate session ID and store
    let session_id = {
        let mut guard = ctx.session_store.sessions.lock().await;
        guard.counter = guard.counter.wrapping_add(1);
        let id = format!(
            "@ses_{:08x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
                .wrapping_add(guard.counter as u32)
        );
        guard.sessions.insert(
            id.clone(),
            Session {
                id: id.clone(),
                io: SessionIo { stdin, child },
                output: Arc::clone(&output),
                cursor,
                _readers: (reader_stdout, reader_stderr),
            },
        );
        id
    };

    let initial_output = output.lock().unwrap()[..cursor].to_string();

    Ok(serde_json::json!({
        "session_id": session_id,
        "output": initial_output,
        "is_alive": true,
        "exited_with": null,
        "hint": "Use session_send to interact, session_cancel to terminate"
    }))
}
```

### Step 5: Add the branch in `RunCommand::call`

In `src/tools/workflow.rs`, in the `RunCommand::call` method (around line 439), add an early-return branch before the `resolve_refs` call:

```rust
async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
    use super::output_buffer::OutputBuffer;

    let command = super::require_str_param(&input, "command")?;
    let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(30);
    let acknowledge_risk = input["acknowledge_risk"].as_bool().unwrap_or(false);
    let cwd_param = input["cwd"].as_str();
    let interactive = input["interactive"].as_bool().unwrap_or(false);   // ← add
    let settle_ms = input["settle_ms"].as_u64().unwrap_or(150);           // ← add
    let root = ctx.agent.require_project_root().await?;
    let security = ctx.agent.security_config().await;

    // ← add this block
    if interactive {
        return spawn_interactive(command, settle_ms, cwd_param, &root, &security, ctx).await;
    }

    // (rest of the existing call method unchanged)
    let (resolved_command, temp_files, buffer_only) =
        ctx.output_buffer.resolve_refs(command)?;
    // ...
```

### Step 6: Run tests

```
cargo test --lib 2>&1 | tail -20
```

Expected: all tests pass including the new `run_command_interactive_returns_session_id`

### Step 7: Verify no clippy warnings

```
cargo clippy -- -D warnings
```

### Step 8: Commit

```
git add src/tools/workflow.rs
git commit -m "feat(session): add interactive mode to run_command via spawn_interactive"
```

---

## Task 6: Register `SessionSend` and `SessionCancel` in `server.rs`

**Files:**
- Modify: `src/server.rs` — add imports + tool registrations

### Step 1: The test already exists — verify it would fail

```
cargo test server_registers_all_tools --lib
```

Look at what this test checks. It likely counts tools or checks by name. Note the current count (32 tools). The test will need updating after we add 2 more.

### Step 2: Add imports for the new tools

In `src/server.rs`, find the existing `use crate::tools::...` import block (around line 33). Add session tool imports alongside the workflow imports:

```rust
use crate::tools::session::{SessionCancel, SessionSend};
```

### Step 3: Register the tools in `from_parts`

In the `tools: Vec<Arc<dyn Tool>> = vec![...]` block (around lines 62–112), add the two new tools after the workflow tools section:

```rust
// Workflow tools
Arc::new(RunCommand),
Arc::new(Onboarding),
// Session tools  ← add
Arc::new(SessionSend),
Arc::new(SessionCancel),
```

### Step 4: Update the `server_registers_all_tools` test

Find `server_registers_all_tools` test and update the expected count from 32 to 34 (or whatever the naming-based assertion checks — update accordingly).

### Step 5: Run all tests

```
cargo test
```

Expected: all tests pass including updated `server_registers_all_tools`

### Step 6: Verify fmt and clippy

```
cargo fmt && cargo clippy -- -D warnings
```

### Step 7: Commit

```
git add src/server.rs
git commit -m "feat(session): register session_send and session_cancel tools"
```

---

## Task 7: End-to-End Integration Test

**Files:**
- Modify: `src/tools/session.rs` — add a full round-trip test

### Step 1: Add a full round-trip test using `cat`

This test goes through the real tool interface (not the internal helpers):

```rust
#[tokio::test]
async fn end_to_end_cat_session_via_tools() {
    use serde_json::json;

    // This test needs a ToolContext. Adapt based on existing test infrastructure.
    // Check src/tools/workflow.rs for how tests build a ToolContext (look for
    // `make_test_context`, `test_ctx`, or similar helpers).
    //
    // If no helper exists, you may need to add one to src/tools/mod.rs.
    // Minimal ToolContext needs: Agent::for_test(), MockLspProvider, OutputBuffer, SessionStore.
    let ctx = /* adapt from existing test infrastructure */;

    // Step 1: Start interactive cat session
    let start_result = RunCommand.call(json!({
        "command": "cat",
        "interactive": true,
        "settle_ms": 150
    }), &ctx).await.unwrap();

    let session_id = start_result["session_id"].as_str().unwrap().to_string();
    assert!(session_id.starts_with("@ses_"));
    assert_eq!(start_result["is_alive"], true);

    // Step 2: Send input, verify echo
    let send_result = SessionSend.call(json!({
        "session_id": session_id,
        "input": "hello world",
        "settle_ms": 150
    }), &ctx).await.unwrap();

    assert_eq!(send_result["output"].as_str().unwrap(), "hello world\n");
    assert_eq!(send_result["is_alive"], true);

    // Step 3: Cancel
    let cancel_result = SessionCancel.call(json!({
        "session_id": &session_id
    }), &ctx).await.unwrap();

    assert_eq!(cancel_result.as_str().unwrap(), "ok");

    // Step 4: Verify session is gone (session_send should return RecoverableError)
    let err = SessionSend.call(json!({
        "session_id": session_id,
        "input": "still alive?"
    }), &ctx).await;
    // This should return a RecoverableError (not panic)
    assert!(err.is_err());
    let e = err.unwrap_err();
    assert!(e.downcast_ref::<RecoverableError>().is_some());
}
```

### Step 2: Run it and fix any issues

```
cargo test end_to_end_cat --lib
```

Fix any ToolContext setup issues. This test exercises the full pipeline.

### Step 3: Run the full test suite

```
cargo test
```

Expected: all tests pass

### Step 4: Final checks

```
cargo fmt && cargo clippy -- -D warnings && cargo test
```

All three must pass cleanly before committing.

### Step 5: Final commit

```
git add src/tools/session.rs
git commit -m "test(session): add end-to-end integration test for interactive cat session"
```

---

## Final Verification

```
cargo build --release
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

All must pass. Then review `docs/TODO-tool-misbehaviors.md` — add an entry if anything unexpected was observed during implementation.

---

## Summary of New Tools

| Tool | Input | Output |
|---|---|---|
| `run_command` (extended) | `interactive: true, settle_ms: 150` | `{ session_id, output, is_alive, exited_with, hint }` |
| `session_send` | `session_id, input, settle_ms?, timeout_secs?` | `{ output, is_alive, exited_with?, timed_out?, hint? }` |
| `session_cancel` | `session_id` | `"ok"` |

Total tool count: 34 (was 32)
