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
}

/// A dangerous command held pending agent acknowledgment.
#[derive(Debug, Clone)]
pub struct PendingAckCommand {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: u64,
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
    // --- pending-ack store ---
    pending_acks: HashMap<String, PendingAckCommand>,
    pending_order: Vec<String>,
    max_pending: usize,
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
                max_pending: 20,
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
        };

        inner.entries.insert(id.clone(), entry);
        inner.order.push(id.clone());
        id
    }

    /// Get an entry by handle, refreshing its LRU position.
    ///
    /// Supports a `.err` suffix on the handle (e.g. `@cmd_xxx.err`),
    /// which returns the same entry (caller decides what to extract).
    pub fn get(&self, id: &str) -> Option<BufferEntry> {
        let canonical = id.strip_suffix(".err").unwrap_or(id);
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(canonical) {
            // Refresh LRU order: move to end
            if let Some(pos) = inner.order.iter().position(|k| k == canonical) {
                inner.order.remove(pos);
                inner.order.push(canonical.to_string());
            }
            inner.entries.get(canonical).cloned()
        } else {
            None
        }
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
            command: path,
            stdout: content,
            stderr: String::new(),
            exit_code: 0,
            timestamp: now,
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
        };
        inner.entries.insert(id.clone(), entry);
        inner.order.push(id.clone());
        id
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
    pub fn resolve_refs(&self, command: &str) -> Result<(String, Vec<PathBuf>, bool)> {
        let re = Regex::new(r"@(?:cmd|file|tool)_[0-9a-f]{8}(\.err)?").expect("valid regex");

        let refs: Vec<&str> = re.find_iter(command).map(|m| m.as_str()).collect();
        if refs.is_empty() {
            return Ok((command.to_string(), vec![], false));
        }

        // Deduplicate while preserving order (same ref token may appear twice).
        let mut seen = std::collections::HashSet::new();
        let unique_refs: Vec<&str> = refs.iter().filter(|r| seen.insert(**r)).copied().collect();

        let mut result = command.to_string();
        let mut temp_paths: Vec<PathBuf> = Vec::new();
        let mut temp_path_strings: Vec<String> = Vec::new();

        for token in &unique_refs {
            let is_stderr = token.ends_with(".err");
            let base_id = if is_stderr {
                &token[..token.len() - 4] // strip ".err"
            } else {
                token
            };

            let entry = self
                .get(base_id)
                .ok_or_else(|| RecoverableError::with_hint(
                    format!("buffer reference not found: {}", token),
                    "Buffer refs expire when the session resets. Re-run the command to get a fresh ref.",
                ))?;

            let content = if is_stderr {
                &entry.stderr
            } else {
                &entry.stdout
            };

            // Write content to a temp file
            let mut tmp = NamedTempFile::new()?;
            tmp.write_all(content.as_bytes())?;
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

        // Determine is_buffer_only: check if there are file-path-like arguments
        // (starting with / or ./) that are NOT one of our temp file paths.
        let is_buffer_only = !shell_words(&result).iter().any(|word| {
            (word.starts_with('/') || word.starts_with("./"))
                && !temp_path_strings
                    .iter()
                    .any(|tp| word.contains(tp.as_str()))
        });

        Ok((result, temp_paths, is_buffer_only))
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
        // Reject if any whitespace-separated word looks like a bare path.
        for word in command.split_whitespace() {
            if word.starts_with('/') || word.starts_with("./") || word.starts_with("../") {
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
        let (cmd, files, is_buffer_only) = buf.resolve_refs("cargo test").unwrap();
        assert_eq!(cmd, "cargo test");
        assert!(files.is_empty());
        assert!(!is_buffer_only);
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_single_ref() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("prev".into(), "hello world\n".into(), "".into(), 0);
        let (cmd, files, is_buffer_only) = buf.resolve_refs(&format!("grep hello {}", id)).unwrap();
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
        let (cmd, files, _) = buf
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
        let (cmd, files, is_buffer_only) =
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
        let (_, files, is_buffer_only) = buf
            .resolve_refs(&format!("diff {} /etc/passwd", id))
            .unwrap();
        assert!(!is_buffer_only);
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_temp_files_are_readonly() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("prev".into(), "data".into(), "".into(), 0);
        let (_, files, _) = buf.resolve_refs(&format!("cat {}", id)).unwrap();
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
        let (resolved, files, _) = buf.resolve_refs(&format!("grep hello {}", id)).unwrap();
        assert!(!resolved.contains('@'), "got: {}", resolved);
        assert!(resolved.starts_with("grep hello /"));
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_substitutes_file_ref() {
        let buf = OutputBuffer::new(20);
        let id = buf.store_file("README.md".into(), "# Hello\n".into());
        let (resolved, files, _) = buf.resolve_refs(&format!("wc -l {}", id)).unwrap();
        assert!(!resolved.contains('@'), "got: {}", resolved);
        OutputBuffer::cleanup_temp_files(&files);
    }

    #[test]
    fn resolve_refs_err_suffix_writes_stderr() {
        let buf = OutputBuffer::new(20);
        let id = buf.store("cmd".into(), "out".into(), "err_text".into(), 0);
        let err_ref = format!("{}.err", id);
        let (resolved, files, _) = buf.resolve_refs(&format!("grep x {}", err_ref)).unwrap();
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
        assert!(
            result.unwrap_err().to_string().contains("@cmd_deadbeef"),
            "error should name the bad ref"
        );
    }

    #[test]
    fn is_buffer_only_true_for_unix_tools_on_refs() {
        assert!(OutputBuffer::is_buffer_only("grep FAILED @cmd_a1b2c3"));
        assert!(OutputBuffer::is_buffer_only("tail -50 @file_abc123"));
        assert!(OutputBuffer::is_buffer_only("diff @cmd_aaa @file_bbb"));
        assert!(OutputBuffer::is_buffer_only("wc -l @cmd_a1b2c3"));
        // @tool_ refs must also be recognised
        assert!(OutputBuffer::is_buffer_only("grep foo @tool_abc12345"));
        assert!(OutputBuffer::is_buffer_only("jq '.symbols' @tool_abc12345"));
    }

    #[test]
    fn is_buffer_only_false_for_plain_commands() {
        assert!(!OutputBuffer::is_buffer_only("cargo test"));
        assert!(!OutputBuffer::is_buffer_only("grep foo /etc/hosts"));
        assert!(!OutputBuffer::is_buffer_only("cat ./README.md"));
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
        let (resolved, _paths, _is_buf_only) = buf.resolve_refs(&cmd).unwrap();
        assert!(
            !resolved.contains("@tool_"),
            "ref should be substituted, got: {}",
            resolved
        );
    }
}
