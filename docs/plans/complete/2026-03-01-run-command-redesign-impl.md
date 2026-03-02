# run_command Redesign + read_file Buffer Extension — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rewrite `run_command` to use smart output summaries + `@cmd_*` buffer refs, extend `read_file` with the same buffer pattern for large files, and update server instructions + routing plugin.

**Architecture:** `OutputBuffer` (already exists) gets three new methods: `store_file()` for `@file_*` refs, `resolve_refs()` to substitute refs with temp file paths before shell execution, and `is_buffer_only()` to detect safe buffer-only commands. `RunCommand::call` gains `cwd`/`acknowledge_risk` params, dangerous command speed bump, and buffer-backed smart summaries. `ReadFile::call` drops its source-file rejection and instead buffers files > 200 lines. Both tools share the same `OutputBuffer` session state via `ToolContext`.

**Tech Stack:** Rust, tokio, `tempfile` crate (already in Cargo.toml), `regex` + `once_cell`, `serde_json`

**Design doc:** `docs/plans/2026-03-01-run-command-redesign-design.md`

**Current state — already implemented (do not reimplement):**
- `src/tools/output_buffer.rs` — `BufferEntry`, `OutputBuffer`, `store()`, `get()`, LRU eviction, 5 passing tests
- `src/tools/command_summary.rs` — `CommandType`, `detect_command_type()`, `needs_summary()`, all summarizers, 16 passing tests
- `src/util/path_security.rs` — `is_dangerous_command()`, `PathSecurityConfig.shell_allow_always`, `PathSecurityConfig.shell_dangerous_patterns` — all done
- `src/tools/mod.rs` — `ToolContext.output_buffer: Arc<OutputBuffer>` already wired

---

### Task 1: `OutputBuffer` — `store_file()`, `resolve_refs()`, `is_buffer_only()`

**Files:**
- Modify: `src/tools/output_buffer.rs`

These three methods complete the `OutputBuffer` API needed by both RunCommand and ReadFile.

**Step 1: Write the failing tests**

Add to the `tests` module in `src/tools/output_buffer.rs`:

```rust
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
    let (resolved, _guards) = buf.resolve_refs(&format!("grep hello {}", id)).unwrap();
    assert!(!resolved.contains('@'), "got: {}", resolved);
    assert!(resolved.starts_with("grep hello /"));
}

#[test]
fn resolve_refs_substitutes_file_ref() {
    let buf = OutputBuffer::new(20);
    let id = buf.store_file("README.md".into(), "# Hello\n".into());
    let (resolved, _guards) = buf.resolve_refs(&format!("wc -l {}", id)).unwrap();
    assert!(!resolved.contains('@'), "got: {}", resolved);
}

#[test]
fn resolve_refs_err_suffix_writes_stderr() {
    let buf = OutputBuffer::new(20);
    let id = buf.store("cmd".into(), "out".into(), "err_text".into(), 0);
    let err_ref = format!("{}.err", id);
    let (resolved, _guards) = buf.resolve_refs(&format!("grep x {}", err_ref)).unwrap();
    let tmp_path = resolved.split_whitespace().last().unwrap();
    let content = std::fs::read_to_string(tmp_path).unwrap();
    assert_eq!(content, "err_text");
}

#[test]
fn resolve_refs_unknown_ref_returns_error() {
    let buf = OutputBuffer::new(20);
    let result = buf.resolve_refs("grep foo @cmd_deadbeef");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("@cmd_deadbeef"));
}

#[test]
fn is_buffer_only_true_for_unix_tools_on_refs() {
    assert!(OutputBuffer::is_buffer_only("grep FAILED @cmd_a1b2c3"));
    assert!(OutputBuffer::is_buffer_only("tail -50 @file_abc123"));
    assert!(OutputBuffer::is_buffer_only("diff @cmd_aaa @file_bbb"));
    assert!(OutputBuffer::is_buffer_only("wc -l @cmd_a1b2c3"));
}

#[test]
fn is_buffer_only_false_for_plain_commands() {
    assert!(!OutputBuffer::is_buffer_only("cargo test"));
    assert!(!OutputBuffer::is_buffer_only("grep foo /etc/hosts"));
    assert!(!OutputBuffer::is_buffer_only("cat ./README.md"));
}
```

**Step 2: Run tests to confirm they fail**

