# CWD Path Stripping Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Strip the absolute project root prefix from all MCP tool output in one place to reduce token waste.

**Architecture:** A single helper `strip_project_root_from_result` is called at the end of `call_tool` in `src/server.rs`, after the `CallToolResult` is assembled from either a successful tool call or `route_tool_error`. The root prefix is read dynamically from `self.agent.project_root()` so `activate_project` mid-session works correctly. No per-tool changes needed.

**Tech Stack:** Rust, `rmcp` (`CallToolResult`, `Content`), `serde_json`

**Design doc:** `docs/plans/2026-03-02-cwd-path-stripping-design.md`

---

### Task 1: Add unit tests for `strip_project_root_from_result`

**Files:**
- Modify: `src/server.rs` (tests module at bottom)

**Step 1: Write three failing tests**

Add inside the existing `#[cfg(test)] mod tests` block at the bottom of `src/server.rs`:

```rust
#[test]
fn strip_project_root_removes_prefix_from_text_content() {
    let prefix = "/home/user/myproject/";
    let result = CallToolResult::success(vec![Content::text(
        r#"{"file":"/home/user/myproject/src/foo.rs","line":1}"#,
    )]);
    let stripped = strip_project_root_from_result(result, prefix);
    let text = extract_text(&stripped);
    assert_eq!(text, r#"{"file":"src/foo.rs","line":1}"#);
}

#[test]
fn strip_project_root_no_op_when_prefix_empty() {
    let result = CallToolResult::success(vec![Content::text("some output")]);
    let stripped = strip_project_root_from_result(result, "");
    assert_eq!(extract_text(&stripped), "some output");
}

#[test]
fn strip_project_root_no_op_when_prefix_absent() {
    let prefix = "/home/user/myproject/";
    let result = CallToolResult::success(vec![Content::text("no paths here")]);
    let stripped = strip_project_root_from_result(result, prefix);
    assert_eq!(extract_text(&stripped), "no paths here");
}

// Helper: extract the text from the first text content block.
fn extract_text(result: &CallToolResult) -> &str {
    result
        .content
        .iter()
        .find_map(|c| if let Content::Text(t) = c { Some(t.text.as_str()) } else { None })
        .unwrap_or("")
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test strip_project_root 2>&1 | head -30
```

Expected: compile error — `strip_project_root_from_result` not defined yet.

---

### Task 2: Implement `strip_project_root_from_result`

**Files:**
- Modify: `src/server.rs` (add function before the `tests` module)

**Step 1: Add the helper function**

Add this function just before the `#[cfg(test)]` block at the bottom of `src/server.rs`:

```rust
/// Strips the absolute project root prefix from all text content blocks in a
/// `CallToolResult`. This normalises tool output to relative paths, reducing
/// token usage — the prefix (e.g. "/home/user/work/project/") is identical in
/// every response and carries no information since agents always operate within
/// the project directory.
///
/// `root_prefix` must end with `/`. Pass an empty string when no project is
/// active; the replace becomes a no-op.
///
/// Buffer content (`@tool_xxx` refs) is covered automatically: it only
/// re-enters the pipeline through `run_command`, which also passes through
/// `call_tool` and gets stripped there.
fn strip_project_root_from_result(mut result: CallToolResult, root_prefix: &str) -> CallToolResult {
    if root_prefix.is_empty() {
        return result;
    }
    for block in &mut result.content {
        if let Content::Text(ref mut t) = block {
            if t.text.contains(root_prefix) {
                t.text = t.text.replace(root_prefix, "");
            }
        }
    }
    result
}
```

**Step 2: Run unit tests to verify they pass**

```bash
cargo test strip_project_root 2>&1 | tail -10
```

Expected: all 3 tests pass.

**Step 3: Check clippy**

```bash
cargo clippy -- -D warnings 2>&1 | tail -20
```

Expected: no warnings.

**Step 4: Commit**

```bash
git add src/server.rs
git commit -m "feat(server): add strip_project_root_from_result helper"
```

---

### Task 3: Wire `strip_project_root_from_result` into `call_tool`

**Files:**
- Modify: `src/server.rs` — `call_tool` method (`impl ServerHandler for CodeExplorerServer`)

**Context:** The current end of `call_tool` looks like this (around line 230):

```rust
        match result {
            Ok(mut blocks) => {
                if !USER_OUTPUT_ENABLED {
                    blocks.retain(|b| b.audience() != Some(&vec![Role::User]));
                }
                Ok(CallToolResult::success(blocks))
            }
            Err(e) => Ok(route_tool_error(e)),
        }
```

