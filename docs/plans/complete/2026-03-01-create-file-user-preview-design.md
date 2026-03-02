# Design: create_file User-Facing Preview

**Date:** 2026-03-01
**Status:** Approved
**Scope:** `create_file` tool only

## Problem

`create_file` returns `json!("ok")` to the LLM — correct, since the LLM generated the content and needs no echo. But the user sees only `"ok"` in the Claude Code tool result panel, which is opaque and unhelpful.

## Goal

Show the user a rich markdown header + preview of the created file without adding a single token to the LLM's context.

## Output Format

**Short file (≤ 30 lines):**
```
**✓ Created** `src/tools/file.rs` — Rust · 42 lines

```rust
pub struct CreateFile;
...
```
```

**Long file (> 30 lines):**
```
**✓ Created** `src/tools/file.rs` — Rust · 312 lines

```rust
pub struct CreateFile;
...
*(showing 30 of 312 lines)*
```
```

**Unknown extension (no language match):**
```
**✓ Created** `.env.example` · 5 lines
```

Language label and fence tag come from the existing `detect_language()` helper
(e.g. `"rust"`, `"markdown"`, `"python"`). If it returns `None`, no fence is emitted.

## Architecture

### New `call_content()` method on Tool trait

```rust
// src/tools/mod.rs
#[async_trait]
pub trait Tool: Send + Sync {
    // ... existing methods unchanged ...

    /// Returns MCP content blocks for this tool call.
    /// Default: delegates to `call()`, wraps result as plain text (no audience tag).
    /// Override to return audience-split blocks (user-only preview, LLM-only ack).
    async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
        let val = self.call(input, ctx).await?;
        Ok(vec![Content::text(serde_json::to_string_pretty(&val)?)])
    }
}
```

`server.rs::call_tool` switches from calling `call()` to `call_content()`. All 29 existing tools inherit the default — existing behavior is preserved exactly. Error routing (`route_tool_error`) is unchanged.

### CreateFile overrides `call_content()`

```rust
// src/tools/file.rs — CreateFile
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    // write the file (same logic as before)
    super::guard_worktree_write(ctx).await?;
    let path = super::require_str_param(&input, "path")?;
    let content = super::require_str_param(&input, "content")?;
    let root = ctx.agent.require_project_root().await?;
    let security = ctx.agent.security_config().await;
    let resolved = validate_write_path(path, &root, &security)?;
    write_utf8(&resolved, content)?;
    ctx.lsp.notify_file_changed(&resolved).await;

    let lang = crate::ast::detect_language(&resolved);
    let line_count = content.lines().count();
    let user_md = render_create_header(&resolved, lang, line_count, content);

    Ok(vec![
        Content::text("ok").with_audience(vec![Role::Assistant]),
        Content::text(user_md).with_audience(vec![Role::User]),
    ])
}
```

### `render_create_header()` free function

```rust
fn render_create_header(path: &Path, lang: Option<&str>, lines: usize, content: &str) -> String {
    let display = path.display();
    let lang_label = lang.map(|l| format!(" — {}", capitalize(l))).unwrap_or_default();
    let mut out = format!("**✓ Created** `{display}`{lang_label} · {lines} lines");

    if let Some(fence_lang) = lang {
        const PREVIEW_LINES: usize = 30;
        let preview: String = content.lines().take(PREVIEW_LINES).collect::<Vec<_>>().join("\n");
        out.push_str(&format!("\n\n```{fence_lang}\n{preview}\n```"));
        if lines > PREVIEW_LINES {
            out.push_str(&format!("\n*(showing {PREVIEW_LINES} of {lines} lines)*"));
        }
    }
    out
}
```

## Key Constraints

- **LLM sees nothing new.** `audience: [assistant]` block is just `"ok"`.
- **Preview is user-only.** `audience: [user]` block never enters the LLM's context.
- **No new dependencies.** Uses existing `detect_language()`, existing string ops.
- **Blast radius: zero.** All other tools inherit the default `call_content()` and are unchanged.
- **`call()` still exists** on `CreateFile` (removed — `call_content()` replaces it fully). The `call()` default on the trait can remain as a convenience for tools that don't need audience split.

## Risk: Audience Annotations in Claude Code

If the Claude Code MCP client does not honor `audience: [user]`, the user block is silently dropped and they see only `"ok"` — same as current state, no regression.

Mitigation: unit-test that `call_content()` emits two `Content` blocks with the expected audience tags. E2e validation of rendering requires manual smoke testing after implementation.

## Out of Scope

- `edit_lines`, `replace_symbol`, `insert_code` — these return location info new to the LLM; not touched.
- Syntax highlighting beyond the fenced markdown block (no ANSI, no HTML).
- Configurable preview line count (hardcoded 30 for now).
