//! Session-scoped, in-memory LRU buffer for command output.
//!
//! Stores stdout/stderr from `run_command` calls and returns opaque
//! handles (`@cmd_<8hex>`) that the LLM can pass to future tools.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tools::RecoverableError;
use anyhow::Result;
use regex::Regex;
use tempfile::NamedTempFile;

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

/// A dangerous command held pending agent acknowledgment.
#[derive(Debug, Clone)]
pub struct PendingAckCommand {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: u64,
}

/// A multi-line source edit held pending agent acknowledgment.
#[derive(Debug, Clone)]
pub struct PendingAckEdit {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}

/// Thread-safe LRU buffer for command output.
///
/// `store()` inserts an entry and returns an opaque `@cmd_<8hex>` handle.
/// `get()` retrieves it and refreshes the LRU position.
/// When capacity is reached the least-recently-used entry is evicted.
pub struct OutputBuffer {
    inner: Mutex<BufferInner>,
}

struct BufferInner {
    entries: HashMap<String, BufferEntry>,
    /// LRU order: index 0 = oldest (evict first), last = most recently used.
    order: Vec<String>,
    max_entries: usize,
    counter: u64,
    // --- pending-ack store (commands) ---
    pending_acks: HashMap<String, PendingAckCommand>,
    pending_order: Vec<String>,
    // --- pending-ack store (source edits) ---
    pending_edits: HashMap<String, PendingAckEdit>,
    pending_edits_order: Vec<String>,
    max_pending: usize,
    // --- background job store ---
    background_jobs: HashMap<String, PathBuf>,
    background_order: Vec<String>,
}

