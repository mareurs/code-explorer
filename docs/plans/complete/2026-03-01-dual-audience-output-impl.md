# Dual-Audience Tool Output Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add user-facing plain text formatting to 8 read-heavy tools, sending compact JSON to the LLM and formatted text to the user via MCP audience annotations.

**Architecture:** Add `format_for_user(&Value) -> Option<String>` to the `Tool` trait. When it returns `Some`, the default `call_content()` emits two `Content` blocks with audience annotations. All formatting lives in a new `src/tools/user_format.rs` module.

**Tech Stack:** Rust, rmcp (MCP library), serde_json

**Design doc:** `docs/plans/2026-03-01-dual-audience-output-design.md`

---

### Task 1: Add `format_for_user` to `Tool` trait and update `call_content`

**Files:**
- Modify: `src/tools/mod.rs:168-192` (Tool trait + default call_content)

**Step 1: Add the trait method**

In `src/tools/mod.rs`, add `Role` to the existing `Content` import on line 25:

```rust
use rmcp::model::{Content, Role};
```

Then add this method to the `Tool` trait (after `call`, before `call_content`):

```rust
    /// Optional human-readable formatting for the tool result.
    /// When Some, call_content() emits dual-audience blocks:
    ///   1. Compact JSON (audience: assistant)
    ///   2. Formatted plain text (audience: user)
    fn format_for_user(&self, _result: &Value) -> Option<String> {
        None
    }
```

**Step 2: Update default `call_content`**

Replace the existing `call_content` body with:

```rust
    async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
        let val = self.call(input, ctx).await?;
        match self.format_for_user(&val) {
            Some(user_text) => {
                let json = serde_json::to_string(&val)
                    .unwrap_or_else(|_| val.to_string());
                Ok(vec![
                    Content::text(json).with_audience(vec![Role::Assistant]),
                    Content::text(user_text).with_audience(vec![Role::User]),
                ])
            }
            None => {
                Ok(vec![Content::text(
                    serde_json::to_string_pretty(&val)
                        .unwrap_or_else(|_| val.to_string()),
                )])
            }
        }
    }
```

**Step 3: Verify it compiles and all tests pass**

Run: `cargo build && cargo test`
Expected: All 533+ tests pass (no behavior change — all tools return `None` from `format_for_user`).

**Step 4: Commit**

```bash
git add src/tools/mod.rs
git commit -m "feat(tools): add format_for_user trait method for dual-audience output"
```

---

### Task 2: Create `user_format.rs` module with internal helpers

**Files:**
- Create: `src/tools/user_format.rs`
- Modify: `src/tools/mod.rs:1-20` (add `pub mod user_format;`)

**Step 1: Write tests for the helper functions**

Create `src/tools/user_format.rs` with the module skeleton and tests for helpers:

```rust
//! User-facing formatting for tool results.
//!
//! Each public `format_*` function takes the JSON `Value` returned by
//! a tool's `call()` and produces a compact, human-readable plain-text
//! representation for the user's Ctrl+O expansion in Claude Code.

use serde_json::Value;

/// Format a line range like "L35-50" or "L35" if start == end.
fn format_line_range(start: u64, end: u64) -> String {
    if start == end || end == 0 {
        format!("L{start}")
    } else {
        format!("L{start}-{end}")
    }
}

/// Truncate a path to max_len chars, replacing the middle with "…".
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    if max_len < 5 {
        return path[..max_len].to_string();
    }
    // Keep the last segment (filename) and as much prefix as fits
    let keep_end = max_len / 2;
    let keep_start = max_len - keep_end - 1; // 1 for "…"
    format!(
        "{}…{}",
        &path[..keep_start],
        &path[path.len() - keep_end..]
    )
}

/// Format an overflow hint as a compact one-liner.
fn format_overflow(overflow: &Value) -> String {
    let shown = overflow["shown"].as_u64().unwrap_or(0);
    let total = overflow["total"].as_u64().unwrap_or(0);
    let hint = overflow["hint"].as_str().unwrap_or("");
    if total > 0 {
        format!("  … showing {shown} of {total} — {hint}")
    } else {
        format!("  … showing first {shown} — {hint}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_range_single() {
        assert_eq!(format_line_range(35, 35), "L35");
    }

    #[test]
    fn line_range_span() {
        assert_eq!(format_line_range(35, 50), "L35-50");
    }

    #[test]
    fn line_range_zero_end() {
        assert_eq!(format_line_range(10, 0), "L10");
    }

    #[test]
    fn truncate_short_path() {
        assert_eq!(truncate_path("src/main.rs", 30), "src/main.rs");
    }

    #[test]
    fn truncate_long_path() {
        let long = "src/tools/very/deeply/nested/path/to/file.rs";
        let result = truncate_path(long, 25);
        assert!(result.len() <= 25);
        assert!(result.contains('…'));
    }

    #[test]
    fn overflow_with_total() {
        let ov = serde_json::json!({
            "shown": 50, "total": 234, "hint": "narrow with path="
        });
        let result = format_overflow(&ov);
        assert!(result.contains("50 of 234"));
        assert!(result.contains("narrow with path="));
    }

    #[test]
    fn overflow_without_total() {
        let ov = serde_json::json!({
            "shown": 50, "total": 0, "hint": "use more specific pattern"
        });
        let result = format_overflow(&ov);
        assert!(result.contains("first 50"));
    }
}
```