```bash
cargo test -p code-explorer output_buffer 2>&1 | grep -E "FAILED|error\[" | head -5
```
Expected: compile errors (methods don't exist yet).

**Step 3: Implement the three methods**

Add required imports near the top of `src/tools/output_buffer.rs` (after existing `use` lines):

```rust
use once_cell::sync::Lazy;
use regex::Regex;

static REF_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"@(?:cmd|file)_[0-9a-f]+(?:\.err)?").unwrap()
});
```

Add these three methods to `impl OutputBuffer`:

```rust
/// Store file content under a `@file_*` ID.
/// Content goes in `stdout`; `stderr` is empty; `exit_code` is 0.
pub fn store_file(&self, path: String, content: String) -> String {
    let mut inner = self.inner.lock().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    inner.counter = inner.counter.wrapping_add(1);
    let id = format!("@file_{:08x}", now.wrapping_add(inner.counter));

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

/// Substitute `@cmd_*` and `@file_*` refs in `command` with read-only temp file paths.
/// Returns the rewritten command and temp file guards (keep alive until command finishes).
/// Returns `Err` if any referenced buffer ID is not found.
pub fn resolve_refs(
    &self,
    command: &str,
) -> anyhow::Result<(String, Vec<tempfile::NamedTempFile>)> {
    use std::io::Write as _;

    let mut guards: Vec<tempfile::NamedTempFile> = Vec::new();
    let mut path_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for cap in REF_RE.find_iter(command) {
        let token = cap.as_str().to_string();
        if path_map.contains_key(&token) {
            continue;
        }
        let is_err = token.ends_with(".err");
        let canonical = if is_err {
            token.strip_suffix(".err").unwrap().to_string()
        } else {
            token.clone()
        };
        let entry = self
            .get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("buffer ref {} not found", token))?;
        let content = if is_err { &entry.stderr } else { &entry.stdout };

        let mut tmp = tempfile::NamedTempFile::new()?;
        tmp.write_all(content.as_bytes())?;
        tmp.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o444))?;
        }
        path_map.insert(token, tmp.path().to_string_lossy().into_owned());
        guards.push(tmp);
    }

    // Replace longest tokens first to avoid partial-match clobbering
    let mut pairs: Vec<_> = path_map.into_iter().collect();
    pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    let mut resolved = command.to_string();
    for (token, path) in pairs {
        resolved = resolved.replace(&token, &path);
    }
    Ok((resolved, guards))
}

/// True when the command operates only on buffer refs (no bare filesystem paths).
/// Buffer-only commands skip shell_command_mode checks and the dangerous-command speed bump.
pub fn is_buffer_only(command: &str) -> bool {
    if !command.contains("@cmd_") && !command.contains("@file_") {
        return false;
    }
    // Reject if any whitespace-separated word looks like a bare path
    for word in command.split_whitespace() {
        if word.starts_with('/') || word.starts_with("./") || word.starts_with("../") {
            return false;
        }
    }
    true
}
```

**Step 4: Run tests**

```bash
cargo test -p code-explorer output_buffer 2>&1 | tail -5
```
Expected: all 12 output_buffer tests pass.

**Step 5: Commit**

```bash
cargo fmt && cargo clippy -- -D warnings
git add src/tools/output_buffer.rs
git commit -m "feat(output_buffer): add store_file, resolve_refs, is_buffer_only"
```

---

### Task 2: `RunCommand` — new input schema (cwd, acknowledge_risk)

**Files:**
- Modify: `src/tools/workflow.rs`

**Step 1: Write the failing test**

Add to the `tests` module in `src/tools/workflow.rs`:

```rust
#[test]
fn run_command_schema_has_cwd_and_acknowledge_risk() {
    let schema = RunCommand.input_schema();
    let props = &schema["properties"];
    assert!(props.get("cwd").is_some(), "missing cwd");
    assert!(props.get("acknowledge_risk").is_some(), "missing acknowledge_risk");
}
```

**Step 2: Run to confirm it fails**

```bash
cargo test -p code-explorer run_command_schema 2>&1 | grep "FAILED\|error\["
```

**Step 3: Update `input_schema`**

Replace `impl Tool for RunCommand / input_schema` body with:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["command"],
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to execute. May reference output buffers with @cmd_* or @file_* syntax."
            },
            "timeout_secs": {
                "type": "integer",
                "default": 30,
                "description": "Max execution time in seconds."
            },
            "cwd": {
                "type": "string",
                "description": "Subdirectory relative to project root to run in. Validated to stay within project."
            },
            "acknowledge_risk": {
                "type": "boolean",
                "description": "Set true to bypass the speed bump for a previously-flagged dangerous command."
            }
        }
    })
}
```

**Step 4: Run test**

```bash
cargo test -p code-explorer run_command_schema 2>&1 | tail -3
```
Expected: PASS.

**Step 5: Commit**

```bash
cargo fmt && cargo clippy -- -D warnings
git add src/tools/workflow.rs
git commit -m "feat(run_command): add cwd + acknowledge_risk to input schema"
```

---

### Task 3: `RunCommand` — dangerous command speed bump + cwd validation

**Files:**
- Modify: `src/tools/workflow.rs`

**Step 1: Write failing tests**

```rust
#[tokio::test]
async fn run_command_dangerous_rejected_without_ack() {
    let ctx = project_ctx().await;
    let result = RunCommand
        .call(json!({"command": "rm -rf /tmp/ce_nonexistent_test"}), &ctx)
        .await
        .unwrap(); // RecoverableError — returns Ok with error field
    assert!(result["error"].is_string(), "expected error field, got: {}", result);
    assert!(
        result["hint"].as_str().unwrap().contains("acknowledge_risk"),
        "hint should mention acknowledge_risk"
    );
}