impl OutputBuffer {
    /// Create a new buffer with the given maximum capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(BufferInner {
                entries: HashMap::new(),
                order: Vec::new(),
                max_entries,
                counter: 0,
                pending_acks: HashMap::new(),
                pending_order: Vec::new(),
                pending_edits: HashMap::new(),
                pending_edits_order: Vec::new(),
                max_pending: 20,
                background_jobs: HashMap::new(),
                background_order: Vec::new(),
            }),
        }
    }

    /// Store command output and return an opaque handle (`@cmd_<8hex>`).
    pub fn store(&self, command: String, stdout: String, stderr: String, exit_code: i32) -> String {
        let mut inner = self.inner.lock().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        inner.counter = inner.counter.wrapping_add(1);
        // Truncate to u32 so the hex portion is always exactly 8 characters.
        let id = format!("@cmd_{:08x}", now.wrapping_add(inner.counter) as u32);

        // Evict oldest if at capacity
        if inner.entries.len() >= inner.max_entries {
            if let Some(oldest_id) = inner.order.first().cloned() {
                inner.order.remove(0);
                inner.entries.remove(&oldest_id);
            }
        }

        let entry = BufferEntry {
            command,
            stdout,
            stderr,
            exit_code,
            timestamp: now,
            source_path: None,
        };

        inner.entries.insert(id.clone(), entry);
        inner.order.push(id.clone());
        id
    }

    /// Get an entry by handle, refreshing its LRU position.
    ///
    /// For `@file_*` handles (entries with `source_path` set), checks the file's
    /// mtime against the stored timestamp. If the file is newer, re-reads its content
    /// and updates the entry in-place. If the file is gone or unreadable, returns `None`.
    ///
    /// Supports a `.err` suffix on the handle (e.g. `@cmd_xxx.err`),
    /// which returns the same entry (caller decides what to extract).
    pub fn get(&self, id: &str) -> Option<BufferEntry> {
        self.get_with_refresh_flag(id).map(|(entry, _)| entry)
    }

    /// Like [`get`], but also returns whether the entry was refreshed from disk.
    /// Only `@file_*` entries with `source_path` set can refresh; all others return `false`.
    pub fn get_with_refresh_flag(&self, id: &str) -> Option<(BufferEntry, bool)> {
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
                    // Became unreadable between stat and read — evict.
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
        inner
            .entries
            .get(canonical)
            .cloned()
            .map(|e| (e, needs_refresh))
    }

    /// Store file content under a `@file_*` handle.
    ///
    /// Content goes in `stdout`; `stderr` is empty; `exit_code` is 0.
    /// The `command` field holds the source path for diagnostics.
    pub fn store_file(&self, path: String, content: String) -> String {
        let mut inner = self.inner.lock().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        inner.counter = inner.counter.wrapping_add(1);
        let id = format!("@file_{:08x}", now.wrapping_add(inner.counter) as u32);

        if inner.entries.len() >= inner.max_entries {
            if let Some(oldest_id) = inner.order.first().cloned() {
                inner.order.remove(0);
                inner.entries.remove(&oldest_id);
            }
        }
        let entry = BufferEntry {
            command: path.clone(),
            stdout: content,
            stderr: String::new(),
            exit_code: 0,
            timestamp: now,
            source_path: Some(PathBuf::from(&path)),
        };
        inner.entries.insert(id.clone(), entry);
        inner.order.push(id.clone());
        id
    }

    /// Store tool output under a `@tool_*` handle.
    ///
    /// Content goes in `stdout`; `stderr` is empty; `exit_code` is 0.
    /// The `command` field holds the tool name for diagnostics.
    pub fn store_tool(&self, tool_name: &str, content: String) -> String {
        let mut inner = self.inner.lock().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        inner.counter = inner.counter.wrapping_add(1);
        let id = format!("@tool_{:08x}", now.wrapping_add(inner.counter) as u32);

        if inner.entries.len() >= inner.max_entries {
            if let Some(oldest_id) = inner.order.first().cloned() {
                inner.order.remove(0);
                inner.entries.remove(&oldest_id);
            }
        }
        let entry = BufferEntry {
            command: tool_name.to_string(),
            stdout: content,
            stderr: String::new(),
            exit_code: 0,
            timestamp: now,
            source_path: None,
        };
        inner.entries.insert(id.clone(), entry);
        inner.order.push(id.clone());
        id
    }

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

        inner.pending_acks.insert(
            id.clone(),
            PendingAckCommand {
                command,
                cwd,
                timeout_secs,
            },
        );
        inner.pending_order.push(id.clone());
        id
    }

    /// Retrieve a stored pending ack by handle.
    ///
    /// Does not consume the entry — LRU eviction handles cleanup.
    /// Returns `None` if the handle is unknown or has been evicted.
    pub fn get_dangerous(&self, handle: &str) -> Option<PendingAckCommand> {
        let inner = self.inner.lock().unwrap();
        inner.pending_acks.get(handle).cloned()
    }

    /// Store a multi-line source edit pending acknowledgement and return an `@ack_*` handle.
    pub fn store_pending_edit(
        &self,
        path: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
    ) -> String {
        let mut inner = self.inner.lock().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        inner.counter = inner.counter.wrapping_add(1);
        let id = format!("@ack_{:08x}", now.wrapping_add(inner.counter) as u32);

        if inner.pending_edits.len() >= inner.max_pending {
            if let Some(oldest) = inner.pending_edits_order.first().cloned() {
                inner.pending_edits_order.remove(0);
                inner.pending_edits.remove(&oldest);
            }
        }

        inner.pending_edits.insert(
            id.clone(),
            PendingAckEdit {
                path,
                old_string,
                new_string,
                replace_all,
            },
        );
        inner.pending_edits_order.push(id.clone());
        id
    }

    /// Retrieve a stored pending edit by handle.
    pub fn get_pending_edit(&self, handle: &str) -> Option<PendingAckEdit> {
        let inner = self.inner.lock().unwrap();
        inner.pending_edits.get(handle).cloned()
    }

    /// Store a background job log path and return a `@bg_<8hex>` handle.
    pub fn store_background(&self, log_path: PathBuf) -> String {
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
    pub fn get_background(&self, id: &str) -> Option<PathBuf> {
        let inner = self.inner.lock().unwrap();
        inner.background_jobs.get(id).cloned()
    }

    /// Resolve `@cmd_<8hex>` (and `@cmd_<8hex>.err`) references in a command string.
    ///
    /// Each reference is replaced with a path to a read-only temp file containing
    /// the corresponding stdout (or stderr for `.err` suffix). Returns:
    /// - The modified command string with references replaced by temp file paths
    /// - A list of temp file paths the caller must clean up via [`cleanup_temp_files`]
    /// - `is_buffer_only`: true if every file-path-like argument in the resulting
    ///   command is one of our temp files (i.e., the command operates solely on
    ///   buffered output, not real filesystem paths)
    /// - `refreshed_handles`: canonical handle IDs (e.g. `@file_abc123`) that were
    ///   auto-refreshed from disk because the underlying file had changed
    pub fn resolve_refs(&self, command: &str) -> Result<(String, Vec<PathBuf>, bool, Vec<String>)> {
        // Guard: @ack_* handles are for deferred execution, not content interpolation.
        if Regex::new(r"@ack_[0-9a-f]{8}")
            .expect("valid regex")
            .is_match(command)
        {
            return Err(RecoverableError::with_hint(
                "ack handle cannot be used for interpolation",
                "Use run_command(\"@ack_<id>\") directly to execute a pending acknowledgment.",
            )
            .into());
        }

        let re = Regex::new(r"@(?:cmd|file|tool|bg)_[0-9a-f]{8}(\.err)?").expect("valid regex");

        let refs: Vec<&str> = re.find_iter(command).map(|m| m.as_str()).collect();
        if refs.is_empty() {
            return Ok((command.to_string(), vec![], false, vec![]));
        }

        // Deduplicate while preserving order (same ref token may appear twice).
        let mut seen = std::collections::HashSet::new();
        let unique_refs: Vec<&str> = refs.iter().filter(|r| seen.insert(**r)).copied().collect();

        let mut result = command.to_string();
        let mut temp_paths: Vec<PathBuf> = Vec::new();
        let mut temp_path_strings: Vec<String> = Vec::new();
        let mut refreshed_handles: Vec<String> = Vec::new();

        for token in &unique_refs {
            let is_stderr = token.ends_with(".err");
            let base_id = if is_stderr {
                &token[..token.len() - 4] // strip ".err"
            } else {
                token
            };

            // @bg_* refs always read fresh from disk — no snapshot caching.
            if base_id.starts_with("@bg_") {
                if is_stderr {
                    return Err(RecoverableError::with_hint(
                        format!("@bg_* refs do not support the .err suffix: {}", token),
                        "Background job logs capture stdout and stderr in one file. Use the bare handle (without .err).",
                    )
                    .into());
                }
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

            let (entry, was_refreshed) = self
                .get_with_refresh_flag(base_id)
                .ok_or_else(|| RecoverableError::with_hint(
                    format!("buffer reference not found: {}", token),
                    "Buffer refs expire when the session resets. Re-run the command to get a fresh ref.",
                ))?;

            if was_refreshed {
                refreshed_handles.push(token.to_string());
            }

            let content = if is_stderr {
                &entry.stderr
            } else {
                &entry.stdout
            };

            // Write content to a temp file.
            // @tool_* refs contain compact single-line JSON (tool result serialized
            // by serde_json::to_string).  Pretty-print it before writing so that
            // grep/sed on the temp file matches individual fields/values instead of
            // the entire JSON blob on one line.  Non-JSON content is written as-is.
            let pretty_content: String;
            let write_content: &str = if base_id.starts_with("@tool_") && !is_stderr {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
                    pretty_content =
                        serde_json::to_string_pretty(&v).unwrap_or_else(|_| content.to_string());
                    &pretty_content
                } else {
                    content
                }
            } else {
                content
            };
            let mut tmp = NamedTempFile::new()?;
            tmp.write_all(write_content.as_bytes())?;
            tmp.flush()?;

            let path = tmp.path().to_path_buf();

            // Make read-only on unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o444);
                std::fs::set_permissions(&path, perms)?;
            }

            // Persist the file (prevent auto-deletion when NamedTempFile drops).
            // into_temp_path() gives us a TempPath that would delete on drop,
            // so we forget it — the caller cleans up via cleanup_temp_files.
            let temp_path = tmp.into_temp_path();
            let path = temp_path.to_path_buf();
            std::mem::forget(temp_path);

            let path_str = path.to_string_lossy().to_string();
            // Replace all occurrences of this token with the temp path
            result = result.replace(token, &path_str);
            temp_path_strings.push(path_str);
            temp_paths.push(path);
        }

        // Determine is_buffer_only: true only when every path-like argument is one
        // of our own injected temp files. Relative paths without a ./ prefix (e.g.
        // "src/main.rs") are also treated as non-buffer-only.
        let is_buffer_only = !shell_words(&result).iter().any(|word| {
            let is_temp = temp_path_strings
                .iter()
                .any(|tp| word.contains(tp.as_str()));
            if is_temp {
                return false;
            }
            // Absolute or explicitly-relative paths
            if word.starts_with('/') || word.starts_with("./") {
                return true;
            }
            // Relative paths with a directory separator but no leading flag sigil
            !word.starts_with('-') && word.contains('/')
        });

        Ok((result, temp_paths, is_buffer_only, refreshed_handles))
    }

    /// Remove temp files created by [`resolve_refs`].
    pub fn cleanup_temp_files(paths: &[PathBuf]) {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }

    /// True when the command operates only on buffer refs (no bare filesystem paths).
    ///
    /// Buffer-only commands skip shell_command_mode checks and the
    /// dangerous-command speed bump because they cannot modify the real
    /// filesystem — they only read from in-memory output buffers materialized
    /// as read-only temp files.
    pub fn is_buffer_only(command: &str) -> bool {
        if !command.contains("@cmd_") && !command.contains("@file_") && !command.contains("@tool_")
        {
            return false;
        }
        // Reject if any whitespace-separated word looks like a path.
        for word in command.split_whitespace() {
            // Absolute or explicitly-relative paths
            if word.starts_with('/') || word.starts_with("./") || word.starts_with("../") {
                return false;
            }
            // Relative paths with a directory separator but no leading flag sigil
            // (e.g. "src/main.rs") — flags like "--format=a/b" are excluded by the '-' check.
            if !word.starts_with('-') && word.contains('/') {
                return false;
            }
        }
        true
    }
}

