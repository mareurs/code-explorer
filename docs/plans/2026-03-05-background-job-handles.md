# Background Job Handles Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Give `run_in_background: true` a proper `@bg_*` handle — the tool waits 5s then returns tail-50 of initial output plus a live-file ref the LLM can query with `tail`, `grep`, or `cat`.

**Architecture:** Add a `background_jobs` map to `BufferInner` (same LRU cap as pending-acks). `resolve_refs` reads `@bg_*` refs fresh from disk on every query. `run_command_inner` spawns the process, waits 5s, stores a `@bg_*` ref, returns `{output_id, stdout: tail_50, hint}`.

**Tech Stack:** Rust, tokio, tempfile crate, existing `OutputBuffer` / `RecoverableError` / `NamedTempFile` patterns already in the file.

---

### Task 1: `@bg_*` storage in `OutputBuffer`

**Files:**
- Modify: `src/tools/output_buffer.rs:55-68` (BufferInner fields)
- Modify: `src/tools/output_buffer.rs:72-86` (OutputBuffer::new)
- Modify: `src/tools/output_buffer.rs` (add `store_background` + `get_background` methods)
- Test: `src/tools/output_buffer.rs` (tests module, end of file)

**Step 1: Write the failing tests**

Add to the `tests` module at the bottom of `src/tools/output_buffer.rs`:

```rust
#[test]
fn store_background_returns_bg_prefix() {
    let buf = OutputBuffer::new(10);
    let path = std::path::PathBuf::from("/tmp/test-codescout.log");
    let id = buf.store_background(path.clone());
    assert!(id.starts_with("@bg_"), "expected @bg_ prefix, got {id}");
    assert_eq!(buf.get_background(&id), Some(path));
}

#[test]
fn get_background_missing_returns_none() {
    let buf = OutputBuffer::new(10);
    assert_eq!(buf.get_background("@bg_00000000"), None);
}
```

**Step 2: Run to verify they fail**

```bash
cargo test store_background_returns_bg_prefix get_background_missing_returns_none 2>&1
```
Expected: compile error — `store_background` and `get_background` not defined.

**Step 3: Add fields to `BufferInner`**

In `BufferInner` struct (L55-68), add after `max_pending`:

```rust
// --- background job store ---
background_jobs: HashMap<String, std::path::PathBuf>,
background_order: Vec<String>,
```

In `OutputBuffer::new` (L72-86), add to the `BufferInner { ... }` initializer:

```rust
background_jobs: HashMap::new(),
background_order: Vec::new(),
```

**Step 4: Add `store_background` and `get_background` methods**

Add after the `get_pending_edit` method (after L355):

```rust
/// Store a background job log path and return a `@bg_<8hex>` handle.
pub fn store_background(&self, log_path: std::path::PathBuf) -> String {
    let mut inner = self.inner.lock().unwrap();
    inner.counter = inner.counter.wrapping_add(1);
    let id = format!("@bg_{:08x}", inner.counter as u32);

    // Evict oldest if at capacity
    if inner.background_jobs.len() >= inner.max_pending {
        if let Some(oldest) = inner.background_order.first().cloned() {
            inner.background_order.remove(0);
            inner.background_jobs.remove(&oldest);
        }
    }

    inner.background_jobs.insert(id.clone(), log_path);
    inner.background_order.push(id.clone());
    id
}

/// Look up the log path for a background job handle.
pub fn get_background(&self, id: &str) -> Option<std::path::PathBuf> {
    let inner = self.inner.lock().unwrap();
    inner.background_jobs.get(id).cloned()
}
```

**Step 5: Run tests to verify they pass**

```bash
cargo test store_background get_background_missing 2>&1
```
Expected: 2 tests pass.

**Step 6: Commit**

```bash
git add src/tools/output_buffer.rs
git commit -m "feat: add @bg_* background job storage to OutputBuffer"
```

---

### Task 2: `resolve_refs` handles `@bg_*` — always-fresh disk reads

**Files:**
- Modify: `src/tools/output_buffer.rs:368-486` (`resolve_refs` method)
- Test: `src/tools/output_buffer.rs` (tests module)

**Step 1: Write the failing tests**