#[tokio::test]
async fn run_command_safe_command_not_blocked() {
    let ctx = project_ctx().await;
    let result = RunCommand
        .call(json!({"command": "echo hello"}), &ctx)
        .await
        .unwrap();
    assert!(result.get("error").is_none(), "echo should not be blocked");
}

#[tokio::test]
async fn run_command_cwd_rejects_path_traversal() {
    let ctx = project_ctx().await;
    let result = RunCommand
        .call(json!({"command": "ls", "cwd": "../../etc"}), &ctx)
        .await
        .unwrap();
    assert!(result["error"].is_string(), "traversal should be rejected");
}

#[tokio::test]
async fn run_command_buffer_only_skips_speed_bump() {
    let ctx = project_ctx().await;
    let id = ctx.output_buffer.store("cmd".into(), "data\n".into(), "".into(), 0);
    // "rm" appears in the command but it's a buffer-only query — should NOT be flagged
    let result = RunCommand
        .call(json!({"command": format!("grep rm {}", id)}), &ctx)
        .await
        .unwrap();
    assert!(result.get("error").map(|e| !e.as_str().unwrap_or("").contains("Dangerous")).unwrap_or(true));
}
```

**Step 2: Run to confirm they fail**

```bash
cargo test -p code-explorer "run_command_dangerous|run_command_safe|run_command_cwd|run_command_buffer_only" 2>&1 | grep "FAILED\|error\[" | head -10
```

**Step 3: Add dangerous check + cwd logic at the top of `RunCommand::call`**

After extracting `command` and `timeout_secs` from `input`, add:

```rust
let cwd_param = input["cwd"].as_str();
let acknowledge_risk = input["acknowledge_risk"].as_bool().unwrap_or(false);
let buffer_only = crate::tools::output_buffer::OutputBuffer::is_buffer_only(command);

// Speed bump for dangerous commands (skip when buffer-only or explicitly acknowledged).
if !buffer_only && !acknowledge_risk {
    if let Some(description) =
        crate::util::path_security::is_dangerous_command(command, &security)
    {
        return Err(super::RecoverableError::with_hint(
            format!("Dangerous command detected: {}", description),
            "Re-run with acknowledge_risk: true to proceed.",
        )
        .into());
    }
}

// Resolve cwd: must be relative and confined to project root.
let working_dir = if let Some(cwd) = cwd_param {
    let candidate = root.join(cwd);
    // Use canonicalize only if the path exists; otherwise reject.
    let canonical = candidate.canonicalize().map_err(|_| {
        super::RecoverableError::with_hint(
            format!("cwd '{}' does not exist or is not accessible", cwd),
            "Provide a subdirectory path relative to the project root.",
        )
    })?;
    if !canonical.starts_with(&root) {
        return Err(super::RecoverableError::with_hint(
            format!("cwd '{}' escapes the project root", cwd),
            "cwd must resolve to a subdirectory inside the project root.",
        )
        .into());
    }
    canonical
} else {
    root.clone()
};
```

Replace every `current_dir(&root)` in the `#[cfg(unix)]` and `#[cfg(windows)]` blocks with `current_dir(&working_dir)`.

**Step 4: Run tests**

```bash
cargo test -p code-explorer "run_command_dangerous|run_command_safe|run_command_cwd|run_command_buffer_only" 2>&1 | tail -5
```
Expected: all pass.

**Step 5: Commit**

```bash
cargo fmt && cargo clippy -- -D warnings
git add src/tools/workflow.rs
git commit -m "feat(run_command): dangerous command speed bump + cwd validation"
```

---

### Task 4: `RunCommand` — buffer ref execution + smart output summaries

**Files:**
- Modify: `src/tools/workflow.rs`

**Step 1: Write failing tests**

```rust
#[tokio::test]
async fn run_command_short_output_returned_directly() {
    let ctx = project_ctx().await;
    let result = RunCommand
        .call(json!({"command": "echo hello"}), &ctx)
        .await
        .unwrap();
    assert!(result.get("output_id").is_none(), "short output should not buffer");
    assert!(result["stdout"].as_str().unwrap().contains("hello"));
}

#[tokio::test]
async fn run_command_large_output_stored_in_buffer() {
    let ctx = project_ctx().await;
    let result = RunCommand
        .call(json!({"command": "seq 1 100"}), &ctx)
        .await
        .unwrap();
    let output_id = result["output_id"].as_str().expect("large output should have output_id");
    assert!(output_id.starts_with("@cmd_"));
    assert!(result["total_stdout_lines"].as_u64().unwrap() >= 100);
    let entry = ctx.output_buffer.get(output_id).unwrap();
    assert!(entry.stdout.contains("50\n"));
}

#[tokio::test]
async fn run_command_buffer_ref_executes_correctly() {
    let ctx = project_ctx().await;
    let r1 = RunCommand
        .call(json!({"command": "seq 1 100"}), &ctx)
        .await
        .unwrap();
    let output_id = r1["output_id"].as_str().unwrap();
    let r2 = RunCommand
        .call(json!({"command": format!("grep '^50$' {}", output_id)}), &ctx)
        .await
        .unwrap();
    assert_eq!(r2["exit_code"], 0);
    assert_eq!(r2["stdout"].as_str().unwrap().trim(), "50");
}
```

