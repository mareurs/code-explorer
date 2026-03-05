# Prompt & Tool Audit Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove noisy/stale guidance from agent-facing prompts, remove the `worktree_hint` advisory from write-tool responses, and add a just-in-time `↻ refreshed` indicator when `@file_*` handles auto-refresh during `run_command`.

**Architecture:** Four independent work streams: (1) pure text edits to prompt files, (2) delete `worktree_hint` helper and its 7 call sites, (3) extend `OutputBuffer::resolve_refs` to report which `@file_*` handles were refreshed, (4) surface that info in `RunCommand` output.

**Tech Stack:** Rust, `serde_json`, existing `OutputBuffer` / `Tool` infrastructure.

---

### Task 1: Documentation-only fixes

**Files:**
- Modify: `src/prompts/server_instructions.md`
- Modify: `src/prompts/onboarding_prompt.md`
- Modify: `.codescout/system-prompt.md`
- Modify: `CLAUDE.md`

No tests needed — these are plain text changes.

**Step 1: Edit `server_instructions.md` — remove `get_usage_stats` line**

In the "Project Management" section, delete this line:
```
- `get_usage_stats` — per-tool call counts, error rates, latency percentiles
```

**Step 2: Edit `server_instructions.md` — remove `worktree_hint` mention from Worktrees section**

Current (lines 155–157):
```
If you forget, write tools will silently modify the main repo instead of the worktree — they will include a `"worktree_hint"` field in their response to alert you. When you see that field, call `activate_project` and redo the write.
```

Replace with (remove the hint-specific sentence, keep the consequence):
```
If you forget, write tools will silently modify the main repo instead of the worktree — call `activate_project` and redo the write.
```

**Step 3: Edit `onboarding_prompt.md` — fix stale tool name**

Find (line 8):
```
`get_symbols_overview("src")` for architecture
```
Replace with:
```
`list_symbols("src/")` for architecture
```

**Step 4: Edit `.codescout/system-prompt.md` — fix tool count**

Find:
```
`Tool` trait (`src/tools/mod.rs:167`) — interface for all 31 tools
```
Replace with:
```
`Tool` trait (`src/tools/mod.rs`) — interface for all 32 tools
```
(Drop the stale line number too — it drifts.)

**Step 5: Edit `CLAUDE.md` — fix tool count**

Find:
```
30 tools registered
```
Replace with:
```
32 tools registered
```

**Step 6: Run tests and commit**

```bash
cargo test
```
Expected: all tests pass (no code changed).

```bash
git add src/prompts/server_instructions.md src/prompts/onboarding_prompt.md \
        .codescout/system-prompt.md CLAUDE.md
git commit -m "docs: fix stale tool counts, remove get_usage_stats from prompt, fix get_symbols_overview"
```

---

### Task 2: Remove `worktree_hint` from write tools

**Files:**
- Modify: `src/util/path_security.rs` — delete `worktree_hint()` + 2 of its tests
- Modify: `src/tools/file.rs` — 3 call sites
- Modify: `src/tools/symbol.rs` — 4 call sites

**Step 1: Delete `worktree_hint()` from `path_security.rs`**

Find and delete the `worktree_hint` function body (around line 311):
```rust
/// Returns an advisory hint string if git linked worktrees exist under
/// `project_root`, so an agent knows it may have written to the main repo
/// instead of a worktree.
///
/// Returns `None` if no worktrees exist (zero-overhead fast path).
pub fn worktree_hint(project_root: &Path) -> Option<String> {
    let worktrees = list_git_worktrees(project_root);
    if worktrees.is_empty() {
        return None;
    }
    // ... (full body)
}
```
Keep `list_git_worktrees()` — it is still used by `guard_worktree_write` in `src/tools/mod.rs`.

Also delete these two tests from the `tests` module in `path_security.rs`:
- `worktree_hint_none_when_no_worktrees`
- `worktree_hint_some_when_worktrees_exist`

**Step 2: Simplify `create_file` in `file.rs` (line ~478)**

Before:
```rust
let hint = crate::util::path_security::worktree_hint(&root);
Ok(match hint {
    None => json!("ok"),
    Some(h) => json!({ "worktree_hint": h }),
})
```
After:
```rust
Ok(json!("ok"))
```

**Step 3: Simplify `edit_file` prepend/append path in `file.rs` (line ~1050)**

Before:
```rust
let hint = crate::util::path_security::worktree_hint(&root);
return Ok(match hint {
    None => json!("ok"),
    Some(h) => json!({ "worktree_hint": h }),
});
```
After:
```rust
return Ok(json!("ok"));
```