/// Naive shell word splitter: splits on whitespace, respecting single/double quotes.
/// Used only for `is_buffer_only` heuristic — not a full POSIX parser.
fn shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape_next = false;

    for ch in s.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if !in_single => escape_next = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_get() {
        let buf = OutputBuffer::new(10);
        let id = buf.store("echo hello".into(), "hello\n".into(), String::new(), 0);
        assert!(id.starts_with("@cmd_"));
        assert_eq!(id.len(), "@cmd_".len() + 8); // 8 hex chars

        let entry = buf.get(&id).expect("entry should exist");
        assert_eq!(entry.command, "echo hello");
        assert_eq!(entry.stdout, "hello\n");
        assert_eq!(entry.stderr, "");
        assert_eq!(entry.exit_code, 0);
    }

    #[test]
    fn get_missing_returns_none() {
        let buf = OutputBuffer::new(10);
        assert!(buf.get("@cmd_deadbeef").is_none());
    }

    #[test]
    fn lru_eviction() {
        let buf = OutputBuffer::new(3);
        let id1 = buf.store("cmd1".into(), "out1".into(), String::new(), 0);
        let id2 = buf.store("cmd2".into(), "out2".into(), String::new(), 0);
        let _id3 = buf.store("cmd3".into(), "out3".into(), String::new(), 0);

        // At capacity (3). Storing a 4th should evict id1 (oldest).
        let _id4 = buf.store("cmd4".into(), "out4".into(), String::new(), 0);

        assert!(buf.get(&id1).is_none(), "oldest entry should be evicted");
        assert!(buf.get(&id2).is_some(), "second entry should survive");
    }

    #[test]
    fn get_refreshes_lru_order() {
        let buf = OutputBuffer::new(3);
        let id1 = buf.store("cmd1".into(), "out1".into(), String::new(), 0);
        let _id2 = buf.store("cmd2".into(), "out2".into(), String::new(), 0);
        let _id3 = buf.store("cmd3".into(), "out3".into(), String::new(), 0);

        // Access id1 to refresh its LRU position (move to end).
        buf.get(&id1);

        // Now store a 4th entry — should evict id2 (the true oldest after refresh).
        let _id4 = buf.store("cmd4".into(), "out4".into(), String::new(), 0);

        assert!(buf.get(&id1).is_some(), "refreshed entry should survive");
        assert!(
            buf.get(&_id2).is_none(),
            "un-refreshed oldest should be evicted"
        );
    }

    #[test]
    fn stderr_suffix() {
        let buf = OutputBuffer::new(10);
        let id = buf.store("failing".into(), String::new(), "error msg".into(), 1);

        let via_err = buf
            .get(&format!("{id}.err"))
            .expect(".err suffix should work");
        assert_eq!(via_err.stderr, "error msg");
        assert_eq!(via_err.command, "failing");
    }

    #[test]
    fn resolve_refs_no_refs() {
        let buf = OutputBuffer::new(20);
        let (cmd, files, is_buffer_only, _refreshed) = buf.resolve_refs("cargo test").unwrap();
        assert_eq!(cmd, "cargo test");
        assert!(files.is_empty());
        assert!(!is_buffer_only);
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_single_ref() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("prev".into(), "hello world\n".into(), "".into(), 0);
        let (cmd, files, is_buffer_only, _refreshed) =
            buf.resolve_refs(&format!("grep hello {}", id)).unwrap();
        assert!(!cmd.contains(&id));
        assert!(cmd.contains("/")); // temp file path
        assert_eq!(files.len(), 1);
        assert!(is_buffer_only);
        let content = std::fs::read_to_string(&files[0]).unwrap();
        assert_eq!(content, "hello world\n");
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_stderr_suffix() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("prev".into(), "stdout".into(), "stderr_content".into(), 1);
        let err_ref = format!("{}.err", id);
        let (cmd, files, _, _refreshed) = buf
            .resolve_refs(&format!("grep error {}", err_ref))
            .unwrap();
        assert!(!cmd.contains(&err_ref));
        let content = std::fs::read_to_string(&files[0]).unwrap();
        assert_eq!(content, "stderr_content");
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_multiple_refs() {
        let buf = OutputBuffer::new(20);
        let id1 = buf.store("cmd1".into(), "out1".into(), "".into(), 0);
        let id2 = buf.store("cmd2".into(), "out2".into(), "".into(), 0);
        let (cmd, files, is_buffer_only, _refreshed) =
            buf.resolve_refs(&format!("diff {} {}", id1, id2)).unwrap();
        assert_eq!(files.len(), 2);
        assert!(is_buffer_only);
        assert!(!cmd.contains(&id1));
        assert!(!cmd.contains(&id2));
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_missing_ref_errors() {
        let buf = OutputBuffer::new(20);
        let result = buf.resolve_refs("grep hello @cmd_deadbeef");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_refs_not_buffer_only_with_real_paths() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("prev".into(), "data".into(), "".into(), 0);
        let (_, files, is_buffer_only, _refreshed) = buf
            .resolve_refs(&format!("diff {} /etc/passwd", id))
            .unwrap();
        assert!(!is_buffer_only);
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_temp_files_are_readonly() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("prev".into(), "data".into(), "".into(), 0);
        let (_, files, _, _refreshed) = buf.resolve_refs(&format!("cat {}", id)).unwrap();
        assert_eq!(files.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&files[0]).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o444);
        }
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn store_file_uses_file_prefix() {
        let buf = OutputBuffer::new(20);
        let id = buf.store_file("src/main.rs".into(), "fn main() {}\n".into());
        assert!(id.starts_with("@file_"), "got: {}", id);
        let entry = buf.get(&id).unwrap();
        assert_eq!(entry.stdout, "fn main() {}\n");
        assert_eq!(entry.stderr, "");
    }

    #[test]
    fn resolve_refs_substitutes_cmd_ref() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("echo hi".into(), "hello\n".into(), "".into(), 0);
        let (resolved, files, _, _refreshed) =
            buf.resolve_refs(&format!("grep hello {}", id)).unwrap();
        assert!(!resolved.contains('@'), "got: {}", resolved);
        assert!(resolved.starts_with("grep hello /"));
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_substitutes_file_ref() {
        let buf = OutputBuffer::new(20);
        let id = buf.store_file("README.md".into(), "# Hello\n".into());
        let (resolved, files, _, _refreshed) = buf.resolve_refs(&format!("wc -l {}", id)).unwrap();
        assert!(!resolved.contains('@'), "got: {}", resolved);
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_err_suffix_writes_stderr() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("cmd".into(), "out".into(), "err_text".into(), 0);
        let err_ref = format!("{}.err", id);
        let (resolved, files, _, _refreshed) =
            buf.resolve_refs(&format!("grep x {}", err_ref)).unwrap();
        let tmp_path = resolved.split_whitespace().last().unwrap();
        let content = std::fs::read_to_string(tmp_path).unwrap();
        assert_eq!(content, "err_text");
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_unknown_ref_returns_error() {
        let buf = OutputBuffer::new(20);
        let result = buf.resolve_refs("grep foo @cmd_deadbeef");
        assert!(result.is_err());
    }

    #[test]
    fn is_buffer_only_true_for_unix_tools_on_refs() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("cmd".into(), "data".into(), "".into(), 0);
        let cmd = format!("grep foo {}", id);
        let (resolved, files, is_buf_only, _refreshed) = buf.resolve_refs(&cmd).unwrap();
        assert!(is_buf_only, "resolved: {}", resolved);
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn is_buffer_only_false_for_plain_commands() {
        assert!(!OutputBuffer::is_buffer_only("grep foo /etc/passwd"));
    }

    #[test]
    fn is_buffer_only_false_for_relative_path_without_dot_prefix() {
        // Baseline: pure buffer reference with no real paths → IS buffer-only
        assert!(
            OutputBuffer::is_buffer_only("grep pattern @cmd_abc1234"),
            "pure buffer command must be classified as buffer-only"
        );

        // Stale (the bug): old code only checked '/' and './' prefixes, so
        // 'src/main.rs' (no leading '.') passed through and the command was
        // wrongly classified as buffer-only, bypassing safety checks.
        // Fixed: any word containing '/' that isn't a flag is treated as a real path.
        assert!(
            !OutputBuffer::is_buffer_only("grep pattern @cmd_abc1234 src/main.rs"),
            "relative path without ./ prefix must NOT be classified as buffer-only"
        );

        // Fresh: other real-path forms are also correctly handled
        assert!(
            !OutputBuffer::is_buffer_only("grep pattern @cmd_abc1234 ./src/main.rs"),
            "./ prefix must NOT be classified as buffer-only"
        );
        assert!(
            !OutputBuffer::is_buffer_only("grep pattern @cmd_abc1234 /tmp/file.rs"),
            "absolute path must NOT be classified as buffer-only"
        );
    }

    #[test]
    fn resolve_refs_not_buffer_only_when_relative_path_arg_present() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("cmd".into(), "output".into(), "".into(), 0);

        // Baseline: command with only a buffer ref → IS buffer-only (safe to skip checks)
        let (_, files, is_buf_only, _) = buf.resolve_refs(&format!("cat {id}")).unwrap();
        OutputBuffer::cleanup_temp_files(&files);
        assert!(
            is_buf_only,
            "command with only buffer refs must be buffer-only"
        );

        // Stale (the bug): old resolve_refs also classified 'grep foo @cmd src/main.rs'
        // as buffer-only because it only checked '/' and './' prefixes on real args.
        // Fixed: relative path 'src/main.rs' now marks the command as NOT buffer-only.
        let cmd = format!("grep foo {id} src/main.rs");
        let (_, files, is_buf_only, _) = buf.resolve_refs(&cmd).unwrap();
        OutputBuffer::cleanup_temp_files(&files);
        assert!(
            !is_buf_only,
            "relative path arg must not classify command as buffer-only"
        );

        // Fresh: absolute-path arg also correctly prevents buffer-only classification
        let cmd = format!("diff {id} /etc/passwd");
        let (_, files, is_buf_only, _) = buf.resolve_refs(&cmd).unwrap();
        OutputBuffer::cleanup_temp_files(&files);
        assert!(
            !is_buf_only,
            "absolute path arg must not classify command as buffer-only"
        );
    }

    #[test]
    fn store_tool_generates_tool_ref() {
        let buf = OutputBuffer::new(10);
        let id = buf.store_tool("list_symbols", "{\"symbols\":[]}".to_string());
        assert!(
            id.starts_with("@tool_"),
            "expected @tool_ prefix, got {}",
            id
        );
    }

    #[test]
    fn store_tool_stores_as_stdout_no_stderr() {
        let buf = OutputBuffer::new(10);
        let json = "{\"symbols\":[1,2,3]}".to_string();
        let id = buf.store_tool("list_symbols", json.clone());
        let entry = buf.get(&id).unwrap();
        assert_eq!(entry.stdout, json);
        assert_eq!(entry.stderr, "");
        assert_eq!(entry.exit_code, 0);
        assert_eq!(entry.command, "list_symbols");
    }

    #[test]
    fn resolve_refs_substitutes_tool_ref() {
        let buf = OutputBuffer::new(10);
        let json = "{\"symbols\":[]}".to_string();
        let id = buf.store_tool("list_symbols", json);
        let cmd = format!("jq '.symbols' {}", id);
        let (resolved, _paths, _is_buf_only, _refreshed) = buf.resolve_refs(&cmd).unwrap();
        assert!(
            !resolved.contains("@tool_"),
            "ref should be substituted, got: {}",
            resolved
        );
    }

    #[test]
    fn store_dangerous_returns_ack_handle() {
        let buf = OutputBuffer::new(10);
        let handle = buf.store_dangerous(
            "rm -rf /dist".to_string(),
            Some("frontend/".to_string()),
            30,
        );
        assert!(
            handle.starts_with("@ack_"),
            "handle should start with @ack_, got: {handle}"
        );
    }

    #[test]
    fn get_dangerous_returns_stored_command() {
        let buf = OutputBuffer::new(10);
        let handle = buf.store_dangerous(
            "rm -rf /dist".to_string(),
            Some("frontend/".to_string()),
            10,
        );
        let cmd = buf
            .get_dangerous(&handle)
            .expect("should find stored command");
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
        let mut handles = Vec::new();
        for i in 0..21u64 {
            handles.push(buf.store_dangerous(format!("cmd_{}", i), None, 30));
        }
        assert!(
            buf.get_dangerous(&handles[0]).is_none(),
            "oldest ack should be evicted"
        );
        assert!(
            buf.get_dangerous(&handles[20]).is_some(),
            "newest ack should survive"
        );
    }

    #[test]
    fn resolve_refs_rejects_ack_handle_interpolation() {
        let buf = OutputBuffer::new(10);
        let handle = buf.store_dangerous("rm -rf /dist".to_string(), None, 30);
        let result = buf.resolve_refs(&format!("grep pattern {handle}"));
        assert!(
            result.is_err(),
            "interpolating an @ack_ handle should return an error"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("ack handle"),
            "error should mention 'ack handle', got: {msg}"
        );
    }

    #[test]
    fn store_file_sets_source_path() {
        use std::fs;

        // Use a real file — get() stats it and evicts non-existent paths.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("foo.rs");
        fs::write(&file_path, "content").unwrap();

        let buf = OutputBuffer::new(10);
        let path_str = file_path.to_string_lossy().to_string();
        let id = buf.store_file(path_str.clone(), "content".to_string());
        let entry = buf.get(&id).unwrap();
        assert_eq!(entry.source_path, Some(file_path));
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

    #[test]
    fn get_file_handle_refreshes_when_file_modified() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // Write initial content and store it
        fs::write(&file_path, "original content").unwrap();

        let buf = OutputBuffer::new(10);
        let id = buf.store_file(
            file_path.to_string_lossy().to_string(),
            "original content".to_string(),
        );

        // Step 1 (baseline): verify cached content is returned
        assert_eq!(buf.get(&id).unwrap().stdout, "original content");

        // Step 2: write new content to disk BUT set mtime to the past (before entry.timestamp)
        // This simulates a file change that shouldn't trigger a refresh yet.
        fs::write(&file_path, "updated content").unwrap();
        let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        filetime::set_file_mtime(&file_path, filetime::FileTime::from_system_time(past)).unwrap();

        // Step 3 (stale-assert): mtime is in the past — must still return cached content
        assert_eq!(
            buf.get(&id).unwrap().stdout,
            "original content",
            "expected cached content when mtime has not advanced"
        );

        // Step 4: now advance mtime past entry.timestamp (trigger condition)
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        filetime::set_file_mtime(&file_path, filetime::FileTime::from_system_time(future)).unwrap();

        // Step 5 (fresh-assert): mtime is newer — must return updated content
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
        let id = buf.store_file(file_path.to_string_lossy().to_string(), "hello".to_string());

        assert!(buf.get(&id).is_some());

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
        let entry = buf.get(&id).unwrap();
        assert_eq!(entry.stdout, "hi");
    }

    #[test]
    fn get_with_refresh_flag_returns_true_when_file_changed() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "original").unwrap();

        let buf = OutputBuffer::new(10);
        let id = buf.store_file(path.to_string_lossy().to_string(), "original".to_string());

        // Overwrite file and bump mtime so it's definitely newer
        fs::write(&path, "modified").unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(future)).unwrap();

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
        assert!(
            !was_refreshed,
            "should not report refresh when file unchanged"
        );
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

    #[test]
    fn resolve_refs_reports_refreshed_file_handle() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.txt");
        fs::write(&path, "original").unwrap();

        let buf = OutputBuffer::new(10);
        let id = buf.store_file(path.to_string_lossy().to_string(), "original".to_string());

        // Modify the file so it looks newer
        fs::write(&path, "updated").unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(future)).unwrap();

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

    #[test]
    fn resolve_refs_bg_rejects_err_suffix() {
        let buf = OutputBuffer::new(10);
        let id = buf.store_background(std::path::PathBuf::from("/tmp/fake.log"));
        let err_ref = format!("{}.err", id);
        let err = buf.resolve_refs(&err_ref).unwrap_err();
        assert!(
            err.to_string().contains("do not support the .err suffix"),
            "unexpected error: {err}"
        );
    }
}
