# create_file User-Facing Preview Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `create_file` return a markdown header + fenced code preview to the human user while keeping the LLM response as bare `"ok"`, using MCP audience annotations.

**Architecture:** Add an optional `call_content() -> Result<Vec<Content>>` method to the `Tool` trait with a default that preserves all existing tool behavior. Override it on `CreateFile` to emit two audience-split content blocks. Switch `server.rs` to dispatch via `call_content()`.

**Tech Stack:** Rust, rmcp 0.1.5 (`Content`, `Role` from `rmcp::model`), existing `crate::ast::detect_language`.

---

### Task 1: Add `call_content()` to Tool trait

**Files:**
- Modify: `src/tools/mod.rs:164-176`

The current trait (lines 164–176) is:
```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;
}
```

**Step 1: Add the import for `Content`**

At line 23 in `src/tools/mod.rs` (after `use serde_json::Value;`), add:
```rust
use rmcp::model::Content;
```

**Step 2: Add the default method to the trait**

Replace the trait body so it reads:
```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;

    /// Returns MCP content blocks for this tool call.
    ///
    /// Default: delegates to `call()` and wraps the JSON value as plain text
    /// with no audience annotation — shown to both the LLM and the user.
    /// Override to return audience-split blocks (e.g. user-only preview).
    async fn call_content(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Vec<Content>> {
        let val = self.call(input, ctx).await?;
        Ok(vec![Content::text(serde_json::to_string_pretty(&val)?)])
    }
}
```

**Step 3: Build to confirm it compiles**

```bash
cargo build 2>&1 | head -20
```
Expected: no errors.

**Step 4: Commit**

```bash
git add src/tools/mod.rs
git commit -m "feat(tools): add call_content() to Tool trait with default impl"
```

---

### Task 2: Switch `server.rs` to use `call_content()`

**Files:**
- Modify: `src/server.rs:153-220` (the `call_tool` method)

**Step 1: Find where `call()` is dispatched**

The relevant block in `call_tool` (around lines 200–220) looks like:
```rust
match tool.call(input, &ctx).await {
    Ok(val) => {
        let text = serde_json::to_string_pretty(&val)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
    Err(e) => Ok(route_tool_error(e)),
}
```

**Step 2: Replace with `call_content()`**

Change that block to:
```rust
match tool.call_content(input, &ctx).await {
    Ok(blocks) => Ok(CallToolResult::success(blocks)),
    Err(e) => Ok(route_tool_error(e)),
}
```

Note: The `serde_json::to_string_pretty` call is now gone — that responsibility moved into the default `call_content()` implementation.

**Step 3: Run full test suite**

```bash
cargo test 2>&1 | tail -10
```
Expected: all existing tests pass (the default `call_content()` produces identical output to the old code).

**Step 4: Commit**

```bash
git add src/server.rs
git commit -m "feat(server): dispatch via call_content() instead of call()"
```

---

### Task 3: Write `render_create_header()` with tests (TDD)

**Files:**
- Modify: `src/tools/file.rs` — add helper function and its unit tests

This is a pure string-building function. Write the tests first.

**Step 1: Write the failing tests**

Add this test module section to the `tests` module inside `src/tools/file.rs` (after the last existing test):

```rust
// ── render_create_header ─────────────────────────────────────────────────

#[test]
fn render_header_known_lang_short_file() {
    use std::path::Path;
    let content = "fn main() {}\nlet x = 1;\n";
    let result = render_create_header(Path::new("src/main.rs"), Some("rust"), 2, content);
    assert!(result.starts_with("**✓ Created** `src/main.rs`"), "header: {result}");
    assert!(result.contains("Rust"), "lang label: {result}");
    assert!(result.contains("2 lines"), "line count: {result}");
    assert!(result.contains("```rust"), "fence: {result}");
    assert!(result.contains("fn main()"), "content: {result}");
    assert!(!result.contains("showing"), "no truncation note for short file: {result}");
}