**Step 4: Simplify `edit_file` regular path in `file.rs` (line ~1137)**

Before:
```rust
let hint = crate::util::path_security::worktree_hint(&root);
Ok(match hint {
    None => json!("ok"),
    Some(h) => json!({ "worktree_hint": h }),
})
```
After:
```rust
Ok(json!("ok"))
```

**Step 5: Simplify `replace_symbol` in `symbol.rs` (line ~1351)**

Before:
```rust
let hint = crate::util::path_security::worktree_hint(&root);
let mut resp = json!({ "status": "ok", "replaced_lines": format!("{}-{}", start + 1, end) });
if let Some(h) = hint {
    resp["worktree_hint"] = json!(h);
}
Ok(resp)
```
After:
```rust
Ok(json!({ "status": "ok", "replaced_lines": format!("{}-{}", start + 1, end) }))
```

**Step 6: Simplify `remove_symbol` in `symbol.rs` (line ~1434)**

Before:
```rust
let hint = crate::util::path_security::worktree_hint(&root);
let line_count = end - start;
let removed_range = format!("{}-{}", start + 1, end);
let mut resp = json!({
    "status": "ok",
    "removed_range": removed_range,
    "line_count": line_count,
});
if let Some(h) = hint {
    resp["worktree_hint"] = json!(h);
}
Ok(resp)
```
After:
```rust
let line_count = end - start;
let removed_range = format!("{}-{}", start + 1, end);
Ok(json!({
    "status": "ok",
    "removed_range": removed_range,
    "line_count": line_count,
}))
```

**Step 7: Simplify `insert_code` in `symbol.rs` (line ~1523)**

Before:
```rust
let hint = crate::util::path_security::worktree_hint(&root);
let mut resp = json!({ "status": "ok", "inserted_at_line": insert_at + 1, "position": position });
if let Some(h) = hint {
    resp["worktree_hint"] = json!(h);
}
Ok(resp)
```
After:
```rust
Ok(json!({ "status": "ok", "inserted_at_line": insert_at + 1, "position": position }))
```

**Step 8: Simplify `rename_symbol` in `symbol.rs` (line ~1883)**

Before:
```rust
if let Some(h) = crate::util::path_security::worktree_hint(&rename_root) {
    result["worktree_hint"] = json!(h);
}
Ok(result)
```
After (just delete those two lines):
```rust
Ok(result)
```
Also delete the `let rename_root = ...` line above if it was only used for the hint call.
Check: search for `rename_root` at that point — if only used for the hint, delete it.

**Step 9: Run tests**

```bash
cargo test
cargo clippy -- -D warnings
```
Expected: all tests pass, no clippy warnings (no dead code for `worktree_hint` anymore).

**Step 10: Commit**

```bash
git add src/util/path_security.rs src/tools/file.rs src/tools/symbol.rs
git commit -m "refactor: remove worktree_hint advisory from all write-tool responses

The guard_worktree_write hard-block is sufficient protection.
The advisory hint added noise to every write response when worktrees exist."
```

---

### Task 3: Add `get_with_refresh_flag` to `OutputBuffer`

**Files:**
- Modify: `src/tools/output_buffer.rs`
- Test: `src/tools/output_buffer.rs` (inline tests)

**Step 1: Write a failing test**

In the `tests` module of `output_buffer.rs`, add after the existing `get_file_handle_refreshes_when_file_modified` test:

```rust
#[test]
fn get_with_refresh_flag_returns_true_when_file_changed() {
    use std::fs;
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.txt");
    fs::write(&path, "original").unwrap();

    let buf = OutputBuffer::new(10);
    let id = buf.store_file(path.to_string_lossy().to_string(), "original".to_string());

    // Modify the file on disk
    fs::write(&path, "modified").unwrap();
    // Touch mtime so it's definitely newer
    let now = std::time::SystemTime::now();
    filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(now)).unwrap();

    let (entry, was_refreshed) = buf.get_with_refresh_flag(&id).unwrap();
    assert!(was_refreshed, "should report refresh when file changed");
    assert_eq!(entry.stdout, "modified");
}

#[test]
fn get_with_refresh_flag_returns_false_when_file_unchanged() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.txt");
    fs::write(&path, "content").unwrap();

    let buf = OutputBuffer::new(10);
    let id = buf.store_file(path.to_string_lossy().to_string(), "content".to_string());

    let (entry, was_refreshed) = buf.get_with_refresh_flag(&id).unwrap();
    assert!(!was_refreshed, "should not report refresh when file unchanged");
    assert_eq!(entry.stdout, "content");
}

#[test]
fn get_with_refresh_flag_returns_false_for_cmd_entries() {
    let buf = OutputBuffer::new(10);
    let id = buf.store("echo hi".to_string(), "hi".to_string(), String::new(), 0);

    let (entry, was_refreshed) = buf.get_with_refresh_flag(&id).unwrap();
    assert!(!was_refreshed, "cmd entries never refresh");
    assert_eq!(entry.stdout, "hi");
}
```