**Step 2: Run to confirm they fail**

```bash
cargo test -p code-explorer "run_command_short|run_command_large|run_command_buffer_ref_exec" 2>&1 | grep "FAILED\|error\[" | head -5
```

**Step 3: Rewrite the execution + response block in `RunCommand::call`**

Replace the block from `let child = ...` through the end of the `match` (currently building `result` with `truncate_output`) with:

```rust
// Resolve any @cmd_* / @file_* refs in the command before execution.
let (resolved_command, _temp_guards) = ctx.output_buffer.resolve_refs(command)?;

#[cfg(unix)]
let child = tokio::process::Command::new("sh")
    .arg("-c")
    .arg(&resolved_command)
    .current_dir(&working_dir)
    .output();

#[cfg(windows)]
let child = tokio::process::Command::new("cmd")
    .arg("/C")
    .arg(&resolved_command)
    .current_dir(&working_dir)
    .output();

match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child).await {
    Ok(Ok(output)) => {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code();

        // Short output: return directly without buffering.
        if !command_summary::needs_summary(&stdout, &stderr) {
            return Ok(json!({
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
            }));
        }

        // Large output: store in buffer, return smart summary.
        let output_id = ctx.output_buffer.store(
            command.to_string(),
            stdout.clone(),
            stderr.clone(),
            exit_code.unwrap_or(-1),
        );
        let total_stdout_lines = command_summary::count_lines(&stdout);
        let total_stderr_lines = command_summary::count_lines(&stderr);

        let mut result = match command_summary::detect_command_type(command) {
            command_summary::CommandType::Test =>
                command_summary::summarize_test_output(&stdout, &stderr),
            command_summary::CommandType::Build =>
                command_summary::summarize_build_output(&stdout, &stderr),
            command_summary::CommandType::Generic =>
                command_summary::summarize_generic(&stdout, &stderr),
        };

        result["exit_code"] = json!(exit_code);
        result["output_id"] = json!(output_id);
        result["total_stdout_lines"] = json!(total_stdout_lines);
        if total_stderr_lines > 0 {
            result["total_stderr_lines"] = json!(total_stderr_lines);
        }
        result["hint"] = json!(format!(
            "Full output stored. Query with: run_command(\"grep/tail/awk/sed {}\")",
            output_id
        ));
        Ok(result)
    }
    Ok(Err(e)) => Err(
        super::RecoverableError::new(format!("command execution error: {}", e)).into()
    ),
    Err(_) => Ok(json!({
        "timed_out": true,
        "stdout": "",
        "stderr": format!("Command timed out after {} seconds", timeout_secs),
        "exit_code": null
    })),
}
```

Add at the top of `workflow.rs` (after existing `use` lines):

```rust
use crate::tools::command_summary;
use crate::tools::output_buffer;
```

**Step 4: Update stale tests**

The existing tests `execute_shell_command_output_truncated` and `execute_shell_command_warn_mode_includes_warning` test behaviour that no longer exists:

- Rename `execute_shell_command_output_truncated` → `execute_shell_command_large_output_buffered`. Change assertion: check `result["output_id"].is_string()` instead of `stdout_truncated`.
- Delete or update `execute_shell_command_warn_mode_includes_warning` — the `warning` field is no longer added (buffer system replaced it).

**Step 5: Run all workflow tests**

```bash
cargo test -p code-explorer workflow 2>&1 | tail -10
```
Expected: all pass.

**Step 6: Commit**

```bash
cargo fmt && cargo clippy -- -D warnings
git add src/tools/workflow.rs
git commit -m "feat(run_command): buffer-backed smart summaries + buffer ref execution"
```

---

### Task 5: `file_summary.rs` — file type summarizers for ReadFile

**Files:**
- Create: `src/tools/file_summary.rs`
- Modify: `src/tools/mod.rs` (add `pub mod file_summary;`)

**Step 1: Write the failing tests first** (put them in the new file)

