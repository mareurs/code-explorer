# run_command Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Route all Bash commands through `run_command` with smart output summaries, buffer references (`@output_id`) queryable via Unix tools, and a two-round-trip speed bump for dangerous commands.

**Architecture:** `OutputBuffer` (session-scoped `HashMap`) stores command output. `run_command` detects `@ref` tokens, substitutes with temp files, executes. Command-type-specific summarizers extract key info (test pass/fail, build errors). Dangerous command patterns trigger `RecoverableError` unless `acknowledge_risk: true`.

**Tech Stack:** Rust, tokio, serde_json, regex, tempfile

**Design doc:** `docs/plans/2026-03-01-run-command-redesign-design.md`

---

### Task 1: OutputBuffer — store, get, evict

The core data structure. Session-scoped, in-memory, LRU eviction.

**Files:**
- Create: `src/tools/output_buffer.rs`
- Modify: `src/tools/mod.rs:17` (add `pub mod output_buffer;`)
- Modify: `src/tools/mod.rs:32-35` (add `output_buffer` field to `ToolContext`)

**Step 1: Write the failing tests**

In `src/tools/output_buffer.rs`, add a `#[cfg(test)] mod tests` with:

```rust
use super::*;

#[test]
fn store_and_get() {
    let buf = OutputBuffer::new(20);
    let id = buf.store("echo hello", "hello\n", "", 0);
    assert!(id.starts_with("@cmd_"));
    let entry = buf.get(&id).unwrap();
    assert_eq!(entry.stdout, "hello\n");
    assert_eq!(entry.stderr, "");
    assert_eq!(entry.exit_code, 0);
    assert_eq!(entry.command, "echo hello");
}

#[test]
fn get_missing_returns_none() {
    let buf = OutputBuffer::new(20);
    assert!(buf.get("@cmd_nonexistent").is_none());
}

#[test]
fn lru_eviction() {
    let buf = OutputBuffer::new(3);
    let id1 = buf.store("cmd1", "out1", "", 0);
    let _id2 = buf.store("cmd2", "out2", "", 0);
    let _id3 = buf.store("cmd3", "out3", "", 0);
    // id1 is oldest — adding a 4th should evict it
    let _id4 = buf.store("cmd4", "out4", "", 0);
    assert!(buf.get(&id1).is_none(), "oldest entry should be evicted");
    assert!(buf.get(&_id4).is_some(), "newest entry should exist");
}

#[test]
fn get_refreshes_lru_order() {
    let buf = OutputBuffer::new(3);
    let id1 = buf.store("cmd1", "out1", "", 0);
    let _id2 = buf.store("cmd2", "out2", "", 0);
    let _id3 = buf.store("cmd3", "out3", "", 0);
    // Access id1 to refresh it
    buf.get(&id1);
    // Now id2 is oldest — adding id4 should evict id2
    let _id4 = buf.store("cmd4", "out4", "", 0);
    assert!(buf.get(&id1).is_some(), "accessed entry should survive");
    assert!(buf.get(&_id2).is_none(), "unaccessed oldest should be evicted");
}

#[test]
fn stderr_suffix() {
    let buf = OutputBuffer::new(20);
    let id = buf.store("cmd", "out", "err_output", 1);
    let entry = buf.get(&id).unwrap();
    assert_eq!(entry.stderr, "err_output");
    // .err suffix should also work
    let err_id = format!("{}.err", id);
    let err_entry = buf.get(&err_id);
    // get() with .err suffix returns same entry (stderr accessible via content resolution)
    assert!(err_entry.is_some());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test output_buffer -- --nocapture 2>&1 | tail -20`
Expected: FAIL — module doesn't exist yet

**Step 3: Write minimal implementation**

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Entry stored in the output buffer.
#[derive(Clone, Debug)]
pub struct BufferEntry {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timestamp: u64,
}

/// Session-scoped buffer for command output.
/// Thread-safe via interior Mutex. LRU eviction by timestamp.
pub struct OutputBuffer {
    inner: Mutex<BufferInner>,
}

struct BufferInner {
    entries: HashMap<String, BufferEntry>,
    /// Insertion/access order for LRU — most recent at end
    order: Vec<String>,
    max_entries: usize,
    counter: u64,
}