Note: if `filetime` is not in `Cargo.toml`, use a 1-second sleep + write instead:
```rust
std::thread::sleep(std::time::Duration::from_millis(10));
fs::write(&path, "modified").unwrap();
```
Check `Cargo.toml` before adding a dependency — prefer sleep if `filetime` isn't already there.

**Step 2: Run tests to verify they fail**

```bash
cargo test get_with_refresh_flag -- --nocapture
```
Expected: FAIL — `get_with_refresh_flag` not found.

**Step 3: Implement `get_with_refresh_flag`**

In `output_buffer.rs`, add this method to `impl OutputBuffer` immediately after the existing `get()` method:

```rust
/// Like [`get`], but also returns whether the entry was refreshed from disk.
/// Only `@file_*` entries (those with `source_path` set) can refresh; all
/// others always return `false`.
pub fn get_with_refresh_flag(&self, id: &str) -> Option<(BufferEntry, bool)> {
    let canonical = OutputBuffer::canonical_id(id);
    let mut inner = self.0.lock().unwrap();

    // Check if a refresh is needed before calling the refresh logic.
    let needs_refresh = if let Some(entry) = inner.entries.get(canonical) {
        if let Some(ref path) = entry.source_path {
            match std::fs::metadata(path) {
                Err(_) => return None, // file gone
                Ok(meta) => {
                    let mtime_ms = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    mtime_ms > entry.timestamp
                }
            }
        } else {
            false
        }
    } else {
        return None;
    };

    if needs_refresh {
        let path = inner.entries[canonical].source_path.clone().unwrap();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let now = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if let Some(entry) = inner.entries.get_mut(canonical) {
                    entry.stdout = content;
                    entry.timestamp = now;
                }
            }
            Err(_) => return None,
        }
    }

    inner.entries.get_refresh(canonical).cloned().map(|e| (e, needs_refresh))
}
```

**Important:** Look at how `get()` uses `inner.entries.get_refresh(canonical)` for LRU promotion — use the same call. If the `LruCache` API differs, mirror `get()` exactly.

**Step 4: Run tests to verify they pass**

```bash
cargo test get_with_refresh_flag -- --nocapture
```
Expected: 3 tests PASS.

**Step 5: Commit**

```bash
cargo fmt
git add src/tools/output_buffer.rs
git commit -m "feat(output_buffer): add get_with_refresh_flag to detect disk refreshes"
```

---

### Task 4: Extend `resolve_refs` and surface refresh in `run_command`

**Files:**
- Modify: `src/tools/output_buffer.rs` — `resolve_refs` signature + implementation
- Modify: `src/tools/workflow.rs` — consume 4th element, prepend refresh lines

**Step 1: Write failing tests**

In `output_buffer.rs` tests, add after the `resolve_refs_*` group:

```rust
#[test]
fn resolve_refs_reports_refreshed_file_handle() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.txt");
    fs::write(&path, "original").unwrap();

    let buf = OutputBuffer::new(10);
    let id = buf.store_file(path.to_string_lossy().to_string(), "original".to_string());

    // Modify the file so it looks newer
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&path, "updated").unwrap();

    let cmd = format!("cat {}", id);
    let (_resolved, _temps, _buffer_only, refreshed) = buf.resolve_refs(&cmd).unwrap();
    assert_eq!(refreshed, vec![id], "should report the refreshed handle");
}

#[test]
fn resolve_refs_no_refresh_for_unchanged_file() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.txt");
    fs::write(&path, "content").unwrap();

    let buf = OutputBuffer::new(10);
    let id = buf.store_file(path.to_string_lossy().to_string(), "content".to_string());

    let cmd = format!("cat {}", id);
    let (_resolved, _temps, _buffer_only, refreshed) = buf.resolve_refs(&cmd).unwrap();
    assert!(refreshed.is_empty(), "no refresh when file unchanged");
}

#[test]
fn resolve_refs_no_refresh_for_cmd_handle() {
    let buf = OutputBuffer::new(10);
    let id = buf.store("cmd".to_string(), "output".to_string(), String::new(), 0);

    let cmd = format!("grep foo {}", id);
    let (_resolved, _temps, _buffer_only, refreshed) = buf.resolve_refs(&cmd).unwrap();
    assert!(refreshed.is_empty(), "cmd handles never refresh");
}
```