**Step 2: Register the module**

In `src/tools/mod.rs`, add after the existing module declarations (around line 18):

```rust
pub mod user_format;
```

**Step 3: Run tests**

Run: `cargo test user_format`
Expected: All 7 tests pass.

**Step 4: Commit**

```bash
git add src/tools/user_format.rs src/tools/mod.rs
git commit -m "feat(tools): add user_format module with helper functions"
```

---

### Task 3: Implement `format_goto_definition` (simplest tool)

**Files:**
- Modify: `src/tools/user_format.rs` (add formatter + tests)
- Modify: `src/tools/symbol.rs` (implement `format_for_user` on `GotoDefinition`)

**Step 1: Write failing test**

Add to `src/tools/user_format.rs` tests:

```rust
    #[test]
    fn goto_definition_single() {
        let val = serde_json::json!({
            "definitions": [{
                "file": "src/tools/output.rs",
                "line": 35,
                "end_line": 41,
                "context": "pub struct OutputGuard {",
                "source": "project"
            }],
            "from": "symbol.rs:120"
        });
        let result = format_goto_definition(&val);
        assert!(result.contains("src/tools/output.rs:35"));
        assert!(result.contains("pub struct OutputGuard {"));
    }

    #[test]
    fn goto_definition_multiple() {
        let val = serde_json::json!({
            "definitions": [
                { "file": "src/a.rs", "line": 10, "end_line": 10, "context": "fn foo()", "source": "project" },
                { "file": "src/b.rs", "line": 20, "end_line": 20, "context": "fn foo()", "source": "project" },
            ],
            "from": "main.rs:5"
        });
        let result = format_goto_definition(&val);
        assert!(result.contains("2 definitions"));
        assert!(result.contains("src/a.rs:10"));
        assert!(result.contains("src/b.rs:20"));
    }

    #[test]
    fn goto_definition_external() {
        let val = serde_json::json!({
            "definitions": [{
                "file": "/home/user/.rustup/toolchains/.../core/option.rs",
                "line": 100,
                "end_line": 150,
                "context": "pub enum Option<T> {",
                "source": "external"
            }],
            "from": "main.rs:1"
        });
        let result = format_goto_definition(&val);
        assert!(result.contains("external"));
    }
```

Run: `cargo test format_goto_definition` — Expected: FAIL (function doesn't exist)

**Step 2: Implement the formatter**

Add to `src/tools/user_format.rs` (public function):

```rust
/// Format `goto_definition` results.
///
/// Single definition:
/// ```text
/// src/tools/output.rs:35
///
///   pub struct OutputGuard {
/// ```
///
/// Multiple definitions:
/// ```text
/// 2 definitions
///
///   src/a.rs:10     fn foo()
///   src/b.rs:20     fn foo()
/// ```
pub fn format_goto_definition(val: &Value) -> String {
    let defs = val["definitions"].as_array();
    let defs = match defs {
        Some(d) if !d.is_empty() => d,
        _ => return "No definitions found.".to_string(),
    };

    let mut out = String::new();

    if defs.len() == 1 {
        let d = &defs[0];
        let file = d["file"].as_str().unwrap_or("?");
        let line = d["line"].as_u64().unwrap_or(0);
        let context = d["context"].as_str().unwrap_or("");
        let source = d["source"].as_str().unwrap_or("project");

        out.push_str(&format!("{file}:{line}"));
        if source != "project" {
            out.push_str(&format!(" ({source})"));
        }
        if !context.is_empty() {
            out.push_str(&format!("\n\n  {context}"));
        }
    } else {
        out.push_str(&format!("{} definitions\n", defs.len()));
        // Calculate alignment width from longest file:line
        let labels: Vec<String> = defs
            .iter()
            .map(|d| {
                let file = d["file"].as_str().unwrap_or("?");
                let line = d["line"].as_u64().unwrap_or(0);
                format!("{file}:{line}")
            })
            .collect();
        let max_label = labels.iter().map(|l| l.len()).max().unwrap_or(0);

        for (i, d) in defs.iter().enumerate() {
            let context = d["context"].as_str().unwrap_or("");
            let source = d["source"].as_str().unwrap_or("project");
            let suffix = if source != "project" {
                format!(" ({source})")
            } else {
                String::new()
            };
            out.push_str(&format!(
                "\n  {:<width$}  {context}{suffix}",
                labels[i],
                width = max_label
            ));
        }
    }

    out
}
```

**Step 3: Run tests**

Run: `cargo test format_goto_definition`
Expected: All 3 tests pass.

**Step 4: Wire it into the tool**

In `src/tools/symbol.rs`, add the import near the top:

```rust
use super::user_format;
```

Then add `format_for_user` to the `GotoDefinition` impl (after `input_schema`, before `call`):

```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_goto_definition(result))
    }