```rust
// src/tools/file_summary.rs — start with just tests, no impl

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_rust_as_source() {
        assert!(matches!(detect_file_type("src/main.rs"), FileSummaryType::Source));
        assert!(matches!(detect_file_type("lib.py"), FileSummaryType::Source));
    }

    #[test]
    fn detect_md_as_markdown() {
        assert!(matches!(detect_file_type("README.md"), FileSummaryType::Markdown));
        assert!(matches!(detect_file_type("docs/guide.mdx"), FileSummaryType::Markdown));
    }

    #[test]
    fn detect_toml_as_config() {
        assert!(matches!(detect_file_type("Cargo.toml"), FileSummaryType::Config));
        assert!(matches!(detect_file_type("config.yaml"), FileSummaryType::Config));
        assert!(matches!(detect_file_type("data.json"), FileSummaryType::Config));
    }

    #[test]
    fn detect_unknown_as_generic() {
        assert!(matches!(detect_file_type("data.csv"), FileSummaryType::Generic));
        assert!(matches!(detect_file_type("Makefile"), FileSummaryType::Generic));
    }

    #[test]
    fn markdown_summary_extracts_h1_and_h2_only() {
        let content = "# Title\nsome text\n## Section\nmore text\n### Sub\nnope";
        let s = summarize_markdown(content);
        let headings = s["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].as_str().unwrap(), "# Title");
        assert_eq!(headings[1].as_str().unwrap(), "## Section");
        assert_eq!(s["line_count"].as_u64().unwrap(), 6);
    }

    #[test]
    fn config_summary_returns_first_30_lines() {
        let content: String = (1..=50).map(|i| format!("key_{} = {}\n", i, i)).collect();
        let s = summarize_config(&content);
        let preview = s["preview"].as_str().unwrap();
        assert!(preview.contains("key_1"));
        assert!(!preview.contains("key_31"));
        assert_eq!(s["line_count"].as_u64().unwrap(), 50);
    }

    #[test]
    fn generic_summary_includes_head_and_tail() {
        let content: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        let s = summarize_generic_file(&content);
        assert!(s["head"].as_str().unwrap().contains("line 1"));
        assert!(s["tail"].as_str().unwrap().contains("line 100"));
        assert!(!s["head"].as_str().unwrap().contains("line 21"));
        assert_eq!(s["line_count"].as_u64().unwrap(), 100);
    }
}
```

**Step 2: Run to confirm compilation fails**

```bash
cargo test -p code-explorer file_summary 2>&1 | grep "error\[" | head -5
```

**Step 3: Implement `src/tools/file_summary.rs`**

```rust
//! Smart summaries for large file reads — parallel to command_summary for run_command.

use serde_json::{json, Value};

pub const FILE_BUFFER_THRESHOLD: usize = 200;

pub enum FileSummaryType {
    Source,
    Markdown,
    Config,
    Generic,
}

pub fn detect_file_type(path: &str) -> FileSummaryType {
    let lower = path.to_lowercase();
    const SOURCE_EXTS: &[&str] = &[
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".java",
        ".kt", ".c", ".cpp", ".h", ".swift", ".rb", ".cs", ".php",
    ];
    const CONFIG_EXTS: &[&str] = &[
        ".toml", ".yaml", ".yml", ".json", ".xml", ".ini", ".env",
        ".lock", ".cfg",
    ];
    if SOURCE_EXTS.iter().any(|e| lower.ends_with(e)) {
        FileSummaryType::Source
    } else if lower.ends_with(".md") || lower.ends_with(".mdx") {
        FileSummaryType::Markdown
    } else if CONFIG_EXTS.iter().any(|e| lower.ends_with(e)) {
        FileSummaryType::Config
    } else {
        FileSummaryType::Generic
    }
}

/// Summarize a source file: extract top-level function signatures via AST.
/// Falls back to generic head+tail if AST parsing yields nothing.
pub fn summarize_source(path: &str, content: &str) -> Value {
    let line_count = content.lines().count();
    let symbols: Vec<String> = crate::ast::parse_functions_from_content(path, content)
        .into_iter()
        .take(30)
        .map(|f| format!("{} (line {})", f.signature, f.start_line + 1))
        .collect();
    if symbols.is_empty() {
        // Fall back to generic if tree-sitter yields nothing
        let mut result = summarize_generic_file(content);
        result["type"] = json!("source");
        return result;
    }
    json!({
        "type": "source",
        "line_count": line_count,
        "symbols": symbols,
    })
}

/// Summarize a markdown file: extract H1 and H2 headings.
pub fn summarize_markdown(content: &str) -> Value {
    let line_count = content.lines().count();
    let headings: Vec<String> = content
        .lines()
        .filter(|l| l.starts_with("# ") || l.starts_with("## "))
        .take(20)
        .map(|l| l.to_string())
        .collect();
    json!({
        "type": "markdown",
        "line_count": line_count,
        "headings": headings,
    })
}

/// Summarize a config file: first 30 lines as preview.
pub fn summarize_config(content: &str) -> Value {
    let line_count = content.lines().count();
    let preview: String = content.lines().take(30).collect::<Vec<_>>().join("\n");
    json!({
        "type": "config",
        "line_count": line_count,
        "preview": preview,
    })
}

/// Generic fallback: first 20 lines + last 10 lines.
pub fn summarize_generic_file(content: &str) -> Value {
    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();
    let head: String = lines.iter().take(20).cloned().collect::<Vec<_>>().join("\n");
    let tail: String = {
        let start = if line_count > 10 { line_count - 10 } else { 0 };
        lines[start..].join("\n")
    };
    json!({
        "type": "generic",
        "line_count": line_count,
        "head": head,
        "tail": tail,
    })
}

#[cfg(test)]
mod tests {
    // ... (tests from Step 1 go here)
}
```