```rust
#[test]
fn resolve_refs_bg_reads_fresh_from_disk() {
    use std::io::Write;
    let buf = OutputBuffer::new(10);

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    write!(tmp, "first content").unwrap();
    tmp.flush().unwrap();
    let log_path = tmp.path().to_path_buf();

    let id = buf.store_background(log_path.clone());

    // First resolve — reads "first content"
    let (resolved1, temps1, _, _) = buf.resolve_refs(&id).unwrap();
    let got1 = std::fs::read_to_string(&resolved1).unwrap();
    OutputBuffer::cleanup_temp_files(&temps1);
    assert_eq!(got1.trim(), "first content");

    // Overwrite log — simulates process writing more output
    std::fs::write(&log_path, "second content").unwrap();

    // Second resolve — must read fresh "second content", not the snapshot
    let (resolved2, temps2, _, _) = buf.resolve_refs(&id).unwrap();
    let got2 = std::fs::read_to_string(&resolved2).unwrap();
    OutputBuffer::cleanup_temp_files(&temps2);
    assert_eq!(got2.trim(), "second content");
}

#[test]
fn resolve_refs_bg_missing_file_errors() {
    let buf = OutputBuffer::new(10);
    let id = buf.store_background(std::path::PathBuf::from(
        "/tmp/nonexistent-codescout-bg-test-xyz.log",
    ));
    let err = buf.resolve_refs(&id).unwrap_err();
    assert!(
        err.to_string().contains("background job log unavailable"),
        "unexpected error: {err}"
    );
}
```

**Step 2: Run to verify they fail**

```bash
cargo test resolve_refs_bg 2>&1
```
Expected: both fail — `resolve_refs` doesn't recognise `@bg_*` yet.

**Step 3: Extend the regex in `resolve_refs`**

On L381 (the `re = Regex::new(...)` line), change:
```rust
// old
let re = Regex::new(r"@(?:cmd|file|tool)_[0-9a-f]{8}(\.err)?").expect("valid regex");
// new
let re = Regex::new(r"@(?:cmd|file|tool|bg)_[0-9a-f]{8}(\.err)?").expect("valid regex");
```

**Step 4: Add the `@bg_*` branch in the ref loop**

Inside the `for token in &unique_refs` loop, add this branch **before** the existing `get_with_refresh_flag` call:

```rust
// @bg_* refs always read fresh from disk — no snapshot caching.
if base_id.starts_with("@bg_") {
    let log_path = self.get_background(base_id).ok_or_else(|| {
        RecoverableError::with_hint(
            format!("background job ref not found: {}", token),
            "Buffer refs expire when the session resets. Re-run the original command to get a fresh handle.",
        )
    })?;
    let content = std::fs::read_to_string(&log_path).map_err(|e| {
        RecoverableError::with_hint(
            format!("background job log unavailable: {}", e),
            format!(
                "Check if the process is still running. Log path: {}",
                log_path.display()
            ),
        )
    })?;
    let mut tmp = NamedTempFile::new()?;
    tmp.write_all(content.as_bytes())?;
    tmp.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o444);
        std::fs::set_permissions(tmp.path(), perms)?;
    }
    let temp_path = tmp.into_temp_path();
    let path = temp_path.to_path_buf();
    std::mem::forget(temp_path);
    let path_str = path.to_string_lossy().to_string();
    result = result.replace(token, &path_str);
    temp_path_strings.push(path_str);
    temp_paths.push(path);
    continue;
}
```

**Step 5: Run tests to verify they pass**

```bash
cargo test resolve_refs_bg 2>&1
```
Expected: both pass.

**Step 6: Run full test suite**

```bash
cargo test 2>&1
```
Expected: all existing tests still pass.

**Step 7: Commit**

```bash
git add src/tools/output_buffer.rs
git commit -m "feat: resolve_refs handles @bg_* with always-fresh disk reads"
```

---

### Task 3: `run_command_inner` warm return

**Files:**
- Modify: `src/tools/workflow.rs` (background branch in `run_command_inner`, around L880-900)
- Test: `src/tools/workflow.rs` (tests module at bottom)

**Step 1: Write the failing test**

In the `tests` module of `workflow.rs`, add after the existing `dangerous_command_*` tests. Follow the same ctx/agent construction pattern used by the `dangerous_command_blocked` test at L1690+:

```rust
#[tokio::test]
async fn run_in_background_returns_bg_handle() {
    let (ctx, root) = make_test_ctx().await; // reuse the helper already in the tests module

    let result = run_command_inner(
        "echo hello-bg-test",
        "echo hello-bg-test",
        30,
        false, // acknowledge_risk
        None,  // cwd_param
        false, // buffer_only
        true,  // run_in_background
        &root,
        &crate::util::path_security::PathSecurityConfig::default(),
        &ctx,
    )
    .await
    .expect("should succeed");

    let output_id = result["output_id"].as_str().expect("output_id missing");
    assert!(
        output_id.starts_with("@bg_"),
        "expected @bg_ prefix, got {output_id}"
    );
    let stdout = result["stdout"].as_str().unwrap_or("");
    assert!(
        stdout.contains("hello-bg-test"),
        "expected stdout to contain echo output, got: {stdout}"
    );
    let hint = result["hint"].as_str().unwrap_or("");
    assert!(
        hint.contains(output_id),
        "hint should reference the handle"
    );
}
```

> **Note:** Check what the existing helper function is called in the tests module (likely `make_test_ctx` or similar) and use it. If it doesn't exist, look at how `dangerous_command_blocked` sets up its `ctx` and replicate that inline.

**Step 2: Run to verify it fails**

```bash
cargo test run_in_background_returns_bg_handle 2>&1
```
Expected: fail — current background branch doesn't return `@bg_*`.

**Step 3: Replace the background branch in `run_command_inner`**

The current Step 4.7 block (around L880) does fire-and-forget. Replace the entire `if run_in_background { ... }` block with:

```rust
// --- Step 4.7: Background spawn with warm return ---
if run_in_background {
    if buffer_only {
        return Err(super::RecoverableError::with_hint(
            "run_in_background cannot be used with buffer queries",
            "Remove run_in_background, or run the query as a plain command without @ref interpolation.",
        )
        .into());
    }

    let log_tmp = tempfile::Builder::new()
        .prefix("codescout-bg-")
        .suffix(".log")
        .tempfile()?;
    let log_path = log_tmp.path().to_path_buf();
    let (log_file, _) = log_tmp.keep()?;
    let log_stderr = log_file.try_clone()?;

    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(resolved_command)
        .current_dir(&work_dir)
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_stderr))
        .spawn()?;

    // Warm return: 5s window captures startup output and fast failures.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let log_content = std::fs::read_to_string(&log_path).unwrap_or_default();
    let tail_50: String = {
        let lines: Vec<&str> = log_content.lines().collect();
        let start = lines.len().saturating_sub(50);
        lines[start..].join("\n")
    };

    let ref_id = ctx.output_buffer.store_background(log_path);

    return Ok(serde_json::json!({
        "output_id": ref_id,
        "stdout": tail_50,
        "hint": format!(
            "Process running. Output captured in {} — use run_command(\"tail -50 {}\") or grep/cat as needed.",
            ref_id, ref_id
        )
    }));
}
```

**Step 4: Run the test to verify it passes**

```bash
cargo test run_in_background_returns_bg_handle 2>&1
```
Expected: pass. Note: test will take ~5s due to the warm return sleep.

**Step 5: Run full suite**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test 2>&1
```
Expected: all pass, no warnings.

**Step 6: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: run_in_background warm return — 5s wait, @bg_* handle, tail-50"
```

---

### Task 4: Build release binary

**Step 1:**

```bash
cargo build --release 2>&1
```
Expected: compiles clean.

**Step 2: Commit**

```bash
git add -A  # only if any files changed
# If nothing changed besides the binary (which is not tracked), skip this commit.
```

---

## Verification

After all tasks, confirm end-to-end with a live test via the MCP server (after `/mcp` restart):

```
run_command("sleep 10 && echo done", run_in_background: true)
```

Expected response shape:
```json
{
  "output_id": "@bg_xxxxxxxx",
  "stdout": "",
  "hint": "Process running. Output captured in @bg_xxxxxxxx — ..."
}
```

Then before 10s are up:
```
run_command("cat @bg_xxxxxxxx")   // empty or partial
```

After 10s:
```
run_command("cat @bg_xxxxxxxx")   // contains "done"
```