```

**Step 5: Build and test**

Run: `cargo build && cargo test`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add src/tools/user_format.rs src/tools/symbol.rs
git commit -m "feat(tools): dual-audience output for goto_definition"
```

---

### Task 4: Implement `format_hover`

**Files:**
- Modify: `src/tools/user_format.rs` (add formatter + tests)
- Modify: `src/tools/symbol.rs` (wire into Hover)

**Step 1: Write failing test**

Add to `user_format.rs` tests:

```rust
    #[test]
    fn hover_basic() {
        let val = serde_json::json!({
            "content": "```rust\npub struct OutputGuard {\n    mode: OutputMode,\n}\n```\n\nProgressive disclosure guard.",
            "location": "output.rs:35"
        });
        let result = format_hover(&val);
        assert!(result.contains("output.rs:35"));
        assert!(result.contains("pub struct OutputGuard"));
        assert!(result.contains("Progressive disclosure guard."));
        // Markdown fences should be stripped
        assert!(!result.contains("```"));
    }

    #[test]
    fn hover_no_fences() {
        let val = serde_json::json!({
            "content": "fn cap_items(&self) -> Option<OverflowInfo>",
            "location": "output.rs:55"
        });
        let result = format_hover(&val);
        assert!(result.contains("output.rs:55"));
        assert!(result.contains("fn cap_items"));
    }
```

**Step 2: Implement**

```rust
/// Format `hover` results — pass through LSP content with markdown fences stripped.
///
/// ```text
/// output.rs:35
///
///   pub struct OutputGuard {
///       mode: OutputMode,
///   }
///
///   Progressive disclosure guard.
/// ```
pub fn format_hover(val: &Value) -> String {
    let location = val["location"].as_str().unwrap_or("?");
    let content = val["content"].as_str().unwrap_or("");

    let mut out = format!("{location}\n");

    // Strip markdown code fences, indent the rest
    let mut in_fence = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            if !in_fence {
                // End of fence — add blank line separator
                out.push('\n');
            }
            continue;
        }
        out.push_str(&format!("\n  {line}"));
    }

    out
}
```

**Step 3: Run tests, wire into `Hover`, build, commit**

Wire in `src/tools/symbol.rs`:
```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_hover(result))
    }
```

Run: `cargo build && cargo test`

```bash
git add src/tools/user_format.rs src/tools/symbol.rs
git commit -m "feat(tools): dual-audience output for hover"
```

---

### Task 5: Implement `format_list_dir`

**Files:**
- Modify: `src/tools/user_format.rs` (add formatter + tests)
- Modify: `src/tools/file.rs` (wire into ListDir)

**Step 1: Write failing test**

```rust
    #[test]
    fn list_dir_basic() {
        let val = serde_json::json!({
            "entries": [
                "src/tools/ast.rs",
                "src/tools/config.rs",
                "src/tools/file.rs",
                "src/tools/mod.rs",
                "src/tools/output.rs",
                "src/tools/subdir/"
            ]
        });
        let result = format_list_dir(&val);
        assert!(result.contains("6 entries"));
        assert!(result.contains("ast.rs"));
        assert!(result.contains("subdir/"));
    }

    #[test]
    fn list_dir_with_overflow() {
        let val = serde_json::json!({
            "entries": ["a.rs", "b.rs"],
            "overflow": { "shown": 2, "total": 150, "hint": "use specific path" }
        });
        let result = format_list_dir(&val);
        assert!(result.contains("2 of 150"));
    }

    #[test]
    fn list_dir_empty() {
        let val = serde_json::json!({ "entries": [] });
        let result = format_list_dir(&val);
        assert!(result.contains("empty") || result.contains("0 entries"));
    }
```

**Step 2: Implement**

```rust
/// Format `list_dir` results as a multi-column listing.
///
/// ```text
/// src/tools/ — 12 entries
///
///   ast.rs           config.rs        file.rs
///   file_summary.rs  git.rs           library.rs
///   memory.rs        mod.rs           output.rs
///   semantic.rs      symbol.rs        workflow.rs
/// ```
pub fn format_list_dir(val: &Value) -> String {
    let entries = match val["entries"].as_array() {
        Some(e) => e,
        None => return "No entries.".to_string(),
    };

    if entries.is_empty() {
        return "0 entries".to_string();
    }

    let count = entries.len();
    // Extract just filenames (last path component)
    let names: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.as_str())
        .map(|path| {
            // For display, show just the last component
            let trimmed = path.trim_end_matches('/');
            let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
            // Re-add trailing / for directories
            if path.ends_with('/') {
                // We'll handle the slash in display
                path.rsplit('/').nth(1).unwrap_or(name)
            } else {
                name
            }
        })
        .collect();

    // Find common prefix for the header
    let first = entries[0].as_str().unwrap_or("");
    let prefix = if let Some(pos) = first.rfind('/') {
        // Check if all entries share this prefix
        let candidate = &first[..=pos];
        if entries
            .iter()
            .all(|e| e.as_str().map_or(false, |s| s.starts_with(candidate)))
        {
            candidate.to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let mut out = String::new();
    if prefix.is_empty() {
        out.push_str(&format!("{count} entries\n"));
    } else {
        out.push_str(&format!("{prefix} — {count} entries\n"));
    }

    // Multi-column layout (3 columns)
    let display_names: Vec<String> = entries
        .iter()
        .filter_map(|e| e.as_str())
        .map(|path| {
            let without_prefix = if !prefix.is_empty() {
                path.strip_prefix(&prefix).unwrap_or(path)
            } else {
                path
            };
            without_prefix.to_string()
        })
        .collect();

    let max_name = display_names.iter().map(|n| n.len()).max().unwrap_or(0);
    let col_width = max_name + 2; // 2 chars padding
    let cols = 3.min(if col_width > 0 { 80 / col_width } else { 3 }).max(1);

    for (i, name) in display_names.iter().enumerate() {
        if i % cols == 0 {
            out.push_str("\n  ");
        }
        out.push_str(&format!("{:<width$}", name, width = col_width));
    }

    // Append overflow hint if present
    if let Some(overflow) = val.get("overflow") {
        out.push_str("\n\n");
        out.push_str(&format_overflow(overflow));
    }

    out
}
```

**Step 3: Wire into `ListDir` in `src/tools/file.rs`**

Add import near the top of `file.rs`:
```rust
use super::user_format;
```

Add to the `ListDir` Tool impl:
```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_list_dir(result))
    }
