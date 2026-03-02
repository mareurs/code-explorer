# File Handle Auto-Refresh Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** When the file behind a `@file_*` buffer handle changes on disk, `get()` transparently re-reads it so agents never operate on stale content.

**Architecture:** Add `source_path: Option<PathBuf>` to `BufferEntry`. `store_file()` sets it; `store()` and `store_tool()` leave it `None`. `get()` checks mtime for entries with a `source_path` and refreshes in-place under the existing mutex if the file is newer. Deleted file → `None`.

**Tech Stack:** Rust, `std::fs::metadata`, `std::time::SystemTime`. Single file: `src/tools/output_buffer.rs`.

---

### Task 1: Add `source_path` field to `BufferEntry` and update all constructors

**Files:**
- Modify: `src/tools/output_buffer.rs:19-25` (`BufferEntry` struct)
- Modify: `src/tools/output_buffer.rs:91-97` (`store()` entry construction)
- Modify: `src/tools/output_buffer.rs` (`store_file()` and `store_tool()` entry construction)

**Step 1: Write the failing test**

Add this test inside the `tests` module (around line 395, near the other `store_file` tests):

```rust
#[test]
fn store_file_sets_source_path() {
    let buf = OutputBuffer::new(10);
    let id = buf.store_file("/tmp/foo.rs".to_string(), "content".to_string());
    let entry = buf.get(&id).unwrap();
    assert_eq!(entry.source_path, Some(PathBuf::from("/tmp/foo.rs")));
}

#[test]
fn store_cmd_has_no_source_path() {
    let buf = OutputBuffer::new(10);
    let id = buf.store("echo hi".to_string(), "hi".to_string(), "".to_string(), 0);
    let entry = buf.get(&id).unwrap();
    assert_eq!(entry.source_path, None);
}

#[test]
fn store_tool_has_no_source_path() {
    let buf = OutputBuffer::new(10);
    let id = buf.store_tool("my_tool", "output".to_string());
    let entry = buf.get(&id).unwrap();
    assert_eq!(entry.source_path, None);
}
```

**Step 2: Run tests to confirm they fail**

```bash
cargo test store_file_sets_source_path store_cmd_has_no_source_path store_tool_has_no_source_path 2>&1 | grep -E "FAILED|error"
```

Expected: compile error — `source_path` field doesn't exist yet.

**Step 3: Add `source_path` to `BufferEntry`**

In `src/tools/output_buffer.rs`, update the struct (lines 18-25):

```rust
/// A single buffered command result.
#[derive(Debug, Clone)]
pub struct BufferEntry {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timestamp: u64,
    /// Set only for `@file_*` entries. Enables mtime-based auto-refresh in `get()`.
    pub source_path: Option<PathBuf>,
}
```

**Step 4: Update `store()` constructor**

In `store()`, update the `BufferEntry` construction (around line 91):

```rust
let entry = BufferEntry {
    command,
    stdout,
    stderr,
    exit_code,
    timestamp: now,
    source_path: None,
};
```

**Step 5: Update `store_file()` constructor**

In `store_file()`, update the `BufferEntry` construction (around line 143):

```rust
let entry = BufferEntry {
    command: path.clone(),
    stdout: content,
    stderr: String::new(),
    exit_code: 0,
    timestamp: now,
    source_path: Some(PathBuf::from(&path)),
};
```

**Step 6: Update `store_tool()` constructor**

In `store_tool()`, update the `BufferEntry` construction (around line 172):

```rust
let entry = BufferEntry {
    command: tool_name.to_string(),
    stdout: content,
    stderr: String::new(),
    exit_code: 0,
    timestamp: now,
    source_path: None,
};
```

**Step 7: Run the three new tests**

```bash
cargo test store_file_sets_source_path store_cmd_has_no_source_path store_tool_has_no_source_path 2>&1 | grep -E "ok|FAILED"
```

Expected: all three pass.

**Step 8: Run full test suite**

```bash
cargo test 2>&1 | tail -5
```

Expected: all tests pass (no regressions — `source_path: None` default for existing tests).

**Step 9: Commit**

```bash
git add src/tools/output_buffer.rs
git commit -m "feat(output_buffer): add source_path field to BufferEntry for file-backed handles"
```

---

### Task 2: Add mtime-refresh logic to `get()`

**Files:**
- Modify: `src/tools/output_buffer.rs:108-121` (`get()` method)

**Step 1: Write the failing tests**

Add these tests inside the `tests` module:

```rust
#[test]
fn get_file_handle_refreshes_when_file_modified() {
    use std::fs;
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");

    // Write initial content
    fs::write(&file_path, "original content").unwrap();

    let buf = OutputBuffer::new(10);
    let id = buf.store_file(
        file_path.to_string_lossy().to_string(),
        "original content".to_string(),
    );

    // Sandwich step 1: verify cached content
    assert_eq!(buf.get(&id).unwrap().stdout, "original content");

    // Sandwich step 2: modify file on disk, ensure mtime advances
    // Sleep briefly so mtime differs (filesystem resolution is usually 1s on Linux)
    std::thread::sleep(std::time::Duration::from_millis(10));
    // Force mtime to be in the future relative to timestamp by touching via write
    fs::write(&file_path, "updated content").unwrap();
    // Set mtime explicitly to be clearly after the stored timestamp
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    filetime::set_file_mtime(&file_path, filetime::FileTime::from_system_time(future)).unwrap();

    // Sandwich step 3: get() should return fresh content
    let entry = buf.get(&id).unwrap();
    assert_eq!(entry.stdout, "updated content");
}

#[test]
fn get_file_handle_returns_none_when_file_deleted() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "hello").unwrap();

    let buf = OutputBuffer::new(10);
    let id = buf.store_file(
        file_path.to_string_lossy().to_string(),
        "hello".to_string(),
    );

    assert!(buf.get(&id).is_some());

    // Delete the file
    fs::remove_file(&file_path).unwrap();

    assert!(buf.get(&id).is_none());
}

#[test]
fn get_file_handle_unmodified_returns_cached() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "stable content").unwrap();

    let buf = OutputBuffer::new(10);
    let id = buf.store_file(
        file_path.to_string_lossy().to_string(),
        "stable content".to_string(),
    );

    // Two gets without touching the file — both return cached content
    assert_eq!(buf.get(&id).unwrap().stdout, "stable content");
    assert_eq!(buf.get(&id).unwrap().stdout, "stable content");
}

#[test]
fn get_cmd_handle_not_affected_by_refresh_logic() {
    let buf = OutputBuffer::new(10);
    let id = buf.store("echo hi".to_string(), "hi".to_string(), "".to_string(), 0);
    // No source_path — should behave exactly as before
    let entry = buf.get(&id).unwrap();
    assert_eq!(entry.stdout, "hi");
}
```

> **Note on `filetime` crate:** Check if it's already in `Cargo.toml`. If not, add it as a dev-dependency:
> ```toml
> [dev-dependencies]
> filetime = "0.2"
> ```
> `filetime` lets us set mtime explicitly, which is more reliable than sleeping for filesystem resolution.

**Step 2: Run tests to confirm they fail**

```bash
cargo test get_file_handle_refreshes get_file_handle_returns_none get_file_handle_unmodified get_cmd_handle_not_affected 2>&1 | grep -E "FAILED|error"
```

Expected: compile error or test failures — refresh logic doesn't exist yet.

**Step 3: Check if `filetime` is needed**

```bash
grep filetime Cargo.toml
```

If absent, add to `[dev-dependencies]` in `Cargo.toml`:

```toml
filetime = "0.2"
```

**Step 4: Implement mtime-refresh in `get()`**

Replace the current `get()` implementation (lines 104-121) with:

```rust
/// Get an entry by handle, refreshing its LRU position.
///
/// For `@file_*` handles (entries with `source_path` set), checks the file's
/// mtime against the stored timestamp. If the file is newer, re-reads its content
/// and updates the entry in-place. If the file is gone, returns `None`.
///
/// Supports a `.err` suffix on the handle (e.g. `@cmd_xxx.err`),
/// which returns the same entry (caller decides what to extract).
pub fn get(&self, id: &str) -> Option<BufferEntry> {
    let canonical = id.strip_suffix(".err").unwrap_or(id);
    let mut inner = self.inner.lock().unwrap();

    if !inner.entries.contains_key(canonical) {
        return None;
    }

    // For file-backed entries: check mtime and refresh if stale.
    let needs_refresh = if let Some(entry) = inner.entries.get(canonical) {
        if let Some(ref path) = entry.source_path {
            match std::fs::metadata(path) {
                Err(_) => {
                    // File gone or unreadable — evict and return None.
                    inner.order.retain(|k| k != canonical);
                    inner.entries.remove(canonical);
                    return None;
                }
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
        false
    };

    if needs_refresh {
        let path = inner.entries[canonical].source_path.clone().unwrap();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if let Some(entry) = inner.entries.get_mut(canonical) {
                    entry.stdout = content;
                    entry.timestamp = now;
                }
            }
            Err(_) => {
                // Became unreadable between the stat and the read — evict.
                inner.order.retain(|k| k != canonical);
                inner.entries.remove(canonical);
                return None;
            }
        }
    }

    // Refresh LRU order: move to end.
    if let Some(pos) = inner.order.iter().position(|k| k == canonical) {
        inner.order.remove(pos);
        inner.order.push(canonical.to_string());
    }
    inner.entries.get(canonical).cloned()
}
```

**Step 5: Run the four new tests**

```bash
cargo test get_file_handle_refreshes get_file_handle_returns_none get_file_handle_unmodified get_cmd_handle_not_affected 2>&1 | grep -E "ok|FAILED"
```

Expected: all four pass.

**Step 6: Run full test suite**

```bash
cargo test 2>&1 | tail -5
```

Expected: all tests pass.

**Step 7: Clippy**

```bash
cargo clippy -- -D warnings 2>&1 | grep -E "error|warning"
```

Expected: clean.

**Step 8: Commit**

```bash
git add src/tools/output_buffer.rs Cargo.toml
git commit -m "feat(output_buffer): auto-refresh @file_* handles when file changes on disk"
```

---

### Task 3: Final verification

**Step 1: Full test suite + clippy + fmt**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt --check
```

Expected: all pass, no warnings, no formatting issues.

**Step 2: Verify the fix end-to-end with a manual smoke test**

```bash
# Build the server
cargo build 2>&1 | tail -3
```

Expected: builds clean.

**Step 3: Update docs/TODO-tool-misbehaviors.md if there's an entry for this bug**

Check if the file mentions stale file handles:

```bash
grep -i "stale\|file.*handle\|@file" docs/TODO-tool-misbehaviors.md
```

If there's an entry, mark it resolved with the commit SHA:

```bash
git log --oneline -3
```

**Step 4: Final commit (only if docs changed)**

```bash
git add docs/TODO-tool-misbehaviors.md
git commit -m "docs: mark @file_* stale handle bug resolved"
```
