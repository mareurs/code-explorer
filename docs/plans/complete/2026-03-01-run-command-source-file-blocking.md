# run_command Source-File Blocking Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Block file-reading shell commands (`cat`, `head`, `tail`, `sed`, `awk`, `less`, `more`, `wc`) when they target source code files inside `run_command`, redirecting agents to code-explorer tools with actionable hints.

**Architecture:** New `check_source_file_access(command: &str) -> Option<String>` function in `src/util/path_security.rs` uses two-part regex (blocked command name + source extension). Called in `run_command_inner` at step 2.5, after the existing dangerous-command speed bump. Bypassed by `buffer_only` and `acknowledge_risk: true` — same escape hatch as dangerous commands.

**Tech Stack:** Rust, `regex` crate (already a dependency), `anyhow`/`RecoverableError` (existing patterns in the codebase).

**Design doc:** `docs/plans/2026-03-01-run-command-source-file-blocking-design.md`

---

### Task 1: Add `check_source_file_access` with unit tests

**Files:**
- Modify: `src/util/path_security.rs`

**Step 1: Write the failing tests**

At the bottom of `src/util/path_security.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block, add:

```rust
#[test]
fn source_file_access_blocks_cat_on_rs() {
    assert!(check_source_file_access("cat src/main.rs").is_some());
}

#[test]
fn source_file_access_blocks_head_on_ts() {
    assert!(check_source_file_access("head -20 src/tools/mod.ts").is_some());
}

#[test]
fn source_file_access_blocks_tail_on_go() {
    assert!(check_source_file_access("tail -n 50 server.go").is_some());
}

#[test]
fn source_file_access_blocks_sed_on_py() {
    assert!(check_source_file_access("sed -n '1,100p' lib.py").is_some());
}

#[test]
fn source_file_access_blocks_awk_on_java() {
    assert!(check_source_file_access("awk '{print}' Foo.java").is_some());
}

#[test]
fn source_file_access_blocks_less_on_rs() {
    assert!(check_source_file_access("less src/agent.rs").is_some());
}

#[test]
fn source_file_access_blocks_wc_on_rs() {
    assert!(check_source_file_access("wc -l src/lib.rs").is_some());
}

#[test]
fn source_file_access_allows_cat_on_markdown() {
    assert!(check_source_file_access("cat README.md").is_none());
}

#[test]
fn source_file_access_allows_wc_on_txt() {
    assert!(check_source_file_access("wc -l output.txt").is_none());
}

#[test]
fn source_file_access_allows_sed_on_toml() {
    assert!(check_source_file_access("sed 's/foo/bar/g' config.toml").is_none());
}

#[test]
fn source_file_access_allows_cat_without_source_ext() {
    assert!(check_source_file_access("cat Makefile").is_none());
}

#[test]
fn source_file_access_hint_mentions_read_file() {
    let hint = check_source_file_access("cat src/main.rs").unwrap();
    assert!(hint.contains("read_file"), "hint should mention read_file, got: {hint}");
}

#[test]
fn source_file_access_hint_mentions_list_symbols() {
    let hint = check_source_file_access("head -5 lib.rs").unwrap();
    assert!(hint.contains("list_symbols"), "hint should mention list_symbols, got: {hint}");
}

#[test]
fn source_file_access_sed_hint_mentions_search_pattern() {
    let hint = check_source_file_access("sed -n '1p' foo.ts").unwrap();
    assert!(hint.contains("search_pattern"), "sed hint should mention search_pattern, got: {hint}");
}