```

**Step 4: Run tests, commit**

Run: `cargo build && cargo test`

```bash
git add src/tools/user_format.rs src/tools/file.rs
git commit -m "feat(tools): dual-audience output for list_dir"
```

---

### Task 6: Implement `format_search_pattern`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/file.rs`

**Step 1: Write failing tests**

```rust
    #[test]
    fn search_pattern_basic() {
        let val = serde_json::json!({
            "matches": [
                { "file": "src/tools/mod.rs", "line": 54, "content": "pub struct RecoverableError {" },
                { "file": "src/tools/mod.rs", "line": 60, "content": "    RecoverableError { error, hint }" },
                { "file": "src/server.rs", "line": 230, "content": "RecoverableError => {" },
            ],
            "total": 3
        });
        let result = format_search_pattern(&val);
        assert!(result.contains("3 matches"));
        assert!(result.contains("src/tools/mod.rs:54"));
        assert!(result.contains("pub struct RecoverableError {"));
    }

    #[test]
    fn search_pattern_context_mode() {
        let val = serde_json::json!({
            "matches": [
                {
                    "file": "src/tools/mod.rs",
                    "match_line": 54,
                    "start_line": 52,
                    "content": "/// Soft error\npub struct RecoverableError {\n    pub error: String,"
                }
            ],
            "total": 1
        });
        let result = format_search_pattern(&val);
        assert!(result.contains("src/tools/mod.rs"));
        assert!(result.contains("52"));
    }

    #[test]
    fn search_pattern_with_overflow() {
        let val = serde_json::json!({
            "matches": [{ "file": "a.rs", "line": 1, "content": "match" }],
            "total": 1,
            "overflow": { "shown": 50, "total": 0, "hint": "narrow with path=" }
        });
        let result = format_search_pattern(&val);
        assert!(result.contains("first 50"));
    }
```

**Step 2: Implement**

```rust
/// Format `search_pattern` results in grep-style.
///
/// Simple mode:
/// ```text
/// 3 matches
///
///   src/tools/mod.rs:54     pub struct RecoverableError {
///   src/tools/mod.rs:60         RecoverableError { error, hint }
///   src/server.rs:230       RecoverableError => {
/// ```
///
/// Context mode:
/// ```text
/// 1 match
///
///   src/tools/mod.rs
///   52   /// Soft error
///   53   pub struct RecoverableError {
///   54       pub error: String,
/// ```
pub fn format_search_pattern(val: &Value) -> String {
    let matches = match val["matches"].as_array() {
        Some(m) => m,
        None => return "No matches.".to_string(),
    };
    let total = val["total"].as_u64().unwrap_or(matches.len() as u64);

    let mut out = if total == 1 {
        "1 match\n".to_string()
    } else {
        format!("{total} matches\n")
    };

    // Detect context mode by presence of "start_line" key
    let context_mode = matches
        .first()
        .map_or(false, |m| m.get("start_line").is_some());

    if context_mode {
        // Group by file, show line-numbered blocks
        let mut current_file = "";
        for m in matches {
            let file = m["file"].as_str().unwrap_or("?");
            let start = m["start_line"].as_u64().unwrap_or(1) as usize;
            let content = m["content"].as_str().unwrap_or("");

            if file != current_file {
                out.push_str(&format!("\n  {file}\n"));
                current_file = file;
            }
            for (i, line) in content.lines().enumerate() {
                out.push_str(&format!("  {:<4} {line}\n", start + i));
            }
        }
    } else {
        // Simple mode: file:line  content
        let labels: Vec<String> = matches
            .iter()
            .map(|m| {
                let file = m["file"].as_str().unwrap_or("?");
                let line = m["line"].as_u64().unwrap_or(0);
                format!("{file}:{line}")
            })
            .collect();
        let max_label = labels.iter().map(|l| l.len()).max().unwrap_or(0);

        for (i, m) in matches.iter().enumerate() {
            let content = m["content"].as_str().unwrap_or("");
            out.push_str(&format!(
                "\n  {:<width$}  {content}",
                labels[i],
                width = max_label
            ));
        }
    }

    if let Some(overflow) = val.get("overflow") {
        out.push_str("\n\n");
        out.push_str(&format_overflow(overflow));
    }

    out
}
```

**Step 3: Wire into `SearchPattern`, test, commit**

In `src/tools/file.rs` add to `SearchPattern` impl:
```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_search_pattern(result))
    }