impl OutputBuffer {
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(BufferInner {
                entries: HashMap::new(),
                order: Vec::new(),
                max_entries,
                counter: 0,
            }),
        }
    }

    /// Store command output, return `@cmd_<hex>` handle.
    pub fn store(
        &self,
        command: &str,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
    ) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.counter += 1;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let id = format!("@cmd_{:08x}", ts.wrapping_add(inner.counter));

        // Evict oldest if at capacity
        while inner.order.len() >= inner.max_entries {
            if let Some(oldest) = inner.order.first().cloned() {
                inner.entries.remove(&oldest);
                inner.order.remove(0);
            }
        }

        let entry = BufferEntry {
            command: command.to_string(),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_code,
            timestamp: ts,
        };
        inner.entries.insert(id.clone(), entry);
        inner.order.push(id.clone());
        id
    }

    /// Get a buffer entry. Refreshes LRU position.
    /// Supports `@cmd_xxx.err` suffix (returns same entry).
    pub fn get(&self, id: &str) -> Option<BufferEntry> {
        let base_id = id.strip_suffix(".err").unwrap_or(id);
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.entries.get(base_id).cloned() {
            // Refresh LRU order
            inner.order.retain(|x| x != base_id);
            inner.order.push(base_id.to_string());
            Some(entry)
        } else {
            None
        }
    }
}
```

**Step 4: Add module declaration and ToolContext field**

In `src/tools/mod.rs`:
- After line 12 (`pub mod output;`), add: `pub mod output_buffer;`
- Add `output_buffer: Arc<OutputBuffer>` to `ToolContext` struct
- Update `ToolContext` construction in `src/server.rs` `from_parts`

```rust
// In src/tools/mod.rs, ToolContext becomes:
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,
    pub output_buffer: Arc<output_buffer::OutputBuffer>,
}
```

In `src/server.rs`, add field to `CodeExplorerServer`:
```rust
output_buffer: Arc<crate::tools::output_buffer::OutputBuffer>,
```
Initialize in `from_parts`:
```rust
let output_buffer = Arc::new(crate::tools::output_buffer::OutputBuffer::new(20));
```
Pass to `ToolContext` in `call_tool`.

Fix all test helper `project_ctx()` functions across test files to include the new field:
```rust
ToolContext {
    agent,
    lsp: lsp(),
    output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
}
```

Search for all `ToolContext {` in the codebase to find every construction site:
```bash
cargo test 2>&1 | grep "missing.*output_buffer"
```

**Step 5: Run all tests to verify they pass**

Run: `cargo test 2>&1 | tail -5`
Expected: all pass

**Step 6: Commit**

```bash
git add src/tools/output_buffer.rs src/tools/mod.rs src/server.rs
# Plus all files with ToolContext fixes
git commit -m "feat(output_buffer): session-scoped LRU buffer for command output"
```

---

### Task 2: Dangerous command detection + speed bump

Add dangerous pattern matching and `acknowledge_risk` escape hatch, independent of output buffer.

**Files:**
- Modify: `src/util/path_security.rs` (add dangerous pattern functions + config fields)
- Modify: `src/config/project.rs` (add `shell_allow_always`, `shell_dangerous_patterns` to `SecuritySection`)
- Test: inline `#[cfg(test)]` in both files

**Step 1: Write the failing tests**

In `src/util/path_security.rs` test module, add:

```rust
#[test]
fn dangerous_command_detected() {
    let config = PathSecurityConfig::default();
    assert!(is_dangerous_command("rm -rf /tmp/foo", &config).is_some());
    assert!(is_dangerous_command("git push --force origin main", &config).is_some());
    assert!(is_dangerous_command("git reset --hard", &config).is_some());
    assert!(is_dangerous_command("git branch -D feature", &config).is_some());
    assert!(is_dangerous_command("git clean -fd", &config).is_some());
    assert!(is_dangerous_command("chmod 777 script.sh", &config).is_some());
    assert!(is_dangerous_command("kill -9 1234", &config).is_some());
}

#[test]
fn safe_command_not_flagged() {
    let config = PathSecurityConfig::default();
    assert!(is_dangerous_command("cargo test", &config).is_none());
    assert!(is_dangerous_command("git status", &config).is_none());
    assert!(is_dangerous_command("git push origin main", &config).is_none());
    assert!(is_dangerous_command("rm temp.txt", &config).is_none());
    assert!(is_dangerous_command("npm run build", &config).is_none());
}

#[test]
fn allow_always_bypasses_detection() {
    let mut config = PathSecurityConfig::default();
    config.shell_allow_always = vec!["git push --force".to_string()];
    assert!(is_dangerous_command("git push --force origin main", &config).is_none());
    // Other dangerous commands still detected
    assert!(is_dangerous_command("rm -rf /tmp", &config).is_some());
}

#[test]
fn custom_dangerous_patterns() {
    let mut config = PathSecurityConfig::default();
    config.shell_dangerous_patterns = vec!["kubectl delete".to_string()];
    assert!(is_dangerous_command("kubectl delete pod nginx", &config).is_some());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test dangerous_command -- --nocapture 2>&1 | tail -10`
Expected: FAIL — function doesn't exist

**Step 3: Write minimal implementation**

Add to `PathSecurityConfig`:
```rust
pub shell_allow_always: Vec<String>,
pub shell_dangerous_patterns: Vec<String>,
```

Add defaults in `impl Default for PathSecurityConfig`.

Add to `SecuritySection` in `src/config/project.rs`:
```rust
#[serde(default)]
pub shell_allow_always: Vec<String>,
#[serde(default)]
pub shell_dangerous_patterns: Vec<String>,
```

Wire them through `to_path_security_config()`.

Add function in `src/util/path_security.rs`:

```rust
use regex::Regex;

/// Default dangerous command patterns.
fn default_dangerous_patterns() -> Vec<&'static str> {
    vec![
        r"rm\s+(-[a-zA-Z]*f|-[a-zA-Z]*r|--force|--recursive)",
        r"git\s+push\s+.*--force",
        r"git\s+reset\s+--hard",
        r"git\s+branch\s+-D\b",
        r"git\s+checkout\s+--\s+\.",
        r"git\s+clean\s+-[a-zA-Z]*f",
        r"(?i)DROP\s+(TABLE|DATABASE)",
        r"chmod\s+777",
        r"kill\s+-9",
        r"\bmkfs\b",
        r"\bdd\s+if=",
    ]
}

/// Check if command matches a dangerous pattern.
/// Returns the matched pattern string if dangerous, None if safe.
/// Respects `shell_allow_always` overrides.
pub fn is_dangerous_command(command: &str, config: &PathSecurityConfig) -> Option<String> {
    // Check allow-always overrides first
    for allowed in &config.shell_allow_always {
        if command.contains(allowed.as_str()) {
            return None;
        }
    }

    // Check default patterns
    for pattern in default_dangerous_patterns() {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_search(command) {
                return Some(pattern.to_string());
            }
        }
    }

    // Check user-configured patterns
    for pattern in &config.shell_dangerous_patterns {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_search(command) {
                return Some(pattern.clone());
            }
        }
    }

    None
}
```

Note: `regex` is already in `Cargo.toml` (used by search_pattern). Use `re.is_match()` if `is_search` isn't available — check the actual regex API version in the project.

**Step 4: Run all tests**

Run: `cargo test 2>&1 | tail -5`
Expected: all pass (including new dangerous_command tests)

**Step 5: Commit**

```bash
git add src/util/path_security.rs src/config/project.rs
git commit -m "feat(security): dangerous command detection with allow-always override"
```

---

### Task 3: Buffer reference resolution — `@cmd_` substitution

Parse `@cmd_*` tokens in commands, write buffer contents to temp files, substitute, execute.

**Files:**
- Modify: `src/tools/output_buffer.rs` (add `resolve_refs` method)
- Test: inline in `output_buffer.rs`

**Step 1: Write the failing tests**

```rust
#[test]
fn resolve_refs_no_refs() {
    let buf = OutputBuffer::new(20);
    let (cmd, files, is_buffer_only) = buf.resolve_refs("cargo test").unwrap();
    assert_eq!(cmd, "cargo test");
    assert!(files.is_empty());
    assert!(!is_buffer_only);
}

#[test]
fn resolve_refs_single_ref() {
    let buf = OutputBuffer::new(20);
    let id = buf.store("prev", "hello world\n", "", 0);
    let (cmd, files, is_buffer_only) = buf.resolve_refs(&format!("grep hello {}", id)).unwrap();
    // Command should have temp file path substituted
    assert!(!cmd.contains(&id));
    assert!(cmd.contains("/"));  // temp file path
    assert_eq!(files.len(), 1);
    assert!(is_buffer_only);
    // Temp file should contain stdout
    let content = std::fs::read_to_string(&files[0]).unwrap();
    assert_eq!(content, "hello world\n");
}

#[test]
fn resolve_refs_stderr_suffix() {
    let buf = OutputBuffer::new(20);
    let id = buf.store("prev", "stdout", "stderr_content", 1);
    let err_ref = format!("{}.err", id);
    let (cmd, files, _) = buf.resolve_refs(&format!("grep error {}", err_ref)).unwrap();
    assert!(!cmd.contains(&err_ref));
    let content = std::fs::read_to_string(&files[0]).unwrap();
    assert_eq!(content, "stderr_content");
}

#[test]
fn resolve_refs_multiple_refs() {
    let buf = OutputBuffer::new(20);
    let id1 = buf.store("cmd1", "out1", "", 0);
    let id2 = buf.store("cmd2", "out2", "", 0);
    let (cmd, files, is_buffer_only) =
        buf.resolve_refs(&format!("diff {} {}", id1, id2)).unwrap();
    assert_eq!(files.len(), 2);
    assert!(is_buffer_only);
    assert!(!cmd.contains(&id1));
    assert!(!cmd.contains(&id2));
}

#[test]
fn resolve_refs_missing_ref_errors() {
    let buf = OutputBuffer::new(20);
    let result = buf.resolve_refs("grep hello @cmd_nonexistent");
    assert!(result.is_err());
}

#[test]
fn resolve_refs_mixed_ref_and_file_not_buffer_only() {
    let buf = OutputBuffer::new(20);
    let id = buf.store("prev", "hello", "", 0);
    let (_, _, is_buffer_only) =
        buf.resolve_refs(&format!("grep hello {} /etc/passwd", id)).unwrap();
    assert!(!is_buffer_only, "mixed ref+file should not be buffer_only");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test resolve_refs -- --nocapture 2>&1 | tail -10`

**Step 3: Write implementation**

Add to `OutputBuffer`:

```rust
use std::io::Write;
use regex::Regex;

/// Result of resolving buffer references in a command.
/// Returns (modified_command, temp_file_paths, is_buffer_only).
///
/// `is_buffer_only` is true when ALL file-like arguments in the command
/// are buffer references (meaning the command is inherently safe — it only
/// operates on ephemeral read-only data).
pub fn resolve_refs(&self, command: &str) -> anyhow::Result<(String, Vec<std::path::PathBuf>, bool)> {
    let re = Regex::new(r"@cmd_[0-9a-f]{8,}(\.err)?").unwrap();
    let refs: Vec<&str> = re.find_iter(command).map(|m| m.as_str()).collect();

    if refs.is_empty() {
        return Ok((command.to_string(), vec![], false));
    }

    let mut modified = command.to_string();
    let mut temp_files = Vec::new();

    for ref_id in &refs {
        let is_stderr = ref_id.ends_with(".err");
        let base_id = ref_id.strip_suffix(".err").unwrap_or(ref_id);

        let entry = self.get(base_id).ok_or_else(|| {
            anyhow::anyhow!("buffer {} not found (expired or invalid)", ref_id)
        })?;

        let content = if is_stderr { &entry.stderr } else { &entry.stdout };

        // Write to temp file (read-only)
        let mut tmp = tempfile::NamedTempFile::new()?;
        tmp.write_all(content.as_bytes())?;
        tmp.flush()?;

        // Make read-only
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o444);
            std::fs::set_permissions(tmp.path(), perms)?;
        }

        let path = tmp.into_temp_path();
        let path_str = path.to_string_lossy().to_string();
        modified = modified.replace(ref_id, &path_str);
        temp_files.push(path.to_path_buf());

        // Keep the TempPath alive by leaking into a PathBuf
        // (caller is responsible for cleanup via the returned paths)
        std::mem::forget(path);
    }

    // Heuristic: is_buffer_only if we replaced refs and there are
    // no other absolute/relative file paths in the remaining args.
    // Simple check: after substitution, no args start with / or ./ that
    // aren't our temp files.
    let is_buffer_only = !modified
        .split_whitespace()
        .any(|arg| {
            (arg.starts_with('/') || arg.starts_with("./"))
                && !temp_files.iter().any(|p| arg.contains(&p.to_string_lossy().to_string()))
        });

    Ok((modified, temp_files, is_buffer_only))
}

/// Clean up temp files created by resolve_refs.
pub fn cleanup_temp_files(paths: &[std::path::PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}
```

**Step 4: Run tests**

Run: `cargo test resolve_refs -- --nocapture 2>&1 | tail -10`
Expected: all pass

**Step 5: Commit**

```bash
git add src/tools/output_buffer.rs
git commit -m "feat(output_buffer): resolve @cmd_ refs to temp files for Unix tool queries"
```

---

### Task 4: Command type detection + summarizers

Detect test/build/git commands and produce smart summaries.

**Files:**
- Create: `src/tools/command_summary.rs`
- Modify: `src/tools/mod.rs` (add `pub mod command_summary;`)
- Test: inline in `command_summary.rs`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_test_command() {
        assert_eq!(detect_command_type("cargo test"), CommandType::Test);
        assert_eq!(detect_command_type("cargo test --release"), CommandType::Test);
        assert_eq!(detect_command_type("pytest tests/"), CommandType::Test);
        assert_eq!(detect_command_type("npm test"), CommandType::Test);
        assert_eq!(detect_command_type("npx jest"), CommandType::Test);
        assert_eq!(detect_command_type("go test ./..."), CommandType::Test);
    }

    #[test]
    fn detect_build_command() {
        assert_eq!(detect_command_type("cargo build"), CommandType::Build);
        assert_eq!(detect_command_type("cargo clippy -- -D warnings"), CommandType::Build);
        assert_eq!(detect_command_type("npm run build"), CommandType::Build);
        assert_eq!(detect_command_type("make"), CommandType::Build);
        assert_eq!(detect_command_type("tsc"), CommandType::Build);
        assert_eq!(detect_command_type("gcc main.c"), CommandType::Build);
    }

    #[test]
    fn detect_generic_command() {
        assert_eq!(detect_command_type("echo hello"), CommandType::Generic);
        assert_eq!(detect_command_type("ls -la"), CommandType::Generic);
        assert_eq!(detect_command_type("cat file.txt"), CommandType::Generic);
    }

    #[test]
    fn summarize_cargo_test_all_pass() {
        let stdout = "\
running 5 tests
test tools::file::tests::a ... ok
test tools::file::tests::b ... ok
test tools::file::tests::c ... ok
test tools::file::tests::d ... ok
test tools::file::tests::e ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
";
        let summary = summarize_test_output(stdout, "", 0);
        assert_eq!(summary["passed"], 5);
        assert_eq!(summary["failed"], 0);
        assert!(summary.get("failures").is_none());
    }

    #[test]
    fn summarize_cargo_test_with_failures() {
        let stdout = "\
running 3 tests
test ok_test ... ok
test failing_test ... FAILED
test another ... ok

failures:

---- failing_test stdout ----
thread 'failing_test' panicked at 'assertion failed'

failures:
    failing_test

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
";
        let summary = summarize_test_output(stdout, "", 1);
        assert_eq!(summary["passed"], 2);
        assert_eq!(summary["failed"], 1);
        assert!(summary["failures"].as_str().unwrap().contains("failing_test"));
    }

    #[test]
    fn summarize_build_errors() {
        let stderr = "\
error[E0308]: mismatched types
 --> src/main.rs:5:20
  |
5 |     let x: String = 42;
  |                     ^^ expected `String`, found integer

warning: unused variable: `y`
 --> src/main.rs:3:9
  |
3 |     let y = 1;
  |         ^ help: consider prefixing with an underscore: `_y`

error: aborting due to 1 previous error; 1 warning emitted
";
        let summary = summarize_build_output("", stderr, 1);
        assert_eq!(summary["errors"], 1);
        assert_eq!(summary["warnings"], 1);
        assert!(summary["first_error"].as_str().unwrap().contains("E0308"));
    }

    #[test]
    fn summarize_generic_head_tail() {
        let lines: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        let summary = summarize_generic(&lines, "", 0);
        let output = summary["stdout"].as_str().unwrap();
        assert!(output.contains("line 1"));
        assert!(output.contains("line 20"));
        assert!(output.contains("lines omitted"));
        assert!(output.contains("line 100"));
    }

    #[test]
    fn short_output_not_summarized() {
        let stdout = "hello\nworld\n";
        assert!(!needs_summary(stdout, ""));
    }

    #[test]
    fn long_output_needs_summary() {
        let stdout: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        assert!(needs_summary(&stdout, ""));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test command_summary -- --nocapture 2>&1 | tail -10`

**Step 3: Write implementation**

```rust
use serde_json::{json, Value};
use regex::Regex;

const SUMMARY_LINE_THRESHOLD: usize = 50;
const HEAD_LINES: usize = 20;
const TAIL_LINES: usize = 10;

#[derive(Debug, PartialEq, Eq)]
pub enum CommandType {
    Test,
    Build,
    Generic,
}

pub fn detect_command_type(command: &str) -> CommandType {
    let cmd = command.trim();
    // Test commands
    if Regex::new(r"\b(cargo\s+test|pytest|npm\s+test|npx\s+jest|jest|go\s+test|mvn\s+test|gradle\s+test)\b")
        .unwrap()
        .is_match(cmd)
    {
        return CommandType::Test;
    }
    // Build commands
    if Regex::new(r"\b(cargo\s+(build|clippy|check)|npm\s+run\s+build|make\b|tsc\b|gcc\b|g\+\+\b|clang\b|javac\b|go\s+build)\b")
        .unwrap()
        .is_match(cmd)
    {
        return CommandType::Build;
    }
    CommandType::Generic
}

pub fn needs_summary(stdout: &str, stderr: &str) -> bool {
    stdout.lines().count() + stderr.lines().count() > SUMMARY_LINE_THRESHOLD
}

pub fn summarize_test_output(stdout: &str, _stderr: &str, exit_code: i32) -> Value {
    // Parse "test result: (ok|FAILED). N passed; M failed; K ignored"
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut ignored = 0u64;

    let result_re = Regex::new(r"(\d+) passed; (\d+) failed; (\d+) ignored").unwrap();
    for line in stdout.lines() {
        if let Some(caps) = result_re.captures(line) {
            passed += caps[1].parse::<u64>().unwrap_or(0);
            failed += caps[2].parse::<u64>().unwrap_or(0);
            ignored += caps[3].parse::<u64>().unwrap_or(0);
        }
    }

    let mut result = json!({
        "type": "test",
        "exit_code": exit_code,
        "passed": passed,
        "failed": failed,
        "ignored": ignored,
    });

    // Extract failure output (between "failures:" sections)
    if failed > 0 {
        let failures_re = Regex::new(r"(?s)failures:\s*\n(.*?)(?:failures:\s*\n|test result:)")
            .unwrap();
        if let Some(caps) = failures_re.captures(stdout) {
            result["failures"] = json!(caps[1].trim());
        }
    }

    result
}

pub fn summarize_build_output(_stdout: &str, stderr: &str, exit_code: i32) -> Value {
    let error_re = Regex::new(r"^error(\[E\d+\])?:").unwrap();
    let warning_re = Regex::new(r"^warning(\[.+\])?:").unwrap();

    let errors = stderr.lines().filter(|l| error_re.is_match(l.trim())).count();
    let warnings = stderr.lines().filter(|l| warning_re.is_match(l.trim())).count();

    // Extract first error block (from "error" line to next blank line or next error/warning)
    let mut first_error = String::new();
    let mut in_error = false;
    for line in stderr.lines() {
        if error_re.is_match(line.trim()) && first_error.is_empty() {
            in_error = true;
        }
        if in_error {
            if line.is_empty() && !first_error.is_empty() {
                break;
            }
            first_error.push_str(line);
            first_error.push('\n');
        }
    }

    let mut result = json!({
        "type": "build",
        "exit_code": exit_code,
        "errors": errors,
        "warnings": warnings,
    });

    if !first_error.is_empty() {
        result["first_error"] = json!(first_error.trim());
    }

    result
}

pub fn summarize_generic(stdout: &str, stderr: &str, exit_code: i32) -> Value {
    let lines: Vec<&str> = stdout.lines().collect();
    let total = lines.len();

    let summary = if total > HEAD_LINES + TAIL_LINES {
        let head = lines[..HEAD_LINES].join("\n");
        let tail = lines[total - TAIL_LINES..].join("\n");
        format!(
            "{}\n\n--- {} lines omitted ---\n\n{}",
            head,
            total - HEAD_LINES - TAIL_LINES,
            tail
        )
    } else {
        stdout.to_string()
    };

    let mut result = json!({
        "type": "generic",
        "exit_code": exit_code,
        "stdout": summary,
        "total_stdout_lines": total,
    });

    if !stderr.is_empty() {
        result["stderr"] = json!(stderr);
    }

    result
}
```

**Step 4: Run tests**

Run: `cargo test command_summary -- --nocapture 2>&1 | tail -10`
Expected: all pass

**Step 5: Commit**

```bash
git add src/tools/command_summary.rs src/tools/mod.rs
git commit -m "feat(command_summary): detect test/build commands and produce smart summaries"
```

---

### Task 5: Rewrite RunCommand::call — wire everything together

Replace the current `call` method to integrate buffer, summarizers, speed bump, cwd, and @ref resolution.

**Files:**
- Modify: `src/tools/workflow.rs` (rewrite `impl Tool for RunCommand`)
- Test: update existing tests + add new ones in `workflow.rs`

**Step 1: Write new tests**

Add to the existing test module in `src/tools/workflow.rs`:

```rust
#[tokio::test]
async fn run_command_short_output_returns_directly() {
    let (dir, ctx) = project_ctx().await;
    let result = RunCommand
        .call(json!({"command": "echo hello"}), &ctx)
        .await
        .unwrap();
    assert_eq!(result["stdout"].as_str().unwrap().trim(), "hello");
    assert!(result.get("output_id").is_none(), "short output should not buffer");
}

#[tokio::test]
async fn run_command_long_output_returns_summary_and_buffer() {
    let (dir, ctx) = project_ctx().await;
    let result = RunCommand
        .call(
            json!({"command": "seq 1 200"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(result.get("output_id").is_some(), "long output should be buffered");
    let output_id = result["output_id"].as_str().unwrap();
    assert!(output_id.starts_with("@cmd_"));
    assert!(result["total_stdout_lines"].as_u64().unwrap() >= 200);
}

#[tokio::test]
async fn run_command_buffer_ref_query() {
    let (dir, ctx) = project_ctx().await;
    // First: generate buffered output
    let result = RunCommand
        .call(json!({"command": "seq 1 200"}), &ctx)
        .await
        .unwrap();
    let output_id = result["output_id"].as_str().unwrap();

    // Second: query with grep
    let grep_result = RunCommand
        .call(
            json!({"command": format!("grep '^10$' {}", output_id)}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(grep_result["stdout"].as_str().unwrap().contains("10"));
}

#[tokio::test]
async fn run_command_dangerous_requires_acknowledge() {
    let (_dir, ctx) = project_ctx().await;
    // First call: should be blocked
    let result = RunCommand
        .call(json!({"command": "rm -rf /tmp/test_dir_xxx"}), &ctx)
        .await;
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Destructive command"));

    // Second call with acknowledge_risk: should execute
    let result = RunCommand
        .call(
            json!({"command": "echo would_delete", "acknowledge_risk": true}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(result["stdout"].as_str().unwrap().contains("would_delete"));
}

#[tokio::test]
async fn run_command_buffer_only_skips_speed_bump() {
    let (_dir, ctx) = project_ctx().await;
    // Store some output
    let result = RunCommand
        .call(json!({"command": "seq 1 200"}), &ctx)
        .await
        .unwrap();
    let output_id = result["output_id"].as_str().unwrap();

    // rm with @ref should NOT be blocked (buffer-only = safe)
    let result = RunCommand
        .call(
            json!({"command": format!("grep -c '1' {}", output_id)}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(result["exit_code"].as_i64().unwrap() == 0);
}

#[tokio::test]
async fn run_command_cwd_relative_to_root() {
    let (dir, ctx) = project_ctx().await;
    std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
    std::fs::write(dir.path().join("subdir/test.txt"), "found").unwrap();
    let result = RunCommand
        .call(
            json!({"command": "cat test.txt", "cwd": "subdir"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(result["stdout"].as_str().unwrap().contains("found"));
}

#[tokio::test]
async fn run_command_cwd_rejects_path_traversal() {
    let (_dir, ctx) = project_ctx().await;
    let result = RunCommand
        .call(
            json!({"command": "ls", "cwd": "../../etc"}),
            &ctx,
        )
        .await;
    assert!(result.is_err());
}
```

**Step 2: Run tests to verify new ones fail**

Run: `cargo test run_command -- --nocapture 2>&1 | tail -20`

**Step 3: Rewrite `RunCommand` implementation**

Replace `input_schema` to add `cwd`, `acknowledge_risk`.

Rewrite `call`:
1. Parse params (`command`, `timeout_secs`, `cwd`, `acknowledge_risk`)
2. Resolve `@ref` tokens via `ctx.output_buffer.resolve_refs()`
3. If NOT buffer-only: check dangerous patterns (unless `acknowledge_risk`)
4. Check shell mode (existing logic)
5. Resolve `cwd` (validate it's within project root)
6. Execute command
7. If output exceeds threshold: store in buffer, return smart summary
8. If short: return directly (existing behavior)
9. Clean up temp files from resolve_refs

See design doc for the full `call` method structure. Key integration points:
- `ctx.output_buffer.store()` / `ctx.output_buffer.resolve_refs()`
- `crate::util::path_security::is_dangerous_command()`
- `crate::tools::command_summary::{detect_command_type, needs_summary, summarize_*}`

**Step 4: Update existing tests**

Some existing tests may need small adjustments (e.g., the output format changed for short commands — verify `stdout` field still present).

**Step 5: Run all tests**

Run: `cargo test 2>&1 | tail -5`
Expected: all pass

**Step 6: Run clippy + fmt**

```bash
cargo fmt
cargo clippy -- -D warnings
```

**Step 7: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(run_command): smart output, buffer refs, speed bump, cwd support"
```

---

### Task 6: Update server instructions + schema docs

**Files:**
- Modify: `src/prompts/server_instructions.md`
- Modify: `src/tools/workflow.rs` (`description()` and `input_schema()` for RunCommand)

**Step 1: Update RunCommand description**

```rust
fn description(&self) -> &str {
    "Run a shell command in the active project root and return stdout/stderr. \
     Large output is stored in a buffer — query it with Unix tools via @output_id references. \
     Dangerous commands require acknowledge_risk: true."
}
```

**Step 2: Update server_instructions.md**

Replace the "Run shell commands" section with:

```markdown
**Run shell commands:**
- `run_command(command)` — execute a shell command. Large output is stored in a
  buffer and a smart summary is returned (test pass/fail, build errors, etc.).
  Query stored output using Unix tools with `@output_id` references:
  `grep FAILED @cmd_a1b2c3`, `tail -20 @cmd_a1b2c3`, `diff @cmd_x @cmd_y`.
  - `cwd` — run from a subdirectory (relative to project root)
  - `acknowledge_risk` — bypass safety check for destructive commands
  - `timeout_secs` — max execution time (default 30)
```

**Step 3: Run tests (instructions are compiled into binary)**

```bash
cargo test 2>&1 | tail -5
```

**Step 4: Commit**

```bash
git add src/tools/workflow.rs src/prompts/server_instructions.md
git commit -m "docs: update run_command schema and server instructions"
```

---

### Task 7: Update routing plugin — block all Bash

**Files:**
- Modify: `/home/marius/work/claude/claude-plugins/code-explorer-routing/hooks/pre-tool-guard.sh`

**Step 1: Update the Bash case**

In the `Bash)` case of `pre-tool-guard.sh`, add a new block BEFORE the existing source-file checks:

```bash
  Bash)
    CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

    # ── Block ALL Bash when code-explorer is available ────────────────
    # Agents should use run_command() instead — it provides smart output
    # summaries, buffer refs for querying, and dangerous command detection.
    deny "⛔ Use run_command(\"$(echo "$CMD" | head -c 80)\") instead of Bash.
run_command provides:
  - Smart output summaries (test pass/fail, build errors)
  - Output buffers queryable with grep/tail/awk/sed @output_id
  - Dangerous command detection with acknowledge_risk escape hatch
  - Runs in project root with optional cwd parameter"
    ;;
```

This replaces the existing source-file-only blocking with a full block.

**Step 2: Manual test**

Start a Claude Code session with the plugin active. Try using Bash — verify it's blocked and the message appears. Verify `run_command` works as expected.

**Step 3: Commit (in the plugin repo)**

```bash
cd /home/marius/work/claude/claude-plugins/code-explorer-routing
git add hooks/pre-tool-guard.sh
git commit -m "feat: block all Bash calls, redirect to run_command"
```

---

### Task 8: Final verification + cleanup

**Files:**
- All modified files

**Step 1: Full test suite**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

**Step 2: Manual integration test**

1. Build and start the MCP server: `cargo run -- start --project .`
2. Run a long command via `run_command("cargo test")`
3. Verify smart summary with test counts
4. Query output with `run_command("grep FAILED @cmd_xxx")`
5. Try a dangerous command — verify speed bump
6. Try `acknowledge_risk: true` — verify it executes
7. Try `cwd: "src"` — verify it works
8. Try invalid `cwd: "../../etc"` — verify rejection

**Step 3: Update memory**

Update `code-insights` and `architecture` memories with new OutputBuffer and command_summary modules.

**Step 4: Final commit (if any cleanup needed)**

```bash
git add -A
git commit -m "chore: final cleanup for run_command redesign"
```

**Step 5: Push**

```bash
git push
```