**Step 1: Write a failing integration test first**

Add inside `#[cfg(test)] mod tests` in `src/server.rs`:

```rust
#[tokio::test]
async fn call_tool_strips_project_root_from_output() {
    let (dir, server) = make_server();
    let root = dir.path().to_string_lossy().to_string();

    // list_dir on the project root will return paths like "{root}/..."
    // After stripping, they should be relative.
    let req = CallToolRequestParam {
        name: "list_dir".into(),
        arguments: Some(serde_json::from_value(serde_json::json!({"path": "."})).unwrap()),
    };
    let ctx = rmcp::RequestContext::default();
    let result = server.call_tool(req, ctx).await.unwrap();

    let text = result
        .content
        .iter()
        .find_map(|c| if let Content::Text(t) = c { Some(&t.text) } else { None })
        .unwrap();

    assert!(
        !text.contains(&root),
        "Expected absolute root to be stripped, but found it in output:\n{text}"
    );
}
```

**Step 2: Run the test to verify it fails**

```bash
cargo test call_tool_strips_project_root 2>&1 | tail -20
```

Expected: FAIL — the absolute root appears in the output.

**Step 3: Replace the `match result` block at the end of `call_tool`**

Replace:
```rust
        match result {
            Ok(mut blocks) => {
                if !USER_OUTPUT_ENABLED {
                    blocks.retain(|b| b.audience() != Some(&vec![Role::User]));
                }
                Ok(CallToolResult::success(blocks))
            }
            Err(e) => Ok(route_tool_error(e)),
        }
```

With:
```rust
        // Assemble the result — success or error both produce a CallToolResult
        // so we can apply post-processing in one place.
        let call_result = match result {
            Ok(mut blocks) => {
                if !USER_OUTPUT_ENABLED {
                    blocks.retain(|b| b.audience() != Some(&vec![Role::User]));
                }
                CallToolResult::success(blocks)
            }
            Err(e) => route_tool_error(e),
        };

        // Strip the absolute project root from all output to reduce token usage.
        // Agents work exclusively within the project directory; relative paths
        // carry all necessary information. The full root (e.g. /home/user/project)
        // is a long repeated prefix that appears in every "file" field and error
        // message. Buffer content (@tool_xxx refs) is covered here too: it only
        // re-enters the pipeline through run_command, which also passes through
        // call_tool.
        let root_prefix = self
            .agent
            .project_root()
            .await
            .map(|p| format!("{}/", p.display()))
            .unwrap_or_default();

        Ok(strip_project_root_from_result(call_result, &root_prefix))
```

**Step 4: Run the integration test to verify it passes**

```bash
cargo test call_tool_strips_project_root 2>&1 | tail -10
```

Expected: PASS.

**Step 5: Run the full test suite**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass. If any existing test asserts on absolute paths in tool output, update the assertion to use the relative form.

**Step 6: Clippy and format**

```bash
cargo clippy -- -D warnings && cargo fmt
```

Expected: clean.

**Step 7: Commit**

```bash
git add src/server.rs
git commit -m "feat(server): strip project root from all tool output in call_tool

Reduces token usage by replacing the absolute project root prefix with
empty string in every CallToolResult content block. Single intercept
point covers: JSON tool results, format_compact summaries, error messages,
and buffer query results (via run_command which also passes through call_tool).

Prefix is read dynamically so activate_project mid-session works correctly.
No-op when no project is active (empty prefix)."
```

---

### Task 4: Update server instructions

**Files:**
- Modify: `src/prompts/server_instructions.md`

The server instructions tell the agent about file paths. Check if any instruction mentions that paths are absolute — if so, update to note they are relative to the project root.

**Step 1: Check for mentions of absolute paths**

```bash
grep -n "absolute\|/home\|project root\|full path" src/prompts/server_instructions.md
```

**Step 2: If found, update the relevant lines**

Change any guidance suggesting paths are absolute to clarify they are relative to the project root. Example:

Before: `Returns absolute file paths`  
After: `Returns file paths relative to the project root`

**Step 3: Run tests**

```bash
cargo test 2>&1 | tail -10
```

**Step 4: Commit if changed**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs(prompts): note that file paths in output are relative to project root"
```

If no mention of absolute paths found, skip the commit — no change needed.