```

Run: `cargo build && cargo test`

```bash
git add src/tools/user_format.rs src/tools/file.rs
git commit -m "feat(tools): dual-audience output for search_pattern"
```

---

### Task 7: Implement `format_find_symbol`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/symbol.rs`

**Step 1: Write failing tests**

```rust
    #[test]
    fn find_symbol_basic() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "OutputGuard",
                    "name_path": "OutputGuard",
                    "kind": "Struct",
                    "file": "src/tools/output.rs",
                    "start_line": 35,
                    "end_line": 50
                },
                {
                    "name": "cap_items",
                    "name_path": "OutputGuard/cap_items",
                    "kind": "Function",
                    "file": "src/tools/output.rs",
                    "start_line": 55,
                    "end_line": 80
                }
            ],
            "total": 2
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("2 matches"));
        assert!(result.contains("Struct"));
        assert!(result.contains("OutputGuard"));
        assert!(result.contains("src/tools/output.rs:35"));
    }

    #[test]
    fn find_symbol_with_body() {
        let val = serde_json::json!({
            "symbols": [{
                "name": "cap_items",
                "name_path": "OutputGuard/cap_items",
                "kind": "Function",
                "file": "src/tools/output.rs",
                "start_line": 55,
                "end_line": 60,
                "body": "pub fn cap_items(&self) -> Option<OverflowInfo> {\n    // impl\n}"
            }],
            "total": 1
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("pub fn cap_items"));
    }

    #[test]
    fn find_symbol_with_overflow() {
        let val = serde_json::json!({
            "symbols": [
                { "name": "A", "kind": "Function", "file": "a.rs", "start_line": 1, "end_line": 1 }
            ],
            "total": 100,
            "overflow": {
                "shown": 1, "total": 100,
                "hint": "narrow with path=",
                "by_file": [["src/a.rs", 50], ["src/b.rs", 30]]
            }
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("1 of 100"));
    }
```

**Step 2: Implement**

```rust
/// Format `find_symbol` results.
///
/// ```text
/// 3 matches for "OutputGuard"
///
///   Struct    src/tools/output.rs:35   OutputGuard
///   Function  src/tools/output.rs:55   OutputGuard/cap_items
///   use       src/server.rs:120        OutputGuard
/// ```
pub fn format_find_symbol(val: &Value) -> String {
    let symbols = match val["symbols"].as_array() {
        Some(s) => s,
        None => return "No matches.".to_string(),
    };
    let total = val["total"].as_u64().unwrap_or(symbols.len() as u64);

    let mut out = if total == 1 {
        "1 match\n".to_string()
    } else {
        format!("{total} matches\n")
    };

    // Build aligned columns: kind, file:line, name_path
    let rows: Vec<(String, String, String)> = symbols
        .iter()
        .map(|s| {
            let kind = s["kind"].as_str().unwrap_or("?");
            let file = s["file"].as_str().unwrap_or("?");
            let start = s["start_line"].as_u64().unwrap_or(0);
            let end = s["end_line"].as_u64().unwrap_or(start);
            let name_path = s["name_path"]
                .as_str()
                .or_else(|| s["name"].as_str())
                .unwrap_or("?");
            let loc = if end > start {
                format!("{file}:{start}-{end}")
            } else {
                format!("{file}:{start}")
            };
            (kind.to_string(), loc, name_path.to_string())
        })
        .collect();

    let max_kind = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let max_loc = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);

    for (kind, loc, name) in &rows {
        out.push_str(&format!(
            "\n  {:<kw$}  {:<lw$}  {name}",
            kind,
            loc,
            kw = max_kind,
            lw = max_loc
        ));
    }

    // Show body if present (for single or few results)
    for s in symbols {
        if let Some(body) = s["body"].as_str() {
            let name = s["name_path"]
                .as_str()
                .or_else(|| s["name"].as_str())
                .unwrap_or("?");
            out.push_str(&format!("\n\n  // {name}\n"));
            for line in body.lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
    }

    if let Some(overflow) = val.get("overflow") {
        out.push_str("\n");
        out.push_str(&format_overflow(overflow));
    }

    out
}
```

**Step 3: Wire into `FindSymbol`, test, commit**

In `src/tools/symbol.rs`:
```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_find_symbol(result))
    }
```

Run: `cargo build && cargo test`

```bash
git add src/tools/user_format.rs src/tools/symbol.rs
git commit -m "feat(tools): dual-audience output for find_symbol"
```

---

### Task 8: Implement `format_list_symbols`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/symbol.rs`

**Step 1: Write failing tests**

