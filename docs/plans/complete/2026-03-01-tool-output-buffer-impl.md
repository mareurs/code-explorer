# Tool Output Auto-Buffering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Route all large tool outputs through `OutputBuffer` automatically, so agents see a compact summary + `@tool_xxx` ref instead of a raw JSON wall when tool responses exceed 10k bytes.

**Architecture:** Add `store_tool()` to `OutputBuffer`, update `resolve_refs` to recognize `@tool_xxx` refs, and modify the `call_content()` default in the `Tool` trait to auto-buffer when `json.len() > TOOL_OUTPUT_BUFFER_THRESHOLD`. Zero per-tool changes needed for buffering to work; optional `format_for_user()` overrides produce better compact summaries.

**Tech Stack:** Rust, serde_json, existing `OutputBuffer` (LRU + temp files), `rmcp` `Content` type, `#[async_trait]`

**Design doc:** `docs/plans/2026-03-01-tool-output-buffer-design.md`

---

### Task 1: Add `store_tool()` to `OutputBuffer`

**Files:**
- Modify: `src/tools/output_buffer.rs` (after `store_file` at line 137)

`store_tool` is nearly identical to `store_file` but uses the `@tool_` prefix and takes a tool name (not a file path) as the `command` field. Copy `store_file` and change 3 things: the prefix string, the parameter name, and the `command` field assignment.

**Step 1: Write the failing test** (at the bottom of `src/tools/output_buffer.rs`, inside the `tests` module)

```rust
#[test]
fn store_tool_generates_tool_ref() {
    let buf = OutputBuffer::new(10);
    let id = buf.store_tool("list_symbols", "{\"symbols\":[]}".to_string());
    assert!(id.starts_with("@tool_"), "expected @tool_ prefix, got {}", id);
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
```

**Step 2: Run tests to verify they fail**

Run: `cargo test store_tool -- --nocapture`
Expected: FAIL with "method not found" or similar compile error

**Step 3: Add `store_tool()` after `store_file()` (~line 138)**

```rust
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
```

**Step 4: Run tests to verify they pass**

Run: `cargo test store_tool -- --nocapture`
Expected: 2 PASS

**Step 5: Commit**

```bash
git add src/tools/output_buffer.rs
git commit -m "feat: add OutputBuffer::store_tool() for tool output buffering"
```

---

### Task 2: Update `resolve_refs` to recognize `@tool_xxx`

**Files:**
- Modify: `src/tools/output_buffer.rs:149` (one character change in the regex)

**Step 1: Write the failing test** (in the `tests` module of `output_buffer.rs`)

```rust
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test resolve_refs_substitutes_tool_ref -- --nocapture`
Expected: FAIL — `RecoverableError` "buffer reference not found: @tool_..." because the regex doesn't match `@tool_` yet.

**Step 3: Update the regex in `resolve_refs` (line 149)**

Change:
```rust
let re = Regex::new(r"@(?:cmd|file)_[0-9a-f]{8}(\.err)?").expect("valid regex");
```
To:
```rust
let re = Regex::new(r"@(?:cmd|file|tool)_[0-9a-f]{8}(\.err)?").expect("valid regex");
```

**Step 4: Run test to verify it passes**

Run: `cargo test resolve_refs_substitutes_tool_ref -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/output_buffer.rs
git commit -m "feat: resolve_refs recognizes @tool_xxx buffer refs"
```

---

### Task 3: Auto-buffer large responses in `call_content()`

**Files:**
- Modify: `src/tools/mod.rs` (the `call_content` default impl at line 196–210)

The current `call_content` has two branches: split-audience (when `format_for_user` is `Some`) and pretty-print (when `None`). Add a new branch at the top: if compact JSON exceeds the threshold, buffer it and return a compact summary. Both paths below the threshold keep their existing behaviour.

**Step 1: Write the failing tests** (in the `#[cfg(test)]` block of `src/tools/mod.rs`)

Add these imports to the existing test module:
```rust
use crate::agent::Agent;
use crate::lsp::LspManager;
use crate::tools::output_buffer::OutputBuffer;
use std::sync::Arc;
```

Add a test helper and a `MinimalTool` struct:
```rust
async fn bare_ctx() -> ToolContext {
    ToolContext {
        agent: Agent::new(None).await.unwrap(),
        lsp: LspManager::new_arc(),
        output_buffer: Arc::new(OutputBuffer::new(20)),
    }
}

struct EchoTool {
    result: Value,
    user_summary: Option<String>,
}

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str { "echo_tool" }
    fn description(&self) -> &str { "test" }
    fn input_schema(&self) -> Value { json!({}) }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Ok(self.result.clone())
    }
    fn format_for_user(&self, _result: &Value) -> Option<String> {
        self.user_summary.clone()
    }
}
```

