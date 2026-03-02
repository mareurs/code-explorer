# Dual-Audience Tool Output

**Date:** 2026-03-01
**Status:** Design

## Problem

All 30+ tools return pretty-printed JSON via `serde_json::to_string_pretty()` in
a single `Content::text()` block. This JSON is consumed by two audiences with
different needs:

1. **The LLM** — needs structured, parseable data. JSON works fine but
   pretty-printing wastes ~30% tokens on whitespace.
2. **The user** (Ctrl+O expansion in Claude Code) — sees raw JSON, which is
   functional but hard to scan. Claude Code does NOT render markdown in tool
   results ([#13600](https://github.com/anthropics/claude-code/issues/13600)),
   so the output appears as monospace plain text.

One tool (`create_file`) already uses dual-audience blocks successfully: `"ok"`
for the LLM, a formatted preview for the user.

## Solution

Extend the `Tool` trait with an optional `format_for_user()` method. When
implemented, `call_content()` emits two MCP content blocks:

1. **Compact JSON** with `audience: ["assistant"]` — for the LLM
2. **Formatted plain text** with `audience: ["user"]` — for Ctrl+O expansion

### Trait Change

```rust
// src/tools/mod.rs — add to Tool trait

/// Optional human-readable formatting for the tool result.
/// When Some, call_content() emits two content blocks:
///   1. Compact JSON (audience: assistant)
///   2. Formatted plain text (audience: user)
fn format_for_user(&self, result: &Value) -> Option<String> {
    None
}
```

### Default `call_content()` Change

```rust
async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
    let val = self.call(input, ctx).await?;
    match self.format_for_user(&val) {
        Some(user_text) => {
            // Compact JSON for LLM (saves ~30% tokens vs pretty-print)
            let json = serde_json::to_string(&val)
                .unwrap_or_else(|_| val.to_string());
            Ok(vec![
                Content::text(json).with_audience(vec![Role::Assistant]),
                Content::text(user_text).with_audience(vec![Role::User]),
            ])
        }
        None => {
            // Legacy: single pretty-printed block for both audiences
            Ok(vec![Content::text(
                serde_json::to_string_pretty(&val)
                    .unwrap_or_else(|_| val.to_string()),
            )])
        }
    }
}
```

### Tools That Override `call_content()` Directly

`CreateFile` already overrides `call_content()` and will continue to do so
(write tools return `"ok"`, not the full call result). No change needed there.

## Scope: Read-Heavy Tools (Phase 1)

Eight tools that produce the richest, most-viewed output:

| Tool | Category | Why it benefits |
|------|----------|----------------|
| `list_dir` | file | Directory listings are scanned visually |
| `list_symbols` | symbol | Symbol trees are the primary navigation output |
| `find_symbol` | symbol | Search results need quick visual scanning |
| `search_pattern` | file | grep-like output is a well-known format |
| `read_file` | file | File content/summaries are viewed frequently |
| `semantic_search` | semantic | Ranked results benefit from alignment |
| `hover` | symbol | LSP markdown is currently escaped in JSON |
| `goto_definition` | symbol | Simple output that benefits from clean formatting |

## Per-Tool Formatting

### 1. `list_dir`

```
src/tools/ — 12 entries

  ast.rs           config.rs        file.rs
  file_summary.rs  git.rs           library.rs
  memory.rs        mod.rs           output.rs
  semantic.rs      symbol.rs        workflow.rs
```

- Multi-column layout for files (3 columns, auto-width)
- Directories get trailing `/`
- Overflow hint at bottom when capped

### 2. `list_symbols`

```
src/tools/output.rs — 8 symbols

  enum   OutputMode                L10-15
           Exploring               L11
           Focused                 L12
  struct OutputGuard               L35-50
           fn new                  L40
           fn cap_items            L55-80
           fn cap_files            L82-100
  fn     overflow_json             L90-120
```

- Indented tree with kind, name, and line range aligned
- Children indented under parent
- Directory mode: file headers with symbol trees below

### 3. `find_symbol`

```
3 matches for "OutputGuard"

  struct  src/tools/output.rs:35       OutputGuard
  fn      src/tools/output.rs:55       OutputGuard::cap_items
  use     src/server.rs:120            OutputGuard
```

With `include_body=true`:
```
1 match for "cap_items"

  fn  src/tools/output.rs:55-80  OutputGuard::cap_items

      pub fn cap_items(&self, items: &mut Vec<Value>) -> Option<OverflowInfo> {
          let max = self.max_items();
          if items.len() <= max { return None; }
          ...
      }
```

- Kind, file:line, and name_path aligned
- Body indented below when present
- Overflow shows `by_file` distribution as compact list

### 4. `search_pattern`

```
5 matches for /RecoverableError/

  src/tools/mod.rs:54       pub struct RecoverableError {
  src/tools/mod.rs:60           RecoverableError { error, hint }
  src/server.rs:230         RecoverableError => {
  src/server.rs:235             let re = &err.downcast_ref::<RecoverableError>();
  src/tools/symbol.rs:412   use super::RecoverableError;
```

With context lines:
```
  src/tools/mod.rs
  53-  /// Soft error with actionable hint
  54:  pub struct RecoverableError {
  55-      pub error: String,
  56-      pub hint: String,
  57-  }
```

- grep/rg-style output (familiar to developers)
- Context mode groups matches by file with `-`/`:` line markers
- File:line left-aligned, content follows

### 5. `read_file`

Summary mode (>200 lines):
```
src/tools/output.rs — 245 lines (Rust)

  Symbols:
    struct OutputMode          L10
    struct OutputGuard         L35
    fn cap_items               L55
    fn cap_files               L82
    fn overflow_json           L90

  Buffer: @file_abc123
  Hint: use list_symbols for full tree, or start_line/end_line for excerpt
```

Content mode (small files / ranged reads):
```
src/tools/output.rs:35-50

  35│ pub struct OutputGuard {
  36│     mode: OutputMode,
  37│     max_items: usize,
  38│ }
  39│
  40│ impl OutputGuard {
```

- Summary mode shows symbol outline + buffer ref
- Content mode shows line-numbered source (like `cat -n`)
- Markdown files show heading outline in summary

### 6. `semantic_search`

```
3 results for "progressive disclosure"

  0.92  src/tools/output.rs:35-50        OutputGuard struct
  0.87  docs/PROGRESSIVE_DIS…:1-30      Design guide
  0.81  src/tools/mod.rs:120-140         Tool trait overflow

  ⚠ Index is 2 commits behind HEAD — run index_project to refresh
```

- Score, file:range, and description aligned
- Long paths truncated with `…`
- Staleness warning at bottom when applicable

### 7. `hover`

```
src/tools/output.rs:35 — OutputGuard

  pub struct OutputGuard {
      mode: OutputMode,
      max_items: usize,
  }

  Progressive disclosure guard that caps output
  based on the current mode (exploring/focused).
```

- Pass through LSP hover content directly (strip markdown fences, keep text)
- Location header, then signature, then documentation
- No JSON escaping of the markdown content

### 8. `goto_definition`

```
src/tools/output.rs:35

  pub struct OutputGuard {
```

Multiple definitions:
```
2 definitions for "cap_items"

  src/tools/output.rs:55      pub fn cap_items(&self, ...) -> Option<OverflowInfo>
  src/tools/output.rs:200     // test impl
```

- Minimal: just location + context line
- Multiple definitions listed with context

## Formatting Module

All formatting functions live in a new module: `src/tools/user_format.rs`.

```rust
// Public API — one function per tool, takes the JSON Value, returns String

pub fn format_list_dir(val: &Value) -> String { ... }
pub fn format_list_symbols(val: &Value) -> String { ... }
pub fn format_find_symbol(val: &Value) -> String { ... }
pub fn format_search_pattern(val: &Value) -> String { ... }
pub fn format_read_file(val: &Value) -> String { ... }
pub fn format_semantic_search(val: &Value) -> String { ... }
pub fn format_hover(val: &Value) -> String { ... }
pub fn format_goto_definition(val: &Value) -> String { ... }

// Internal helpers
fn align_columns(rows: &[(String, String, String)]) -> String { ... }
fn indent_tree(items: &[TreeItem], depth: usize) -> String { ... }
fn truncate_path(path: &str, max_len: usize) -> String { ... }
fn format_line_range(start: u64, end: u64) -> String { ... }
fn format_overflow(overflow: &Value) -> String { ... }
```

Each tool's `format_for_user()` delegates to the corresponding function:

```rust
impl Tool for ListDir {
    fn format_for_user(&self, val: &Value) -> Option<String> {
        Some(user_format::format_list_dir(val))
    }
}
```

## Overflow in User View

When overflow metadata is present in the JSON result, append it to the user
view as a compact hint line:

```
  … showing 50 of 234 — narrow with path= or pattern=
```

This replaces the full `overflow` JSON object in the user view.

## Error Formatting

`RecoverableError` results don't go through `format_for_user()` — they are
handled in `route_tool_error()` in `server.rs`. These could get dual-audience
treatment in a future phase.

## Testing Strategy

Each `format_*` function gets unit tests with:
1. **Basic output** — typical result JSON → expected formatted string
2. **Empty results** — empty arrays/no matches
3. **Overflow** — results with overflow metadata
4. **Edge cases** — very long paths, missing optional fields, unicode

Tests live in `src/tools/user_format.rs` as `#[cfg(test)] mod tests`.

## Implementation Order

1. Add `format_for_user()` to `Tool` trait + update default `call_content()`
2. Create `src/tools/user_format.rs` with internal helpers
3. Implement formatters one at a time, easiest first:
   - `goto_definition` (simplest output)
   - `hover` (mostly pass-through)
   - `list_dir` (simple list)
   - `search_pattern` (grep-style, familiar format)
   - `find_symbol` (aligned columns)
   - `list_symbols` (indented tree)
   - `semantic_search` (aligned + staleness)
   - `read_file` (two modes: summary + content)
4. Add tests for each formatter
5. Manual verification in Claude Code (Ctrl+O to inspect)

## Future Phases

- **Phase 2:** Write tools (`edit_file`, `replace_symbol`, etc.) — show diff-like
  output to user
- **Phase 3:** Workflow tools (`run_command`, `onboarding`) — structured summaries
- **Phase 4:** RecoverableError dual-audience formatting
- **Phase 5:** Remaining tools (memory, library, config, AST)

## Non-Goals

- Markdown rendering in Claude Code (that's a Claude Code feature request, not ours)
- ANSI color codes (MCP text content doesn't support escape sequences)
- Changing the JSON structure sent to the LLM (only switching from pretty to compact)