```rust
    #[test]
    fn list_symbols_file_mode() {
        let val = serde_json::json!({
            "file": "src/tools/output.rs",
            "symbols": [
                {
                    "name": "OutputMode", "name_path": "OutputMode",
                    "kind": "Enum", "start_line": 10, "end_line": 15,
                    "children": [
                        { "name": "Exploring", "kind": "EnumMember", "start_line": 11, "end_line": 11 },
                        { "name": "Focused", "kind": "EnumMember", "start_line": 12, "end_line": 12 }
                    ]
                },
                {
                    "name": "OutputGuard", "name_path": "OutputGuard",
                    "kind": "Struct", "start_line": 35, "end_line": 50
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("src/tools/output.rs"));
        assert!(result.contains("Enum"));
        assert!(result.contains("OutputMode"));
        assert!(result.contains("Exploring"));
        assert!(result.contains("OutputGuard"));
    }

    #[test]
    fn list_symbols_directory_mode() {
        let val = serde_json::json!({
            "directory": "src/tools",
            "files": [
                {
                    "file": "src/tools/ast.rs",
                    "symbols": [
                        { "name": "ListFunctions", "kind": "Struct", "start_line": 10, "end_line": 20 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("src/tools/ast.rs"));
        assert!(result.contains("ListFunctions"));
    }
```

**Step 2: Implement**

```rust
/// Format `list_symbols` results as an indented tree.
///
/// File mode:
/// ```text
/// src/tools/output.rs — 4 symbols
///
///   Enum     OutputMode           L10-15
///              Exploring          L11
///              Focused            L12
///   Struct   OutputGuard          L35-50
/// ```
pub fn format_list_symbols(val: &Value) -> String {
    // Directory mode: {directory, files: [{file, symbols}]}
    if let Some(files) = val["files"].as_array() {
        let dir = val["directory"].as_str().unwrap_or(".");
        let mut out = format!("{dir}\n");
        for file_obj in files {
            let file = file_obj["file"].as_str().unwrap_or("?");
            let symbols = file_obj["symbols"].as_array();
            let count = symbols.map_or(0, |s| s.len());
            out.push_str(&format!("\n  {file} — {count} symbols\n"));
            if let Some(symbols) = symbols {
                format_symbol_tree(symbols, 2, &mut out);
            }
        }
        if let Some(overflow) = val.get("overflow") {
            out.push_str("\n");
            out.push_str(&format_overflow(overflow));
        }
        return out;
    }

    // File mode: {file, symbols: [...]}
    let file = val["file"].as_str().unwrap_or("?");
    let symbols = match val["symbols"].as_array() {
        Some(s) => s,
        None => return format!("{file} — no symbols"),
    };
    let count = symbols.len();
    let mut out = format!("{file} — {count} symbols\n");
    format_symbol_tree(symbols, 0, &mut out);

    if let Some(overflow) = val.get("overflow") {
        out.push_str("\n");
        out.push_str(&format_overflow(overflow));
    }

    out
}

/// Recursively format a symbol tree with indentation.
fn format_symbol_tree(symbols: &[Value], indent: usize, out: &mut String) {
    let prefix = "  ".repeat(indent + 1);
    let child_prefix = "  ".repeat(indent + 2);

    for s in symbols {
        let kind = s["kind"].as_str().unwrap_or("?");
        let name = s["name"].as_str().unwrap_or("?");
        let start = s["start_line"].as_u64().unwrap_or(0);
        let end = s["end_line"].as_u64().unwrap_or(start);
        let lr = format_line_range(start, end);

        out.push_str(&format!("\n{prefix}{kind:<9} {name:<24} {lr}"));

        // Render children indented
        if let Some(children) = s["children"].as_array() {
            for child in children {
                let ck = child["kind"].as_str().unwrap_or("?");
                let cn = child["name"].as_str().unwrap_or("?");
                let cs = child["start_line"].as_u64().unwrap_or(0);
                let ce = child["end_line"].as_u64().unwrap_or(cs);
                let clr = format_line_range(cs, ce);
                // Show kind only for non-trivial children
                let kind_label = match ck {
                    "EnumMember" | "Field" => "",
                    _ => ck,
                };
                if kind_label.is_empty() {
                    out.push_str(&format!("\n{child_prefix}  {cn:<24} {clr}"));
                } else {
                    out.push_str(&format!(
                        "\n{child_prefix}{kind_label:<9} {cn:<24} {clr}"
                    ));
                }
            }
        }
    }
}
```

**Step 3: Wire into `ListSymbols`, test, commit**

In `src/tools/symbol.rs`:
```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_list_symbols(result))
    }
```

Run: `cargo build && cargo test`

```bash
git add src/tools/user_format.rs src/tools/symbol.rs
git commit -m "feat(tools): dual-audience output for list_symbols"
```

---

### Task 9: Implement `format_semantic_search`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/semantic.rs`

**Step 1: Write failing tests**