#[test]
fn render_header_long_file_truncates() {
    use std::path::Path;
    let content = (0..50).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let result = render_create_header(Path::new("src/lib.rs"), Some("rust"), 50, &content);
    assert!(result.contains("50 lines"), "line count: {result}");
    assert!(result.contains("showing 30 of 50"), "truncation note: {result}");
    // Only 30 lines in the fence
    let fence_content = result.split("```rust").nth(1).unwrap_or("");
    assert_eq!(fence_content.lines().filter(|l| l.starts_with("line ")).count(), 30);
}

#[test]
fn render_header_unknown_lang_no_fence() {
    use std::path::Path;
    let content = "KEY=value\n";
    let result = render_create_header(Path::new(".env.example"), None, 1, content);
    assert!(result.starts_with("**✓ Created** `.env.example`"), "header: {result}");
    assert!(result.contains("1 lines"), "line count: {result}");
    assert!(!result.contains("```"), "no fence for unknown lang: {result}");
}

#[test]
fn render_header_markdown_file() {
    use std::path::Path;
    let content = "# Hello\n\nWorld\n";
    let result = render_create_header(Path::new("README.md"), Some("markdown"), 3, content);
    assert!(result.contains("Markdown"), "lang label: {result}");
    assert!(result.contains("```markdown"), "fence: {result}");
}
```

**Step 2: Run the tests to confirm they fail**

```bash
cargo test render_header 2>&1 | head -20
```
Expected: compile error — `render_create_header` not defined yet.

**Step 3: Implement `render_create_header()`**

Add this free function in `src/tools/file.rs`, before the `// ── tests` module:

```rust
/// Build the user-facing markdown string for a created file.
/// `lang` is the language name from `detect_language()` (e.g. `"rust"`, `"markdown"`).
/// Pass `None` when the extension is unknown — no fenced block will be emitted.
fn render_create_header(path: &std::path::Path, lang: Option<&str>, line_count: usize, content: &str) -> String {
    const PREVIEW_LINES: usize = 30;

    let display = path.display();
    let lang_label = lang
        .map(|l| {
            let mut s = l.to_string();
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            format!(" — {s}")
        })
        .unwrap_or_default();

    let mut out = format!("**✓ Created** `{display}`{lang_label} · {line_count} lines");

    if let Some(fence_lang) = lang {
        let lines: Vec<&str> = content.lines().take(PREVIEW_LINES).collect();
        let preview = lines.join("\n");
        out.push_str(&format!("\n\n```{fence_lang}\n{preview}\n```"));
        if line_count > PREVIEW_LINES {
            out.push_str(&format!("\n*(showing {PREVIEW_LINES} of {line_count} lines)*"));
        }
    }

    out
}
```

**Step 4: Run the tests to confirm they pass**

```bash
cargo test render_header 2>&1
```
Expected: 4 tests pass.

**Step 5: Commit**

```bash
git add src/tools/file.rs
git commit -m "feat(create_file): add render_create_header() with unit tests"
```

---

### Task 4: Override `call_content()` on `CreateFile`

**Files:**
- Modify: `src/tools/file.rs` — `impl Tool for CreateFile` block (lines 414–445)

**Step 1: Write the failing test**

Add to the `tests` module in `src/tools/file.rs`:

```rust
// ── CreateFile::call_content audience split ───────────────────────────────

#[tokio::test]
async fn create_file_call_content_returns_two_audience_blocks() {
    use rmcp::model::Role;
    let (dir, ctx) = project_ctx().await;
    let file = dir.path().join("demo.rs");

    let blocks = CreateFile
        .call_content(
            json!({
                "path": file.to_str().unwrap(),
                "content": "fn main() {}\n"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(blocks.len(), 2, "expected two content blocks");

    // Block 0: LLM-only "ok"
    let llm_block = &blocks[0];
    assert_eq!(
        llm_block.audience(),
        Some(&vec![Role::Assistant]),
        "first block must be assistant-only"
    );
    // The text content of the annotated block:
    assert!(
        format!("{:?}", llm_block).contains("ok"),
        "LLM block must contain 'ok'"
    );

    // Block 1: user-only markdown header
    let user_block = &blocks[1];
    assert_eq!(
        user_block.audience(),
        Some(&vec![Role::User]),
        "second block must be user-only"
    );
    let user_text = format!("{:?}", user_block);
    assert!(user_text.contains("Created"), "user block must have header");
    assert!(user_text.contains("demo.rs"), "user block must mention filename");
}
```