**Note on `parse_functions_from_content`:** The existing `ast` module exposes `parse_functions` which takes a `Path`. Check `src/ast/mod.rs` for the actual function signature. If it takes a path+content pair, call it directly. If it reads from disk, add a `parse_functions_from_content(path: &str, content: &str) -> Vec<FunctionInfo>` wrapper that writes to a temp file or uses the in-memory parser. Use the simplest approach — if the AST parser always reads from disk, use the generic fallback unconditionally for now and file a follow-up.

**Step 4: Register module**

Add to `src/tools/mod.rs`:
```rust
pub mod file_summary;
```

**Step 5: Run tests**

```bash
cargo test -p code-explorer file_summary 2>&1 | tail -5
```
Expected: all 7 file_summary tests pass.

**Step 6: Commit**

```bash
cargo fmt && cargo clippy -- -D warnings
git add src/tools/file_summary.rs src/tools/mod.rs
git commit -m "feat(file_summary): file type detection + per-type smart summarizers"
```

---

### Task 6: `ReadFile` — large file buffering

**Files:**
- Modify: `src/tools/file.rs`

**Step 1: Write failing tests**

Add to `src/tools/file.rs` tests (the file already has a `tests` module — look for the `project_ctx` helper):

```rust
#[tokio::test]
async fn read_file_small_file_returns_content_directly() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("small.md");
    std::fs::write(&path, "# Hello\nWorld\n").unwrap();
    let ctx = project_ctx_at(dir.path()).await;
    let result = ReadFile
        .call(json!({"file_path": path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();
    assert!(result.get("file_id").is_none(), "small file should not buffer");
    assert!(result["content"].as_str().unwrap().contains("Hello"));
}

#[tokio::test]
async fn read_file_large_file_returns_buffer_ref() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("big.md");
    let content: String = (1..=210).map(|i| format!("line {}\n", i)).collect();
    std::fs::write(&path, &content).unwrap();
    let ctx = project_ctx_at(dir.path()).await;
    let result = ReadFile
        .call(json!({"file_path": path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();
    let file_id = result["file_id"].as_str().expect("large file should have file_id");
    assert!(file_id.starts_with("@file_"));
    assert!(result["hint"].as_str().unwrap().contains("@file_"));
    // Buffer should hold the full content
    let entry = ctx.output_buffer.get(file_id).unwrap();
    assert!(entry.stdout.contains("line 100"));
}

#[tokio::test]
async fn read_file_explicit_range_always_returns_directly() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("big.rs");
    let content: String = (1..=300).map(|i| format!("// line {}\n", i)).collect();
    std::fs::write(&path, &content).unwrap();
    let ctx = project_ctx_at(dir.path()).await;
    let result = ReadFile
        .call(
            json!({"file_path": path.to_str().unwrap(), "start_line": 1, "end_line": 5}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(result.get("file_id").is_none(), "explicit range should never buffer");
    assert!(result["content"].as_str().unwrap().contains("line 1"));
}

#[tokio::test]
async fn read_file_large_source_file_no_longer_errors() {
    // Previously: source files without line range returned a RecoverableError.
    // Now: they should buffer (if large) or return directly (if small).
    let dir = tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    // 210 lines of rust source
    let content: String = (0..105).map(|i| format!("fn fn_{}() {{}}\n\n", i)).collect();
    std::fs::write(&path, &content).unwrap();
    let ctx = project_ctx_at(dir.path()).await;
    let result = ReadFile
        .call(json!({"file_path": path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();
    // Should NOT be a RecoverableError — either buffered or direct
    assert!(
        result.get("file_id").is_some() || result.get("content").is_some(),
        "should buffer or return content, got: {}",
        result
    );
}
```

**Step 2: Check if `project_ctx_at` helper exists; if not, add it**

Look in the existing `tests` module for a `project_ctx` helper. If it only supports a fixed temp directory, add:

```rust
async fn project_ctx_at(root: &std::path::Path) -> ToolContext {
    // same as project_ctx() but uses the given root
    let agent = Agent::with_project_root(root.to_path_buf()).await.unwrap();
    ToolContext {
        agent,
        lsp: Arc::new(super::super::lsp::NoopLspProvider),
        output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
    }
}
```