```rust
    #[test]
    fn semantic_search_basic() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/tools/output.rs",
                    "start_line": 35, "end_line": 50,
                    "score": 0.923, "language": "rust",
                    "content": "pub struct OutputGuard...",
                    "source": "project"
                },
                {
                    "file_path": "docs/PROGRESSIVE_DISCOVERABILITY.md",
                    "start_line": 1, "end_line": 30,
                    "score": 0.871, "language": "markdown",
                    "content": "# Progressive Disclosure...",
                    "source": "project"
                }
            ],
            "total": 2
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("2 results"));
        assert!(result.contains("0.92"));
        assert!(result.contains("src/tools/output.rs:35-50"));
    }

    #[test]
    fn semantic_search_stale() {
        let val = serde_json::json!({
            "results": [],
            "total": 0,
            "stale": true,
            "behind_commits": 5,
            "hint": "Index is behind HEAD. Run index_project to update."
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("0 results"));
        assert!(result.contains("behind HEAD"));
    }
```

**Step 2: Implement**

```rust
/// Format `semantic_search` results with scores and staleness.
///
/// ```text
/// 3 results
///
///   0.92  src/tools/output.rs:35-50       OutputGuard struct
///   0.87  docs/PROGRESSIVE_DIS…:1-30      Design guide
///
///   Index is 2 commits behind HEAD — run index_project to refresh
/// ```
pub fn format_semantic_search(val: &Value) -> String {
    let results = val["results"].as_array();
    let total = val["total"].as_u64().unwrap_or(0);

    let mut out = if total == 1 {
        "1 result\n".to_string()
    } else {
        format!("{total} results\n")
    };

    if let Some(results) = results {
        // Build aligned columns: score, file:range, preview
        let rows: Vec<(String, String, String)> = results
            .iter()
            .map(|r| {
                let score = r["score"].as_f64().unwrap_or(0.0);
                let file = r["file_path"].as_str().unwrap_or("?");
                let start = r["start_line"].as_u64().unwrap_or(0);
                let end = r["end_line"].as_u64().unwrap_or(start);
                let content = r["content"].as_str().unwrap_or("");
                // Use first line of content as preview
                let preview = content.lines().next().unwrap_or("").trim();
                let preview = if preview.len() > 40 {
                    format!("{}…", &preview[..39])
                } else {
                    preview.to_string()
                };
                let loc = if end > start {
                    format!("{file}:{start}-{end}")
                } else {
                    format!("{file}:{start}")
                };
                (format!("{score:.2}"), loc, preview)
            })
            .collect();

        let max_loc = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        for (score, loc, preview) in &rows {
            out.push_str(&format!(
                "\n  {score}  {:<width$}  {preview}",
                loc,
                width = max_loc
            ));
        }
    }

    // Staleness warning
    if val["stale"].as_bool().unwrap_or(false) {
        let behind = val["behind_commits"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "\n\n  Index is {behind} commits behind HEAD — run index_project to refresh"
        ));
    }

    if let Some(overflow) = val.get("overflow") {
        out.push_str("\n\n");
        out.push_str(&format_overflow(overflow));
    }

    out
}
```

**Step 3: Wire into `SemanticSearch`, test, commit**

In `src/tools/semantic.rs`:
```rust
use super::user_format;
```
Add to `SemanticSearch` impl:
```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_semantic_search(result))
    }