Now the four tests:
```rust
#[tokio::test]
async fn call_content_passthrough_small_output() {
    let ctx = bare_ctx().await;
    let result = json!({"key": "value"});
    let tool = EchoTool { result: result.clone(), user_summary: None };
    let content = tool.call_content(json!({}), &ctx).await.unwrap();
    // Small output: no buffering, buffer should be empty
    assert_eq!(ctx.output_buffer.get("@tool_"), None);
    // Content should contain the JSON
    assert!(content[0].text().unwrap_or("").contains("key"));
}

#[tokio::test]
async fn call_content_buffers_large_output() {
    let ctx = bare_ctx().await;
    // Build a Value that serializes to > 10_000 bytes
    let big_array: Vec<Value> = (0..500)
        .map(|i| json!({"index": i, "name": format!("symbol_{}", i), "file": "src/tools/symbol.rs"}))
        .collect();
    let result = json!({ "symbols": big_array });
    let tool = EchoTool { result, user_summary: None };
    let content = tool.call_content(json!({}), &ctx).await.unwrap();
    // Must return exactly 1 Content item
    assert_eq!(content.len(), 1);
    let text = content[0].text().unwrap_or("");
    // Contains a @tool_ ref handle
    assert!(text.contains("@tool_"), "expected @tool_ ref in: {}", text);
}

#[tokio::test]
async fn call_content_uses_format_for_user_in_compact_text() {
    let ctx = bare_ctx().await;
    let big_array: Vec<Value> = (0..500)
        .map(|i| json!({"index": i, "name": format!("symbol_{}", i)}))
        .collect();
    let result = json!({ "symbols": big_array });
    let tool = EchoTool {
        result,
        user_summary: Some("Found 500 symbols".to_string()),
    };
    let content = tool.call_content(json!({}), &ctx).await.unwrap();
    let text = content[0].text().unwrap_or("");
    assert!(text.contains("Found 500 symbols"), "expected summary in: {}", text);
    assert!(text.contains("@tool_"), "expected ref handle in: {}", text);
}

#[tokio::test]
async fn call_content_generic_fallback_without_format_for_user() {
    let ctx = bare_ctx().await;
    let big_array: Vec<Value> = (0..500)
        .map(|i| json!({"index": i, "name": format!("symbol_{}", i)}))
        .collect();
    let result = json!({ "symbols": big_array });
    let tool = EchoTool { result, user_summary: None };
    let content = tool.call_content(json!({}), &ctx).await.unwrap();
    let text = content[0].text().unwrap_or("");
    // No format_for_user → generic fallback message with byte count and ref
    assert!(text.contains("bytes") || text.contains("stored"), "expected fallback in: {}", text);
    assert!(text.contains("@tool_"), "expected ref handle in: {}", text);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test call_content_buffers -- --nocapture`
Expected: `call_content_buffers_large_output` should FAIL (large output currently returned verbatim, no `@tool_` ref)

**Step 3: Add the threshold constant and update `call_content`**

In `src/tools/mod.rs`, add the constant after the imports (around line 25):
```rust
/// Compact JSON size above which tool output is routed through OutputBuffer.
const TOOL_OUTPUT_BUFFER_THRESHOLD: usize = 10_000;
```

Replace the entire `call_content` method body (lines 196–210):
```rust
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    let val = self.call(input, ctx).await?;
    let json = serde_json::to_string(&val).unwrap_or_else(|_| val.to_string());

    if json.len() > TOOL_OUTPUT_BUFFER_THRESHOLD {
        let ref_id = ctx.output_buffer.store_tool(self.name(), json.clone());
        let summary = self
            .format_for_user(&val)
            .unwrap_or_else(|| format!("Result stored in {} ({} bytes)", ref_id, json.len()));
        return Ok(vec![Content::text(format!(
            "{}\nFull result: {}",
            summary, ref_id
        ))]);
    }

    match self.format_for_user(&val) {
        Some(user_text) => Ok(vec![
            Content::text(json).with_audience(vec![Role::Assistant]),
            Content::text(user_text).with_audience(vec![Role::User]),
        ]),
        None => Ok(vec![Content::text(
            serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string()),
        )]),
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test call_content -- --nocapture`
Expected: all 4 new tests PASS, all existing tests still PASS

Run: `cargo test`
Expected: all tests pass

**Step 5: Commit**