**Step 3: Run to confirm they fail**

```bash
cargo test -p code-explorer "read_file_small|read_file_large|read_file_explicit|read_file_large_source" 2>&1 | grep "FAILED\|error\[" | head -10
```

**Step 4: Update `ReadFile::call`**

In `src/tools/file.rs`, within `impl Tool for ReadFile / call`:

1. Remove the block that returns `RecoverableError` for source files without a line range. (It checks `ast::detect_language()` and rejects if language is detected and no `start_line`/`end_line` given.)

2. After reading `content` (the full file string) and before the existing return, add:

```rust
let has_explicit_range = start_line.is_some() || end_line.is_some();
let line_count = content.lines().count();

if !has_explicit_range && line_count > crate::tools::file_summary::FILE_BUFFER_THRESHOLD {
    let file_id = ctx.output_buffer.store_file(
        file_path.to_string_lossy().to_string(),
        content.clone(),
    );
    let summary = match crate::tools::file_summary::detect_file_type(
        &file_path.to_string_lossy()
    ) {
        crate::tools::file_summary::FileSummaryType::Source =>
            crate::tools::file_summary::summarize_source(
                &file_path.to_string_lossy(), &content
            ),
        crate::tools::file_summary::FileSummaryType::Markdown =>
            crate::tools::file_summary::summarize_markdown(&content),
        crate::tools::file_summary::FileSummaryType::Config =>
            crate::tools::file_summary::summarize_config(&content),
        crate::tools::file_summary::FileSummaryType::Generic =>
            crate::tools::file_summary::summarize_generic_file(&content),
    };
    let mut result = summary;
    result["file_id"] = json!(file_id);
    result["hint"] = json!(format!(
        "Full file stored. Query with: run_command(\"grep/sed/awk {}\")",
        file_id
    ));
    return Ok(result);
}
```

**Step 5: Run tests**

```bash
cargo test -p code-explorer read_file 2>&1 | tail -10
```
Expected: all read_file tests pass including the 4 new ones.

**Step 6: Commit**

```bash
cargo fmt && cargo clippy -- -D warnings
git add src/tools/file.rs
git commit -m "feat(read_file): buffer large files with smart summaries + @file_* refs"
```

---

### Task 7: Server instructions update

**Files:**
- Modify: `src/prompts/server_instructions.md`

**Step 1: Add the Output Buffers concept section**

After the `## Output Modes` section, insert a new section:

```markdown
## Output Buffers

Large content — whether from a command or a file read — is stored in an
`OutputBuffer` rather than dumped into your context. You get a smart summary
and an `@ref` handle (`@cmd_*` for commands, `@file_*` for files). The full
content costs you nothing to hold. Query it via `run_command` + Unix tools:
  run_command("grep FAILED @cmd_a1b2c3")
  run_command("sed -n '42,80p' @file_abc123")
  run_command("diff @cmd_a1b2c3 @file_abc123")
**Be targeted:** extract what you need in one well-crafted query per buffer —
don't probe the same `@ref` multiple times for overlapping information.
```

**Step 2: Replace the `run_command` line**

Find:
```
- `run_command(command)` — run a shell command in the active project root and return stdout/stderr.
```

Replace with:
```markdown
- `run_command(command)` — execute a shell command. Run freely even if output
  might be large; the buffer handles it. Returns content directly for short
  output, smart summary + `@cmd_*` ref for large output.
  - `cwd` — run from a subdirectory (relative to project root)
  - `acknowledge_risk` — bypass safety check for destructive commands
```

**Step 3: Update the `read_file` line**

Find the existing `read_file` description and replace/update to add buffer framing:
```markdown
- `read_file(path)` — read a file. Returns content directly for short files,
  smart summary + `@file_*` ref for large files (> 200 lines). For source code
  files the summary includes top-level symbols. Prefer `list_symbols` /
  `find_symbol` for source code navigation — they are more structured and
  token-efficient.
```

**Step 4: Verify**

```bash
grep -n "OutputBuffer\|@cmd_\|@file_\|run_command\|read_file" src/prompts/server_instructions.md | head -20
```

**Step 5: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs(server_instructions): add OutputBuffer concept + update run_command + read_file"
```

---

### Task 8: Routing plugin — block all Bash calls

**Files:**
- Modify: `../claude-plugins/code-explorer-routing/hooks/pre-tool-guard.sh`

**Step 1: Find the Bash case**

```bash
grep -n "Bash)" ../claude-plugins/code-explorer-routing/hooks/pre-tool-guard.sh
```

**Step 2: Replace the Bash case body**

The current Bash case checks for specific read-like commands on source files. Replace the entire body (between `Bash)` and `;;`) with a blanket block:

```bash
  Bash)
    # When code-explorer is available, all Bash calls route through run_command.
    [ "$HAS_CODE_EXPLORER" = "false" ] && exit 0

    CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
    deny "⛔ BLOCKED: Use run_command(\"${CMD}\") instead of Bash.