#[test]
fn source_file_access_allows_non_blocked_command() {
    // `cp`, `mv`, `diff` are not in the blocked set
    assert!(check_source_file_access("cp src/main.rs src/main2.rs").is_none());
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test source_file_access 2>&1 | tail -20
```
Expected: compilation error — `check_source_file_access` not found.

**Step 3: Implement `check_source_file_access`**

In `src/util/path_security.rs`, add this constant and function **before** the `#[cfg(test)]` block (place it after the `is_dangerous_command` function):

```rust
/// Source file extensions that should be accessed via code-explorer tools,
/// not raw shell commands. Mirrors `crate::ast::detect_language()` minus markdown.
const SOURCE_EXTENSIONS: &str =
    r"\.(rs|py|ts|tsx|js|cjs|mjs|jsx|go|java|kt|kts|c|cpp|cc|cxx|cs|rb|php|swift|scala|ex|exs|hs|lua|sh|bash)\b";

/// Shell commands whose primary job is reading file content.
const SOURCE_ACCESS_COMMANDS: &str = r"\b(cat|head|tail|sed|awk|less|more|wc)\b";

/// Returns a hint string if `command` is a file-reading tool targeting a source file,
/// `None` if the command is safe to execute.
///
/// Two-part heuristic: blocked command name AND source extension both present.
/// Known limit: variable expansion (`cat $FILE`) is undetectable — accepted.
pub fn check_source_file_access(command: &str) -> Option<String> {
    let cmd_re = Regex::new(SOURCE_ACCESS_COMMANDS).ok()?;
    let ext_re = Regex::new(SOURCE_EXTENSIONS).ok()?;

    if !cmd_re.is_match(command) || !ext_re.is_match(command) {
        return None;
    }

    // Tailor hint based on the detected command
    let hint = if let Some(m) = cmd_re.find(command) {
        match m.as_str() {
            "sed" | "awk" => {
                "use read_file(path, start_line, end_line), list_symbols(path), \
                 find_symbol(name, include_body=true), or search_pattern(regex) instead. \
                 Re-run with acknowledge_risk: true if you need raw shell access."
            }
            _ => {
                "use read_file(path, start_line, end_line) or list_symbols(path) + \
                 find_symbol(name, include_body=true) instead. \
                 Re-run with acknowledge_risk: true if you need raw shell access."
            }
        }
    } else {
        "use read_file, list_symbols, or find_symbol instead. \
         Re-run with acknowledge_risk: true if you need raw shell access."
    };

    Some(hint.to_string())
}
```

**Step 4: Run tests to verify they pass**

```bash
cargo test source_file_access 2>&1 | tail -20
```
Expected: all tests pass.

**Step 5: Run clippy and fmt**

```bash
cargo fmt && cargo clippy -- -D warnings 2>&1 | tail -20
```
Expected: no warnings.

**Step 6: Commit**

```bash
git add src/util/path_security.rs
git commit -m "feat(security): add check_source_file_access for run_command blocking"
```

---

### Task 2: Integrate the check into `run_command_inner`

**Files:**
- Modify: `src/tools/workflow.rs`

**Step 1: Write the failing integration tests**

In `src/tools/workflow.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block, add after the existing `run_command_*` tests:

```rust
#[tokio::test]
async fn run_command_blocks_cat_on_source_file() {
    let (_dir, ctx) = project_ctx().await;
    let result = RunCommand
        .call(
            json!({ "command": "cat src/main.rs" }),
            &ctx,
        )
        .await;
    // Should be a RecoverableError (not a hard anyhow error)
    let err = result.unwrap_err();
    let recoverable = err.downcast::<crate::tools::RecoverableError>().unwrap();
    assert!(
        recoverable.message.contains("source files is blocked"),
        "expected source-file block message, got: {}",
        recoverable.message
    );
}

#[tokio::test]
async fn run_command_source_block_bypassed_with_acknowledge_risk() {
    let (dir, ctx) = project_ctx().await;
    // Create an actual file so the command doesn't fail for wrong reasons
    std::fs::write(dir.path().join("tiny.rs"), "fn main() {}\n").unwrap();
    let result = RunCommand
        .call(
            json!({
                "command": "cat tiny.rs",
                "acknowledge_risk": true
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok(), "acknowledge_risk should bypass source block");
}

#[tokio::test]
async fn run_command_source_block_not_triggered_for_markdown() {
    let (dir, ctx) = project_ctx().await;
    std::fs::write(dir.path().join("README.md"), "# hello\n").unwrap();
    let result = RunCommand
        .call(
            json!({ "command": "cat README.md" }),
            &ctx,
        )
        .await;
    assert!(result.is_ok(), "cat on markdown should not be blocked");
}

#[tokio::test]
async fn run_command_source_block_not_triggered_for_non_source() {
    let (dir, ctx) = project_ctx().await;
    std::fs::write(dir.path().join("data.txt"), "hello\n").unwrap();
    let result = RunCommand
        .call(
            json!({ "command": "cat data.txt" }),
            &ctx,
        )
        .await;
    assert!(result.is_ok(), "cat on .txt should not be blocked");
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test run_command_blocks_cat 2>&1 | tail -20
cargo test run_command_source_block 2>&1 | tail -20
```
Expected: tests fail — `cat src/main.rs` is not blocked yet.

**Step 3: Add step 2.5 to `run_command_inner`**

In `src/tools/workflow.rs`, find the `run_command_inner` function. After the `// --- Step 2: Dangerous command speed bump ---` block (around line 492), add:

```rust
    // --- Step 2.5: Source file access block ---
    if !buffer_only && !acknowledge_risk {
        if let Some(hint) = crate::util::path_security::check_source_file_access(resolved_command) {
            return Err(super::RecoverableError::with_hint(
                "shell access to source files is blocked",
                &hint,
            )
            .into());
        }
    }
```

**Step 4: Run the integration tests to verify they pass**

```bash
cargo test run_command_blocks_cat 2>&1 | tail -20
cargo test run_command_source_block 2>&1 | tail -20
```
Expected: all four tests pass.

**Step 5: Run the full test suite**

```bash
cargo test 2>&1 | tail -30
```
Expected: all tests pass (was ~591 before this change; new total ~606).

**Step 6: Run clippy and fmt**

```bash
cargo fmt && cargo clippy -- -D warnings 2>&1 | tail -20
```
Expected: no warnings.

**Step 7: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat(run_command): block file-reading shell commands on source files"
```

---

### Task 3: Update server instructions

**Files:**
- Modify: `src/prompts/server_instructions.md`

**Step 1: Find the run_command section**

```bash
grep -n "run_command\|shell\|source" src/prompts/server_instructions.md | head -30
```

**Step 2: Add a note about source-file blocking**

Find the `run_command` tool description in `src/prompts/server_instructions.md`. Add a note like:

```markdown
**Anti-patterns (never do these):**
- `cat src/foo.rs` → use `read_file` or `list_symbols` + `find_symbol`
- `head -20 lib.py` → use `read_file(path, start_line=1, end_line=20)`
- `sed -n '1,50p' main.ts` → use `read_file(path, start_line=1, end_line=50)` or `search_pattern`
- `awk '{print}' server.go` → use `search_pattern` or `find_symbol`

Shell access to source files is blocked by `run_command` — use code-explorer symbol tools instead.
```

**Step 3: Run tests (no code changed, just docs)**

```bash
cargo test 2>&1 | tail -10
```
Expected: same count as after Task 2.

**Step 4: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs(instructions): document run_command source-file blocking anti-patterns"
```

---

## Verification

After all tasks, run the full suite:

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

Expected:
- Zero clippy warnings
- All tests pass (~606+ total)
- `check_source_file_access` unit tests all green
- Four new `run_command` integration tests all green