```

Run: `cargo build && cargo test`

```bash
git add src/tools/user_format.rs src/tools/semantic.rs
git commit -m "feat(tools): dual-audience output for semantic_search"
```

---

### Task 10: Implement `format_read_file`

**Files:**
- Modify: `src/tools/user_format.rs`
- Modify: `src/tools/file.rs`

This is the most complex formatter because `read_file` has three output modes:
1. **Content mode** (small files / ranged reads): `{content, total_lines, source}`
2. **Summary mode** (large files): `{type: "source", line_count, symbols, file_id, hint}` or `{type: "markdown", headings}` etc.
3. **Overflow mode** (exploring cap): `{content, total_lines, overflow}`

**Step 1: Write failing tests**

```rust
    #[test]
    fn read_file_content() {
        let val = serde_json::json!({
            "content": "fn main() {\n    println!(\"hello\");\n}\n",
            "total_lines": 3,
            "source": "project"
        });
        let result = format_read_file(&val);
        assert!(result.contains("3 lines"));
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn read_file_source_summary() {
        let val = serde_json::json!({
            "type": "source",
            "line_count": 500,
            "symbols": [
                { "name": "OutputGuard", "kind": "Struct", "line": 35 },
                { "name": "cap_items", "kind": "Function", "line": 55 }
            ],
            "file_id": "@file_abc123",
            "hint": "Full file stored as @file_abc123."
        });
        let result = format_read_file(&val);
        assert!(result.contains("500 lines"));
        assert!(result.contains("OutputGuard"));
        assert!(result.contains("@file_abc123"));
    }

    #[test]
    fn read_file_markdown_summary() {
        let val = serde_json::json!({
            "type": "markdown",
            "line_count": 200,
            "headings": ["# Title", "## Section 1", "## Section 2"],
            "file_id": "@file_xyz",
            "hint": "Full file stored as @file_xyz."
        });
        let result = format_read_file(&val);
        assert!(result.contains("200 lines"));
        assert!(result.contains("# Title"));
    }
```

**Step 2: Implement**

```rust
/// Format `read_file` results.
///
/// Content mode:
/// ```text
/// 42 lines
///
///   1│ fn main() {
///   2│     println!("hello");
///   3│ }
/// ```
///
/// Summary mode (source):
/// ```text
/// 500 lines (Rust)
///
///   Symbols:
///     Struct    OutputGuard      L35
///     Function  cap_items        L55
///
///   Buffer: @file_abc123
/// ```
pub fn format_read_file(val: &Value) -> String {
    // Summary mode: has "type" field
    if let Some(file_type) = val["type"].as_str() {
        return format_read_file_summary(val, file_type);
    }

    // Content mode: has "content" field
    let content = val["content"].as_str().unwrap_or("");
    let total = val["total_lines"].as_u64().unwrap_or(0);

    let mut out = format!("{total} lines\n");

    // Show line-numbered content
    for (i, line) in content.lines().enumerate() {
        out.push_str(&format!("\n  {:>4}| {line}", i + 1));
    }

    if let Some(overflow) = val.get("overflow") {
        out.push_str("\n\n");
        out.push_str(&format_overflow(overflow));
    }

    out
}

fn format_read_file_summary(val: &Value, file_type: &str) -> String {
    let line_count = val["line_count"].as_u64().unwrap_or(0);
    let file_id = val["file_id"].as_str().unwrap_or("");

    let mut out = format!("{line_count} lines");

    match file_type {
        "source" => {
            out.push_str("\n\n  Symbols:");
            if let Some(symbols) = val["symbols"].as_array() {
                for s in symbols {
                    let kind = s["kind"].as_str().unwrap_or("?");
                    let name = s["name"].as_str().unwrap_or("?");
                    let line = s["line"].as_u64().unwrap_or(0);
                    out.push_str(&format!("\n    {kind:<10} {name:<28} L{line}"));
                }
            }
        }
        "markdown" => {
            out.push_str(" (Markdown)");
            if let Some(headings) = val["headings"].as_array() {
                out.push_str("\n\n  Headings:");
                for h in headings {
                    let heading = h.as_str().unwrap_or("");
                    out.push_str(&format!("\n    {heading}"));
                }
            }
        }
        "config" => {
            out.push_str(" (Config)");
            if let Some(preview) = val["preview"].as_str() {
                out.push_str("\n\n  Preview:");
                for line in preview.lines().take(10) {
                    out.push_str(&format!("\n    {line}"));
                }
            }
        }
        "generic" => {
            if let Some(head) = val["head"].as_str() {
                out.push_str("\n\n  Head:");
                for line in head.lines().take(5) {
                    out.push_str(&format!("\n    {line}"));
                }
            }
        }
        _ => {}
    }

    if !file_id.is_empty() {
        out.push_str(&format!("\n\n  Buffer: {file_id}"));
    }
    if let Some(hint) = val["hint"].as_str() {
        out.push_str(&format!("\n  {hint}"));
    }

    out
}
```

**Step 3: Wire into `ReadFile`, test, commit**

In `src/tools/file.rs` add to `ReadFile` impl:
```rust
    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_read_file(result))
    }
```

Run: `cargo build && cargo test`

```bash
git add src/tools/user_format.rs src/tools/file.rs
git commit -m "feat(tools): dual-audience output for read_file"
```

---

### Task 11: Final verification and clippy

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass (existing 533+ plus ~25 new user_format tests).

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

**Step 3: Run formatter**

Run: `cargo fmt`

**Step 4: Manual smoke test**

Run: `cargo run -- start --project .`

In a Claude Code session, test each tool and press Ctrl+O to verify the user view:
- `list_dir("src/tools")`
- `find_symbol("OutputGuard")`
- `list_symbols("src/tools/output.rs")`
- `search_pattern("RecoverableError")`
- `hover("src/tools/output.rs", line=35)`
- `goto_definition("src/tools/output.rs", line=35)`
- `semantic_search("progressive disclosure")` (needs index)
- `read_file("src/tools/output.rs")` (should hit summary mode)

**Step 5: Squash into clean commits if needed, then final commit**

```bash
git add -A
git commit -m "feat(tools): dual-audience output for 8 read-heavy tools

Add format_for_user() to Tool trait with per-tool implementations for:
list_dir, list_symbols, find_symbol, search_pattern, read_file,
semantic_search, hover, goto_definition.

User sees formatted plain text (Ctrl+O), LLM gets compact JSON."
```

---

## Summary

| Task | Tool | Complexity |
|------|------|-----------|
| 1 | Trait + call_content | Low — trait change only |
| 2 | user_format helpers | Low — utility functions |
| 3 | goto_definition | Low — simplest output |
| 4 | hover | Low — mostly pass-through |
| 5 | list_dir | Medium — multi-column layout |
| 6 | search_pattern | Medium — two modes (simple + context) |
| 7 | find_symbol | Medium — aligned columns + body |
| 8 | list_symbols | Medium — recursive tree |
| 9 | semantic_search | Medium — scores + staleness |
| 10 | read_file | High — three output modes |
| 11 | Verification | Low — test + clippy + smoke test |