run_command provides:
  - Smart output summaries (test results, build errors)
  - Output buffers queryable with grep/tail/awk/sed @output_id or @file_id
  - Dangerous command detection with escape hatch (acknowledge_risk: true)
  - Runs in project root with optional cwd parameter"
    ;;
```

**Step 3: Manual test**

In a new Claude Code session on this project, try `Bash("cargo test")` — confirm the block message appears and suggests `run_command("cargo test")`.

**Step 4: Commit**

```bash
cd ../claude-plugins
git add code-explorer-routing/hooks/pre-tool-guard.sh
git commit -m "feat(routing): block all Bash calls and redirect to run_command"
cd -
```

---

### Task 9: Integration tests + full test suite

**Files:**
- Modify: `tests/integration.rs`

**Step 1: Write integration tests**

Add to `tests/integration.rs`:

```rust
#[tokio::test]
async fn integration_run_command_buffer_round_trip() {
    let server = test_server().await;

    // Generate > 50 lines
    let r1 = server.call_tool("run_command", json!({"command": "seq 1 100"})).await;
    let output_id = r1["output_id"].as_str().expect("should buffer");
    assert!(output_id.starts_with("@cmd_"));

    // Query the buffer
    let r2 = server.call_tool("run_command", json!({
        "command": format!("grep '^50$' {}", output_id)
    })).await;
    assert_eq!(r2["exit_code"], 0);
    assert_eq!(r2["stdout"].as_str().unwrap().trim(), "50");
}

#[tokio::test]
async fn integration_read_file_large_then_query_via_buffer() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let content: String = (1..=250).map(|i| format!("entry {}\n", i)).collect();
    std::fs::write(&path, &content).unwrap();

    let server = test_server_at(dir.path()).await;
    let r1 = server.call_tool("read_file", json!({
        "file_path": path.to_str().unwrap()
    })).await;
    let file_id = r1["file_id"].as_str().expect("large file should buffer");
    assert!(file_id.starts_with("@file_"));

    let r2 = server.call_tool("run_command", json!({
        "command": format!("grep 'entry 200' {}", file_id)
    })).await;
    assert_eq!(r2["exit_code"], 0);
    assert!(r2["stdout"].as_str().unwrap().contains("entry 200"));
}

#[tokio::test]
async fn integration_speed_bump_two_round_trips() {
    let server = test_server().await;

    // First call: blocked
    let r1 = server.call_tool("run_command", json!({
        "command": "rm -rf /tmp/ce_integration_test_nonexistent_dir"
    })).await;
    assert!(r1["error"].as_str().unwrap().contains("Dangerous"));
    assert!(r1["hint"].as_str().unwrap().contains("acknowledge_risk"));

    // Second call: acknowledged → executes
    let r2 = server.call_tool("run_command", json!({
        "command": "rm -rf /tmp/ce_integration_test_nonexistent_dir",
        "acknowledge_risk": true
    })).await;
    // rm on non-existent path: exits with code 1 but runs (not a "Dangerous" error)
    assert!(r2["exit_code"].is_number());
    assert!(r2.get("error")
        .map(|e| !e.as_str().unwrap_or("").contains("Dangerous"))
        .unwrap_or(true));
}
```

**Step 2: Run integration tests**

```bash
cargo test --test integration 2>&1 | tail -15
```

**Step 3: Full test suite**

```bash
cargo test 2>&1 | tail -5
```
Expected: all tests pass. Verify the count is at least 591 (baseline) + new tests.

**Step 4: Final checks**

```bash
cargo clippy -- -D warnings 2>&1 | grep "^error"
cargo fmt --check
```

**Step 5: Final commit**

```bash
git add tests/integration.rs
git commit -m "test(integration): buffer round-trips + speed bump + read_file buffer"
```

---

## Completion Checklist

- [ ] `OutputBuffer::store_file()` — `@file_*` entries
- [ ] `OutputBuffer::resolve_refs()` — substitutes refs with temp file paths
- [ ] `OutputBuffer::is_buffer_only()` — detects buffer-only commands
- [ ] `RunCommand` — `cwd` + `acknowledge_risk` in input schema
- [ ] `RunCommand` — dangerous command speed bump (skipped for buffer-only)
- [ ] `RunCommand` — buffer-backed smart summaries for large output
- [ ] `RunCommand` — buffer ref execution via `resolve_refs()`
- [ ] `file_summary.rs` — `detect_file_type()` + 4 summarizers
- [ ] `ReadFile` — large file buffering (> 200 lines), source-file block removed
- [ ] `src/prompts/server_instructions.md` — Output Buffers section + updated tool entries
- [ ] `pre-tool-guard.sh` — all Bash calls blocked and redirected to `run_command`
- [ ] All integration tests pass
- [ ] `cargo test` all pass, `cargo clippy -- -D warnings` clean