**Step 2: Run the test to confirm it fails**

```bash
cargo test create_file_call_content_returns_two_audience_blocks 2>&1 | head -20
```
Expected: compile error or test failure — `call_content` on `CreateFile` still uses the default which returns a single block.

**Step 3: Add imports to `src/tools/file.rs`**

At the top of `src/tools/file.rs`, add:
```rust
use rmcp::model::{Content, Role};
```

**Step 4: Add `call_content()` override to `impl Tool for CreateFile`**

In `src/tools/file.rs`, inside the `impl Tool for CreateFile` block, add `call_content()` after the existing `call()` method. The existing `call()` remains untouched — it still writes the file and returns `json!("ok")` for direct test coverage.

The new `call_content()` duplicates the write logic briefly (see note below):

```rust
async fn call_content(
    &self,
    input: Value,
    ctx: &ToolContext,
) -> Result<Vec<Content>> {
    // Write the file (same guards as call())
    super::guard_worktree_write(ctx).await?;
    let path_str = super::require_str_param(&input, "path")?;
    let content = super::require_str_param(&input, "content")?;
    let root = ctx.agent.require_project_root().await?;
    let security = ctx.agent.security_config().await;
    let resolved = crate::util::path_security::validate_write_path(path_str, &root, &security)?;
    crate::util::fs::write_utf8(&resolved, content)?;
    ctx.lsp.notify_file_changed(&resolved).await;

    // Build user-facing preview
    let lang = crate::ast::detect_language(&resolved);
    let line_count = content.lines().count();
    let user_md = render_create_header(&resolved, lang, line_count, content);

    Ok(vec![
        Content::text("ok").with_audience(vec![Role::Assistant]),
        Content::text(user_md).with_audience(vec![Role::User]),
    ])
}
```

Note: The write logic is intentionally duplicated from `call()` rather than extracted into a helper. The existing `call()` tests provide coverage for validation/error paths; `call_content()` tests cover the happy-path audience split. YAGNI — no helper until a third call site exists.

**Step 5: Run the new test**

```bash
cargo test create_file_call_content_returns_two_audience_blocks 2>&1
```
Expected: PASS.

**Step 6: Run the full test suite**

```bash
cargo test 2>&1 | tail -10
```
Expected: all tests pass.

**Step 7: Commit**

```bash
git add src/tools/file.rs
git commit -m "feat(create_file): override call_content() with audience-split markdown preview"
```

---

### Task 5: Final cleanup and verification

**Step 1: Format and lint**

```bash
cargo fmt && cargo clippy -- -D warnings 2>&1 | tail -10
```
Expected: clean.

**Step 2: Full test run with count**

```bash
cargo test 2>&1 | grep "test result"
```
Expected: all pass, count increased by ~5 (new render_header tests + call_content test).

**Step 3: Verify server.rs still compiles cleanly**

```bash
cargo build --release 2>&1 | tail -5
```
Expected: no warnings.

**Step 4: Manual smoke note**

After deploying the server, create a test file and check Claude Code's tool result panel to confirm:
- The panel shows the markdown header and preview (audience: user block rendered)
- The LLM response in context is just `ok` (inspect with debug logging if needed)

If the user block is invisible (Claude Code not honoring audience tags), the result is `ok` — same as before, no regression.

**Step 5: Commit any fmt/clippy fixes**

```bash
git add -p
git commit -m "style: fmt and clippy fixes for create_file preview"
```