Also update ALL existing `resolve_refs` tests that destructure the 3-tuple to use a 4-tuple:
```rust
// Before:
let (cmd, files, is_buffer_only) = buf.resolve_refs(&cmd).unwrap();
// After:
let (cmd, files, is_buffer_only, _refreshed) = buf.resolve_refs(&cmd).unwrap();
```
Do this for: `resolve_refs_no_refs`, `resolve_refs_single_ref`, `resolve_refs_stderr_suffix`,
`resolve_refs_multiple_refs`, `resolve_refs_not_buffer_only_with_real_paths`,
`resolve_refs_temp_files_are_readonly` (and any others that destructure).

**Step 2: Run tests to verify they fail**

```bash
cargo test resolve_refs -- --nocapture
```
Expected: FAIL — return type mismatch (still 3-tuple).

**Step 3: Extend `resolve_refs` in `output_buffer.rs`**

Change the signature:
```rust
pub fn resolve_refs(&self, command: &str) -> Result<(String, Vec<PathBuf>, bool, Vec<String>)> {
```

In the early-return for empty refs:
```rust
return Ok((command.to_string(), vec![], false, vec![]));
```

Replace the inner `self.get(base_id)` call with `self.get_with_refresh_flag(base_id)`:
```rust
let (entry, was_refreshed) = self
    .get_with_refresh_flag(base_id)
    .ok_or_else(|| RecoverableError::with_hint(
        format!("buffer reference not found: {}", token),
        "Buffer refs expire when the session resets. Re-run the command to get a fresh ref.",
    ))?;

if was_refreshed {
    refreshed_handles.push(token.to_string()); // token is e.g. "@file_abc12345"
}
```

Add `let mut refreshed_handles: Vec<String> = Vec::new();` at the top of the function (alongside `temp_paths`).

Return it at the end:
```rust
Ok((result, temp_paths, is_buffer_only, refreshed_handles))
```

**Step 4: Run tests to verify they pass**

```bash
cargo test resolve_refs -- --nocapture
```
Expected: all tests PASS.

**Step 5: Update `resolve_refs` caller in `workflow.rs`**

Find the destructuring in `RunCommand::call`:
```rust
let (resolved_command, temp_files, buffer_only) =
    ctx.output_buffer.resolve_refs(command)?;
```
Change to:
```rust
let (resolved_command, temp_files, buffer_only, refreshed_handles) =
    ctx.output_buffer.resolve_refs(command)?;
```

After `run_command_inner` returns `result`, prepend refresh lines to stdout:
```rust
let mut result = run_command_inner(...).await;
OutputBuffer::cleanup_temp_files(&temp_files);

// Inject refresh indicator lines into stdout
if let Ok(ref mut val) = result {
    if !refreshed_handles.is_empty() {
        let prefix: String = refreshed_handles
            .iter()
            .map(|id| format!("↻ {} refreshed from disk (file changed since last read)\n", id))
            .collect();
        if let Some(stdout) = val["stdout"].as_str() {
            val["stdout"] = json!(format!("{}{}", prefix, stdout));
        }
    }
}

result
```

**Step 6: Run full test suite**

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```
Expected: all pass.

**Step 7: Commit**

```bash
cargo fmt
git add src/tools/output_buffer.rs src/tools/workflow.rs
git commit -m "feat(output_buffer): report @file_* refresh in resolve_refs; surface in run_command output

When a @file_* handle is queried via run_command and the underlying file
has changed on disk, prepend a '↻ @file_xxx refreshed from disk' line to
the command output so the agent knows the content is fresh."
```

---

## Verification

After all tasks:

```bash
cargo test
cargo clippy -- -D warnings
```

Confirm:
- `get_usage_stats` no longer in `server_instructions.md`
- `worktree_hint` field absent from all write-tool responses
- `resolve_refs` returns 4-tuple with refreshed handle list
- `run_command` prepends `↻ ... refreshed` when `@file_*` auto-refreshes
- Tool counts say 32 in both `system-prompt.md` and `CLAUDE.md`
- `list_symbols` (not `get_symbols_overview`) in `onboarding_prompt.md`
