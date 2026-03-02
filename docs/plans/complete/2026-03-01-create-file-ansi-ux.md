# create_file ANSI UX Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `create_file`'s flat markdown output with ANSI-colored, line-numbered preview — and drop the redundant "ok" assistant block.

**Architecture:** Add `pub fn format_create_file` to `user_format.rs` (alongside the existing `render_diff_header` / `render_edit_diff` helpers). Update `CreateFile::call_content` to call it and return a single user-audience block. Delete the now-dead `render_create_header` function.

**Tech Stack:** Rust · existing ANSI constants in `user_format.rs` · `rmcp::model::{Content, Role}`

---

### Task 1: Update the test first (TDD)

**Files:**
- Modify: `src/tools/file.rs` — test at L2217

The existing test `create_file_call_content_returns_two_audience_blocks` asserts 2 blocks and an "ok" in block 0. Rewrite it to match the new contract: 1 block, user-only audience, ANSI header present, filename present.

**Step 1: Replace the test**

Find test fn `create_file_call_content_returns_two_audience_blocks` (L2217) and replace its body with:

```rust
async fn create_file_call_content_returns_user_only_block() {
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

    assert_eq!(blocks.len(), 1, "expected exactly one content block");

    let block = &blocks[0];
    assert_eq!(
        block.audience(),
        Some(&vec![Role::User]),
        "block must be user-only"
    );
    let text = format!("{:?}", block);
    assert!(text.contains("demo.rs"), "block must mention filename");
    // ANSI header produced by render_diff_header starts with ESC[
    assert!(text.contains("\\u{1b}["), "block must contain ANSI codes");
}
```

**Step 2: Run to verify it fails**

```bash
cargo test create_file_call_content 2>&1 | tail -20
```

Expected: FAIL — still 2 blocks, "ok" present.

**Step 3: Commit the failing test**

```bash
git add src/tools/file.rs
git commit -m "test(create_file): expect single user-only ANSI block"
```

---

### Task 2: Add `format_create_file` to `user_format.rs`

**Files:**
- Modify: `src/tools/user_format.rs` — append before the `#[cfg(test)]` line (~L1360)

The function reuses the existing private `render_diff_header`, `DIM`, `RESET` constants already in scope.

**Step 1: Add the function** — insert before `#[cfg(test)]` in `user_format.rs`:

```rust
/// ANSI-formatted header + 5-line numbered preview for create_file output.
pub fn format_create_file(path: &std::path::Path, line_count: usize, content: &str) -> String {
    const PREVIEW_LINES: usize = 5;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)");

    let mut out = render_diff_header("create_file", filename);
    out.push('\n');

    let lines: Vec<&str> = content.lines().take(PREVIEW_LINES).collect();
    let lineno_width = line_count.to_string().len().max(1);
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            "{DIM}{:>width$}{RESET}| {line}\n",
            i + 1,
            width = lineno_width
        ));
    }
    if line_count > PREVIEW_LINES {
        let remaining = line_count - PREVIEW_LINES;
        out.push_str(&format!("{DIM}···  ({remaining} more lines){RESET}\n"));
    }
    out
}
```

**Step 2: Run tests (should still fail on the create_file test)**

```bash
cargo test create_file_call_content 2>&1 | tail -10
```

Expected: still FAIL — `call_content` not updated yet.

---

### Task 3: Update `CreateFile::call_content` + delete dead code

**Files:**
- Modify: `src/tools/file.rs` — `CreateFile::call_content` (L486) and `render_create_header` (L742)

**Step 1: Replace `call_content` body**

New body for `CreateFile::call_content` (L486–509):

```rust
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    super::guard_worktree_write(ctx).await?;
    let path_str = super::require_str_param(&input, "path")?;
    let content = super::require_str_param(&input, "content")?;
    let root = ctx.agent.require_project_root().await?;
    let security = ctx.agent.security_config().await;
    let resolved =
        crate::util::path_security::validate_write_path(path_str, &root, &security)?;
    crate::util::fs::write_utf8(&resolved, content)?;
    ctx.lsp.notify_file_changed(&resolved).await;

    let line_count = content.lines().count();
    let user_md = super::user_format::format_create_file(&resolved, line_count, content);

    Ok(vec![Content::text(user_md).with_audience(vec![Role::User])])
}
```

**Step 2: Delete `render_create_header`**

Remove the entire `fn render_create_header(...)` (L742–770). It is no longer called.

**Step 3: Run the target test**

```bash
cargo test create_file_call_content 2>&1 | tail -10
```

Expected: PASS.

**Step 4: Run full test suite + lint**

```bash
cargo test 2>&1 | tail -20
cargo clippy -- -D warnings 2>&1 | tail -20
```

Expected: all green.

**Step 5: Commit**

```bash
git add src/tools/file.rs src/tools/user_format.rs
git commit -m "feat(create_file): ANSI line-numbered preview, drop redundant ok block"
```

---

### Task 4: Manual smoke test

**Step 1: Start the server and call create_file**

```bash
cargo run -- start --project . 2>/dev/null &
```

Or just verify via an existing Claude Code session: run `create_file` on any small file and confirm the terminal shows:
- A bold-cyan `─── create_file: <filename> ───` header
- Dim line numbers with `|` separator
- `···  (N more lines)` if file > 5 lines
- No separate `⎿  ok` block

**Step 2: Remove the temp notes.md created during brainstorming**

```bash
git rm notes.md
git commit -m "chore: remove temporary notes.md"
```