```bash
git add src/tools/mod.rs
git commit -m "feat: auto-buffer large tool outputs in call_content() default impl"
```

---

### Task 4: Add `format_for_user` to `FindReferences`

**Files:**
- Modify: `src/tools/user_format.rs` (add `format_find_references` after `format_find_symbol`)
- Modify: `src/tools/symbol.rs` (`FindReferences` impl, add `format_for_user` override)

The `find_references` JSON result contains a `"references"` array and a `"total"` field. Add a compact formatter that reports the count and file distribution.

**Step 1: Write the failing test** (in `user_format.rs` test module)

```rust
#[test]
fn find_references_basic() {
    let result = json!({
        "references": [
            {"file": "src/foo.rs", "line": 10, "kind": "usage"},
            {"file": "src/bar.rs", "line": 20, "kind": "usage"},
            {"file": "src/foo.rs", "line": 30, "kind": "usage"}
        ],
        "total": 3
    });
    let text = format_find_references(&result);
    assert!(text.contains("3"), "should mention count");
    assert!(text.contains("reference"), "should say reference(s)");
}

#[test]
fn find_references_empty() {
    let result = json!({ "references": [], "total": 0 });
    let text = format_find_references(&result);
    assert!(text.contains("0") || text.contains("No"), "should indicate no refs");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test find_references_basic -- --nocapture`
Expected: FAIL — `format_find_references` not found

**Step 3: Add `format_find_references` to `user_format.rs`**

Add after `format_find_symbol` (around line 455):
```rust
pub fn format_find_references(result: &Value) -> String {
    let refs = result["references"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let total = result["total"].as_u64().unwrap_or(refs.len() as u64);

    if total == 0 {
        return "No references found.".to_string();
    }

    // Count unique files
    let mut files: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for r in refs {
        if let Some(f) = r["file"].as_str() {
            files.insert(f);
        }
    }

    let word = if total == 1 { "reference" } else { "references" };
    let file_word = if files.len() == 1 { "file" } else { "files" };
    format!(
        "Found {} {} across {} {}.",
        total, word, files.len(), file_word
    )
}
```

**Step 4: Add `format_for_user` override to `FindReferences` in `symbol.rs`**

Find the `FindReferences` `impl Tool` block (around line 838). It currently ends without a `format_for_user` override. Add it before the closing `}`:

```rust
fn format_for_user(&self, result: &Value) -> Option<String> {
    Some(user_format::format_find_references(result))
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test find_references -- --nocapture`
Expected: PASS

Run: `cargo test`
Expected: all tests pass

**Step 6: Commit**

```bash
git add src/tools/user_format.rs src/tools/symbol.rs
git commit -m "feat: add format_for_user to FindReferences for compact buffer summaries"
```

---

### Task 5: Update server instructions for `@tool_xxx`

**Files:**
- Modify: `src/prompts/server_instructions.md`

Find the section that documents `@cmd_xxx` and `@file_xxx` buffer refs (search for `@cmd_` in the file). Add `@tool_xxx` documentation alongside the existing buffer ref docs.

**Step 1: Find the existing buffer ref documentation**

Run: `grep -n "@cmd_\|@file_\|OutputBuffer\|buffer ref" src/prompts/server_instructions.md`

This will show where to insert the `@tool_xxx` docs. The new paragraph should say something like:

> **`@tool_xxx`** — large tool responses (> 10 KB). Query with `run_command("jq '.field' @tool_abc12345")` or `run_command("grep pattern @tool_abc12345")`. Stored as compact JSON (not pretty-printed). No `.err` suffix variant.

**Step 2: Add the documentation** adjacent to the existing buffer ref docs.

**Step 3: Verify the file looks right**

Run: `grep -A3 "@tool_" src/prompts/server_instructions.md`

**Step 4: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs: document @tool_xxx buffer refs in server instructions"
```

---

### Task 6: Final verification

**Step 1: Run full test suite**

Run: `cargo test`
Expected: all tests pass (previously ~738; should be ~748+ with new tests)

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: no warnings

**Step 3: Run formatter**

Run: `cargo fmt`
Then check: `git diff` — should show no changes (formatter was already applied)

**Step 4: Manual smoke test**

Run the MCP server against this repo and call `list_symbols("src/tools/symbol.rs")` — which previously returned ~13k tokens. Verify the response is now compact with a `@tool_xxx` ref handle.

**Step 5: Final commit if needed**

If any formatting changes after `cargo fmt`:
```bash
git add -p
git commit -m "style: cargo fmt after tool output buffering"
```
