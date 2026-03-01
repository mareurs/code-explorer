//! User-facing formatting for tool results.
//!
//! Each public `format_*` function takes the JSON `Value` returned by
//! a tool's `call()` and produces a compact, human-readable plain-text
//! representation for the user's Ctrl+O expansion in Claude Code.

use serde_json::Value;

/// Format a line range like "L35-50" or "L35" if start == end.
pub(crate) fn format_line_range(start: u64, end: u64) -> String {
    if start == end || end == 0 {
        format!("L{start}")
    } else {
        format!("L{start}-{end}")
    }
}

/// Truncate a path to max_len chars, replacing the middle with "...".
#[allow(dead_code)] // Used by format_for_user impls added in subsequent tasks.
pub(crate) fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    if max_len < 5 {
        return path[..max_len].to_string();
    }
    let keep_end = max_len / 2;
    let keep_start = max_len - keep_end - 1; // 1 for the ellipsis char
    format!("{}…{}", &path[..keep_start], &path[path.len() - keep_end..])
}

/// Format an overflow hint as a compact one-liner.
pub(crate) fn format_overflow(overflow: &Value) -> String {
    let shown = overflow["shown"].as_u64().unwrap_or(0);
    let total = overflow["total"].as_u64().unwrap_or(0);
    let hint = overflow["hint"].as_str().unwrap_or("");
    if total > shown {
        format!("  … showing {shown} of {total} — {hint}")
    } else {
        format!("  … showing first {shown} — {hint}")
    }
}

/// Format goto_definition result for human display.
///
/// Single definition: shows path:line + indented context.
/// Multiple definitions: shows count + compact table.
/// External definitions show "(external)" or "(lib:name)" tag.
pub fn format_goto_definition(val: &Value) -> String {
    let defs = match val["definitions"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if defs.is_empty() {
        return String::new();
    }

    if defs.len() == 1 {
        let d = &defs[0];
        let file = d["file"].as_str().unwrap_or("?");
        let line = d["line"].as_u64().unwrap_or(0);
        let context = d["context"].as_str().unwrap_or("");
        let source = d["source"].as_str().unwrap_or("project");

        let mut out = if source != "project" {
            format!("{}:{} ({})", file, line, source)
        } else {
            format!("{}:{}", file, line)
        };

        if !context.is_empty() {
            out.push_str("\n\n  ");
            out.push_str(context);
        }
        return out;
    }

    // Multiple definitions: compact table
    let mut out = format!("{} definitions\n", defs.len());
    for d in defs {
        let file = d["file"].as_str().unwrap_or("?");
        let line = d["line"].as_u64().unwrap_or(0);
        let context = d["context"].as_str().unwrap_or("");
        let source = d["source"].as_str().unwrap_or("project");

        out.push_str("\n  ");
        out.push_str(&format!("{}:{}", file, line));
        if source != "project" {
            out.push_str(&format!(" ({})", source));
        }
        if !context.is_empty() {
            out.push_str(&format!("   {}", context));
        }
    }
    out
}

/// Format hover result for human display.
///
/// Strips markdown code fences, indents all content lines with two spaces,
/// and shows location header.
pub fn format_hover(val: &Value) -> String {
    let content = match val["content"].as_str() {
        Some(s) => s,
        None => return String::new(),
    };
    let location = val["location"].as_str().unwrap_or("");

    let mut out = String::new();
    if !location.is_empty() {
        out.push_str(location);
        out.push_str("\n\n");
    }

    // Strip markdown code fences and indent content
    let mut in_code_block = false;
    let mut first_content_line = true;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if !first_content_line {
            out.push('\n');
        }
        out.push_str("  ");
        out.push_str(line);
        first_content_line = false;
    }
    out
}

/// Format list_dir result for human display.
///
/// Shows common prefix header, entry count, and a multi-column layout
/// of filenames (directories shown with trailing `/`).
pub fn format_list_dir(val: &Value) -> String {
    let entries = match val["entries"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if entries.is_empty() {
        return "(empty directory)".to_string();
    }

    let names: Vec<&str> = entries.iter().filter_map(|e| e.as_str()).collect();
    if names.is_empty() {
        return "(empty directory)".to_string();
    }

    // Find common prefix (up to last `/`)
    let prefix = common_path_prefix(&names);

    // Strip prefix from each entry to get short names
    let short_names: Vec<&str> = names
        .iter()
        .map(|n| {
            let stripped = &n[prefix.len()..];
            if stripped.is_empty() {
                *n
            } else {
                stripped
            }
        })
        .collect();

    // Header: show directory path and entry count
    let dir_display = if prefix.is_empty() {
        ".".to_string()
    } else {
        // Remove trailing `/` for display
        prefix.trim_end_matches('/').to_string()
    };
    let mut out = format!("{} — {} entries\n", dir_display, names.len());

    // Multi-column layout
    let max_name_len = short_names.iter().map(|n| n.len()).max().unwrap_or(0);
    let col_width = max_name_len + 2; // 2 spaces padding
                                      // Terminal width ~80, minus 2 leading spaces = 78 usable
    let num_cols = (78 / col_width).max(1);

    out.push('\n');
    for (i, name) in short_names.iter().enumerate() {
        if i % num_cols == 0 {
            out.push_str("  ");
        }
        out.push_str(name);
        // Pad to column width unless it's the last in the row or last overall
        if (i + 1) % num_cols != 0 && i + 1 < short_names.len() {
            let padding = col_width - name.len();
            for _ in 0..padding {
                out.push(' ');
            }
        }
        if (i + 1) % num_cols == 0 && i + 1 < short_names.len() {
            out.push('\n');
        }
    }

    // Overflow hint
    if let Some(overflow) = val.get("overflow") {
        if overflow.is_object() {
            out.push('\n');
            out.push_str(&format_overflow(overflow));
        }
    }

    out
}

/// Find the longest common directory prefix across paths.
/// Returns a string ending with `/` (or empty if no common prefix).
fn common_path_prefix(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    if paths.len() == 1 {
        // Single entry: prefix is the directory part
        if let Some(pos) = paths[0].rfind('/') {
            return paths[0][..=pos].to_string();
        }
        return String::new();
    }

    let first = paths[0];
    let mut prefix_len = 0;
    let mut last_slash = 0;

    for (i, ch) in first.char_indices() {
        if paths[1..]
            .iter()
            .any(|p| p.len() <= i || p.as_bytes()[i] != ch as u8)
        {
            break;
        }
        prefix_len = i + ch.len_utf8();
        if ch == '/' {
            last_slash = prefix_len;
        }
    }

    // Only use prefix up to the last `/` to avoid partial directory names
    if last_slash > 0 {
        first[..last_slash].to_string()
    } else {
        // Check if the full prefix ends with `/`
        let candidate = &first[..prefix_len];
        if candidate.ends_with('/') {
            candidate.to_string()
        } else {
            String::new()
        }
    }
}

/// Format search_pattern result for human display.
///
/// Detects simple mode vs context mode based on the presence of `start_line`
/// in the first match. Simple mode shows grep-style output; context mode
/// groups by file with numbered lines.
pub fn format_search_pattern(val: &Value) -> String {
    let matches = match val["matches"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    let total = val["total"].as_u64().unwrap_or(matches.len() as u64);

    if matches.is_empty() {
        return "0 matches".to_string();
    }

    // Detect mode: context mode has `start_line` field
    let is_context_mode = matches[0].get("start_line").is_some();

    let match_word = if total == 1 { "match" } else { "matches" };
    let mut out = format!("{total} {match_word}\n");

    if is_context_mode {
        format_search_context_mode(&mut out, matches);
    } else {
        format_search_simple_mode(&mut out, matches);
    }

    // Overflow hint
    if let Some(overflow) = val.get("overflow") {
        if overflow.is_object() {
            out.push('\n');
            out.push_str(&format_overflow(overflow));
        }
    }

    out
}

/// Simple (grep-style) search output: `file:line  content`
fn format_search_simple_mode(out: &mut String, matches: &[Value]) {
    // Calculate the max width of "file:line" for alignment
    let locations: Vec<String> = matches
        .iter()
        .map(|m| {
            let file = m["file"].as_str().unwrap_or("?");
            let line = m["line"].as_u64().unwrap_or(0);
            format!("{file}:{line}")
        })
        .collect();

    let max_loc_len = locations.iter().map(|l| l.len()).max().unwrap_or(0);

    for (i, m) in matches.iter().enumerate() {
        let content = m["content"].as_str().unwrap_or("");
        let padding = max_loc_len - locations[i].len();
        out.push_str("\n  ");
        out.push_str(&locations[i]);
        for _ in 0..padding {
            out.push(' ');
        }
        out.push_str("   ");
        out.push_str(content.trim());
    }
}

/// Context mode search output: grouped by file with line numbers.
fn format_search_context_mode(out: &mut String, matches: &[Value]) {
    let mut current_file: Option<&str> = None;

    for m in matches {
        let file = m["file"].as_str().unwrap_or("?");
        let start_line = m["start_line"].as_u64().unwrap_or(1);
        let content = m["content"].as_str().unwrap_or("");

        // Group header when file changes
        if current_file != Some(file) {
            out.push_str("\n  ");
            out.push_str(file);
            out.push('\n');
            current_file = Some(file);
        }

        // Show each line with its number
        for (i, line) in content.lines().enumerate() {
            let line_num = start_line + i as u64;
            out.push_str(&format!("  {:<4} {}\n", line_num, line));
        }
    }

    // Remove trailing newline if present
    if out.ends_with('\n') {
        out.pop();
    }
}

/// Format find_symbol result for human display.
///
/// Shows aligned columns: kind, file:line-range, name_path.
/// Optionally shows body for the first results that have one.
/// Handles overflow with `format_overflow`.
pub fn format_find_symbol(val: &Value) -> String {
    let symbols = match val["symbols"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    let total = val["total"].as_u64().unwrap_or(symbols.len() as u64);

    if symbols.is_empty() {
        return "0 matches".to_string();
    }

    // Build column data for alignment
    struct SymRow {
        kind: String,
        location: String,
        name_path: String,
        body: Option<String>,
    }

    let rows: Vec<SymRow> = symbols
        .iter()
        .map(|s| {
            let kind = s["kind"].as_str().unwrap_or("?").to_string();
            let file = s["file"].as_str().unwrap_or("?");
            let start = s["start_line"].as_u64().unwrap_or(0);
            let end = s["end_line"].as_u64().unwrap_or(0);
            let location = if end > start {
                format!("{file}:{start}-{end}")
            } else {
                format!("{file}:{start}")
            };
            let name_path = s["name_path"]
                .as_str()
                .or_else(|| s["name"].as_str())
                .unwrap_or("?")
                .to_string();
            let body = s["body"].as_str().map(|b| b.to_string());
            SymRow {
                kind,
                location,
                name_path,
                body,
            }
        })
        .collect();

    let max_kind_len = rows.iter().map(|r| r.kind.len()).max().unwrap_or(0);
    let max_loc_len = rows.iter().map(|r| r.location.len()).max().unwrap_or(0);

    // Header
    let match_word = if total == 1 { "match" } else { "matches" };
    let header = if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        let shown = overflow["shown"].as_u64().unwrap_or(symbols.len() as u64);
        format!("{shown} {match_word} ({total} total)")
    } else {
        format!("{total} {match_word}")
    };
    let mut out = format!("{header}\n");

    // Symbol rows
    for row in &rows {
        let kind_pad = max_kind_len - row.kind.len();
        let loc_pad = max_loc_len - row.location.len();
        out.push_str("\n  ");
        out.push_str(&row.kind);
        for _ in 0..kind_pad {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(&row.location);
        for _ in 0..loc_pad {
            out.push(' ');
        }
        out.push_str("   ");
        out.push_str(&row.name_path);

        // Show body indented if present
        if let Some(body) = &row.body {
            out.push('\n');
            for line in body.lines() {
                out.push_str("\n      ");
                out.push_str(line);
            }
        }
    }

    // Overflow hint
    if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        out.push('\n');
        out.push_str(&format_overflow(overflow));
    }

    out
}

/// Format list_symbols result for human display.
///
/// Handles three modes:
/// - **File mode**: `{ "file": "...", "symbols": [...] }` — single file with symbol tree.
/// - **Directory mode**: `{ "directory": "...", "files": [...] }` — multiple files.
/// - **Pattern/glob mode**: `{ "pattern": "...", "files": [...] }` — glob results.
pub fn format_list_symbols(val: &Value) -> String {
    // File mode
    if let Some(file) = val["file"].as_str() {
        let symbols = match val["symbols"].as_array() {
            Some(arr) => arr,
            None => return String::new(),
        };
        let count = symbols.len();
        let sym_word = if count == 1 { "symbol" } else { "symbols" };
        let mut out = format!("{file} — {count} {sym_word}\n");
        format_symbol_tree(&mut out, symbols, 2);

        // Overflow
        if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
            out.push('\n');
            out.push_str(&format_overflow(overflow));
        }
        return out;
    }

    // Directory or pattern mode
    let dir = val["directory"]
        .as_str()
        .or_else(|| val["pattern"].as_str())
        .unwrap_or(".");
    let files = match val["files"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if files.is_empty() {
        return format!("{dir} — 0 symbols");
    }

    let mut out = format!("{dir}\n");

    for file_entry in files {
        let file = file_entry["file"].as_str().unwrap_or("?");
        let symbols = match file_entry["symbols"].as_array() {
            Some(arr) => arr,
            None => continue,
        };
        let count = symbols.len();
        let sym_word = if count == 1 { "symbol" } else { "symbols" };
        out.push_str(&format!("\n  {file} — {count} {sym_word}\n"));
        format_symbol_tree(&mut out, symbols, 4);
    }

    // Overflow
    if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        out.push('\n');
        out.push_str(&format_overflow(overflow));
    }

    out
}

/// Render a tree of symbols with indentation.
///
/// `indent` is the number of leading spaces for top-level symbols.
/// Children get `indent + 5` spaces (nested under parent).
/// EnumMember and Field children omit the kind label.
fn format_symbol_tree(out: &mut String, symbols: &[Value], indent: usize) {
    // Compute alignment widths across all symbols at this level
    let max_kind_len = symbols
        .iter()
        .map(|s| s["kind"].as_str().unwrap_or("").len())
        .max()
        .unwrap_or(0);
    let max_name_len = symbols
        .iter()
        .map(|s| {
            s["name_path"]
                .as_str()
                .or_else(|| s["name"].as_str())
                .unwrap_or("")
                .len()
        })
        .max()
        .unwrap_or(0);

    let pad = " ".repeat(indent);

    for sym in symbols {
        let kind = sym["kind"].as_str().unwrap_or("?");
        let name = sym["name_path"]
            .as_str()
            .or_else(|| sym["name"].as_str())
            .unwrap_or("?");
        let start = sym["start_line"].as_u64().unwrap_or(0);
        let end = sym["end_line"].as_u64().unwrap_or(0);
        let line_range = format_line_range(start, end);

        let kind_pad = max_kind_len - kind.len();
        let name_pad = max_name_len.saturating_sub(name.len());
        out.push('\n');
        out.push_str(&pad);
        out.push_str(kind);
        for _ in 0..kind_pad {
            out.push(' ');
        }
        out.push_str("   ");
        out.push_str(name);
        for _ in 0..name_pad {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(&line_range);

        // Children
        if let Some(children) = sym["children"].as_array() {
            let child_indent = indent + 5;
            let child_pad = " ".repeat(child_indent);
            // Compute child name alignment
            let max_child_name = children
                .iter()
                .map(|c| c["name"].as_str().unwrap_or("").len())
                .max()
                .unwrap_or(0);

            for child in children {
                let child_kind = child["kind"].as_str().unwrap_or("?");
                let child_name = child["name"].as_str().unwrap_or("?");
                let cs = child["start_line"].as_u64().unwrap_or(0);
                let ce = child["end_line"].as_u64().unwrap_or(0);
                let child_lr = format_line_range(cs, ce);
                let child_name_pad = max_child_name.saturating_sub(child_name.len());

                out.push('\n');
                out.push_str(&child_pad);

                // Omit kind for EnumMember and Field
                if child_kind == "EnumMember" || child_kind == "Field" {
                    out.push_str(child_name);
                    for _ in 0..child_name_pad {
                        out.push(' ');
                    }
                } else {
                    out.push_str(child_kind);
                    out.push_str("  ");
                    out.push_str(child_name);
                    for _ in 0..child_name_pad {
                        out.push(' ');
                    }
                }
                out.push_str("  ");
                out.push_str(&child_lr);
            }
        }
    }
}

/// Format semantic_search result for human display.
///
/// Shows ranked results with score, file:range, and content preview.
/// Shows staleness warning if the index is behind HEAD.
pub fn format_semantic_search(val: &Value) -> String {
    let results = match val["results"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };
    let total = val["total"].as_u64().unwrap_or(results.len() as u64);

    if results.is_empty() {
        return "0 results".to_string();
    }

    let result_word = if total == 1 { "result" } else { "results" };
    let mut out = format!("{total} {result_word}\n");

    // Build rows: (score_str, location, preview)
    let rows: Vec<(String, String, String)> = results
        .iter()
        .map(|r| {
            let score = r["score"].as_f64().unwrap_or(0.0);
            let score_str = format!("{:.2}", score);
            let file = r["file_path"].as_str().unwrap_or("?");
            let start = r["start_line"].as_u64().unwrap_or(0);
            let end = r["end_line"].as_u64().unwrap_or(0);
            let location = if start > 0 && end > 0 && start != end {
                format!("{file}:{start}-{end}")
            } else if start > 0 {
                format!("{file}:{start}")
            } else {
                file.to_string()
            };

            // Content preview: first line, truncated to ~50 chars
            let content = r["content"].as_str().unwrap_or("");
            let first_line = content.lines().next().unwrap_or("").trim();
            let preview = if first_line.len() > 50 {
                format!("{}...", &first_line[..47])
            } else {
                first_line.to_string()
            };

            (score_str, location, preview)
        })
        .collect();

    // Compute column widths for alignment
    let max_score_len = rows.iter().map(|(s, _, _)| s.len()).max().unwrap_or(0);
    let max_loc_len = rows.iter().map(|(_, l, _)| l.len()).max().unwrap_or(0);

    for (score_str, location, preview) in &rows {
        out.push('\n');
        // Right-align score within its column
        let score_pad = max_score_len - score_str.len();
        out.push_str("  ");
        for _ in 0..score_pad {
            out.push(' ');
        }
        out.push_str(score_str);
        out.push_str("  ");
        out.push_str(location);
        // Pad location column
        let loc_pad = max_loc_len - location.len();
        for _ in 0..loc_pad {
            out.push(' ');
        }
        if !preview.is_empty() {
            out.push_str("  ");
            out.push_str(preview);
        }
    }

    // Staleness warning
    if val["stale"].as_bool() == Some(true) {
        out.push('\n');
        let behind = val["behind_commits"].as_u64();
        if let Some(n) = behind {
            out.push_str(&format!(
                "\n  Index is {n} commits behind HEAD — run index_project to refresh"
            ));
        } else if let Some(hint) = val["hint"].as_str() {
            out.push_str(&format!("\n  {hint}"));
        }
    }

    // Overflow
    if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        out.push('\n');
        out.push_str(&format_overflow(overflow));
    }

    out
}

/// Format read_file result for human display.
///
/// Handles multiple modes:
/// - Content mode: line-numbered file content
/// - Source summary: symbols + buffer reference
/// - Markdown summary: headings + buffer reference
/// - Config summary: preview + buffer reference
/// - Generic summary: head/tail + buffer reference
pub fn format_read_file(val: &Value) -> String {
    // Summary modes have a "type" key
    if let Some(file_type) = val["type"].as_str() {
        return format_read_file_summary(val, file_type);
    }

    // Content mode
    let content = match val["content"].as_str() {
        Some(c) => c,
        None => return String::new(),
    };

    let total_lines = val["total_lines"]
        .as_u64()
        .unwrap_or_else(|| content.lines().count() as u64);

    if content.is_empty() {
        let mut out = "0 lines".to_string();
        if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
            out.push('\n');
            out.push_str(&format_overflow(overflow));
        }
        return out;
    }

    let line_word = if total_lines == 1 { "line" } else { "lines" };
    let mut out = format!("{total_lines} {line_word}\n");

    // Line-numbered content with right-aligned line numbers
    let lines: Vec<&str> = content.lines().collect();
    let max_lineno = total_lines as usize;
    let lineno_width = max_lineno.to_string().len();

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        out.push('\n');
        out.push_str(&format!("{:>width$}| {line}", lineno, width = lineno_width));
    }

    // Overflow
    if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        out.push('\n');
        out.push_str(&format_overflow(overflow));
    }

    out
}

/// Format a read_file summary (large file buffered as summary + @file handle).
fn format_read_file_summary(val: &Value, file_type: &str) -> String {
    let line_count = val["line_count"].as_u64().unwrap_or(0);

    let type_label = match file_type {
        "markdown" => " (Markdown)",
        "config" => " (Config)",
        _ => "",
    };
    let mut out = format!("{line_count} lines{type_label}\n");

    match file_type {
        "source" => {
            if let Some(symbols) = val["symbols"].as_array() {
                if !symbols.is_empty() {
                    out.push_str("\n  Symbols:");

                    // Compute alignment widths
                    let max_kind = symbols
                        .iter()
                        .map(|s| s["kind"].as_str().unwrap_or("").len())
                        .max()
                        .unwrap_or(0);
                    let max_name = symbols
                        .iter()
                        .map(|s| s["name"].as_str().unwrap_or("").len())
                        .max()
                        .unwrap_or(0);

                    for sym in symbols {
                        let kind = sym["kind"].as_str().unwrap_or("?");
                        let name = sym["name"].as_str().unwrap_or("?");
                        let line = sym["line"].as_u64().unwrap_or(0);
                        let kind_pad = max_kind - kind.len();
                        let name_pad = max_name.saturating_sub(name.len());
                        out.push_str(&format!(
                            "\n    {kind}{:kind_pad$}  {name}{:name_pad$}  L{line}",
                            "",
                            "",
                            kind_pad = kind_pad,
                            name_pad = name_pad
                        ));
                    }
                }
            }
        }
        "markdown" => {
            if let Some(headings) = val["headings"].as_array() {
                if !headings.is_empty() {
                    out.push_str("\n  Headings:");
                    for h in headings {
                        if let Some(heading) = h.as_str() {
                            out.push_str(&format!("\n    {heading}"));
                        }
                    }
                }
            }
        }
        "config" => {
            if let Some(preview) = val["preview"].as_str() {
                out.push_str("\n  Preview:");
                for line in preview.lines() {
                    out.push_str(&format!("\n    {line}"));
                }
            }
        }
        "generic" => {
            if let Some(head) = val["head"].as_str() {
                out.push_str("\n  Head:");
                for line in head.lines() {
                    out.push_str(&format!("\n    {line}"));
                }
            }
            if let Some(tail) = val["tail"].as_str() {
                out.push_str("\n  Tail:");
                for line in tail.lines() {
                    out.push_str(&format!("\n    {line}"));
                }
            }
        }
        _ => {}
    }

    // Buffer reference
    if let Some(file_id) = val["file_id"].as_str() {
        out.push_str(&format!("\n\n  Buffer: {file_id}"));
    }
    if let Some(hint) = val["hint"].as_str() {
        out.push_str(&format!("\n  {hint}"));
    }

    out
}

pub fn format_find_file(result: &Value) -> String {
    let total = result["total"].as_u64().unwrap_or(0);
    let overflow = result["overflow"].is_object();
    let cap_note = if overflow {
        " (cap hit — narrow pattern)"
    } else {
        ""
    };
    format!("{total} files{cap_note}")
}

pub fn format_write_memory(result: &Value) -> String {
    let topic = result["topic"].as_str().unwrap_or("?");
    format!("written · {topic}")
}

pub fn format_read_memory(result: &Value) -> String {
    let topic = result["topic"].as_str().unwrap_or("?");
    match result["content"].as_str() {
        None => format!("not found · {topic}"),
        Some(content) => {
            let mut out = topic.to_string();
            for line in content.lines() {
                out.push_str(&format!("\n  {line}"));
            }
            out
        }
    }
}

pub fn format_list_memories(result: &Value) -> String {
    let topics = match result["topics"].as_array() {
        Some(t) if !t.is_empty() => t,
        _ => return "0 topics".to_string(),
    };
    let mut out = format!("{} topics", topics.len());
    for topic in topics.iter() {
        if let Some(name) = topic.as_str() {
            out.push_str(&format!("\n  {name}"));
        }
    }
    out
}

pub fn format_delete_memory(result: &Value) -> String {
    let topic = result["topic"].as_str().unwrap_or("?");
    format!("deleted · {topic}")
}

pub fn format_get_config(result: &Value) -> String {
    let root = result["project_root"].as_str().unwrap_or("?");
    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(root);
    format!("config · {name}")
}

pub fn format_activate_project(result: &Value) -> String {
    let root = result["activated"]["project_root"]
        .as_str()
        .or_else(|| result["path"].as_str())
        .unwrap_or("?");
    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(root);
    format!("activated · {name}")
}

pub fn format_index_project(result: &Value) -> String {
    let status = result["status"].as_str().unwrap_or("?");
    format!("index {status}")
}

pub fn format_index_library(result: &Value) -> String {
    let name = result["library"].as_str().unwrap_or("?");
    let chunks = result["chunks"].as_u64().unwrap_or(0);
    format!("{name} · {chunks} chunks")
}

pub fn format_list_libraries(result: &Value) -> String {
    let libs = match result["libraries"].as_array() {
        Some(l) if !l.is_empty() => l,
        _ => return "0 libraries".to_string(),
    };
    let name_width = libs
        .iter()
        .filter_map(|l| l["name"].as_str())
        .map(|n| n.len())
        .max()
        .unwrap_or(0);
    let mut out = format!("{} libraries", libs.len());
    for lib in libs.iter() {
        let name = lib["name"].as_str().unwrap_or("?");
        let status = if lib["indexed"].as_bool().unwrap_or(false) {
            "indexed"
        } else {
            "not indexed"
        };
        out.push_str(&format!("\n  {name:<name_width$}  {status}"));
    }
    out
}

pub fn format_index_status(result: &Value) -> String {
    let indexed = result["indexed"].as_bool().unwrap_or(false);
    if !indexed {
        return "not indexed".to_string();
    }
    let files = result["file_count"].as_u64().unwrap_or(0);
    let chunks = result["chunk_count"].as_u64().unwrap_or(0);
    let stale = result["stale"].as_bool().unwrap_or(false);
    let stale_note = if stale { " · stale" } else { "" };
    format!("{files} files · {chunks} chunks{stale_note}")
}

pub fn format_list_functions(result: &Value) -> String {
    let count = result["functions"].as_array().map(|a| a.len()).unwrap_or(0);
    let file = result["file"].as_str().unwrap_or("?");
    format!("{file} → {count} functions")
}

pub fn format_list_docs(result: &Value) -> String {
    let count = result["docstrings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let file = result["file"].as_str().unwrap_or("?");
    format!("{file} → {count} docstrings")
}

pub fn format_find_references(result: &Value) -> String {
    let total = result["total"].as_u64().unwrap_or_else(|| {
        result["references"]
            .as_array()
            .map(|a| a.len() as u64)
            .unwrap_or(0)
    });

    if total == 0 {
        return "No references found.".to_string();
    }

    let files: std::collections::HashSet<&str> = result["references"]
        .as_array()
        .map(|refs| refs.iter().filter_map(|r| r["file"].as_str()).collect())
        .unwrap_or_default();
    let file_count = files.len();
    if file_count > 1 {
        format!("{total} refs · {file_count} files")
    } else {
        format!("{total} refs")
    }
}

pub fn format_rename_symbol(result: &Value) -> String {
    let total_edits = result["total_edits"].as_u64().unwrap_or(0);
    let textual = result["textual_match_count"].as_u64().unwrap_or(0);
    let total = total_edits + textual;
    let new_name = result["new_name"].as_str().unwrap_or("?");
    let files = result["files_changed"].as_u64().unwrap_or(0);
    if files <= 1 {
        format!("→ {new_name} · {total} sites")
    } else {
        format!("→ {new_name} · {total} sites · {files} files")
    }
}

pub fn format_insert_code(result: &Value) -> String {
    let line = result["inserted_at_line"].as_u64().unwrap_or(0);
    let pos = result["position"].as_str().unwrap_or("after");
    format!("inserted {pos} L{line}")
}

pub fn format_replace_symbol(result: &Value) -> String {
    let lines = result["replaced_lines"].as_str().unwrap_or("?");
    format!("replaced · L{lines}")
}

pub fn format_remove_symbol(result: &Value) -> String {
    let lines = result["removed_lines"].as_str().unwrap_or("?");
    let count = result["line_count"].as_u64().unwrap_or(0);
    format!("removed · L{lines} ({count} lines)")
}

pub fn format_git_blame(result: &Value) -> String {
    let file = result["file"].as_str().unwrap_or("?");
    let line_count = result["lines"].as_array().map(|a| a.len()).unwrap_or(0);
    let authors: std::collections::HashSet<&str> = result["lines"]
        .as_array()
        .map(|lines| lines.iter().filter_map(|l| l["author"].as_str()).collect())
        .unwrap_or_default();
    if authors.is_empty() {
        format!("{file} · {line_count} lines")
    } else {
        format!("{file} · {line_count} lines · {} authors", authors.len())
    }
}

pub fn format_run_command(result: &Value) -> String {
    if result["output_id"].is_string() {
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        let output_id = result["output_id"].as_str().unwrap_or("");
        match result["type"].as_str() {
            Some("test") => {
                let passed = result["passed"].as_u64().unwrap_or(0);
                let failed = result["failed"].as_u64().unwrap_or(0);
                let ignored = result["ignored"].as_u64().unwrap_or(0);
                let mut s = format!("{check} exit {exit} · {passed} passed");
                if failed > 0 {
                    s.push_str(&format!(" · {failed} FAILED"));
                }
                if ignored > 0 {
                    s.push_str(&format!(" · {ignored} ignored"));
                }
                s.push_str(&format!("  (query {output_id})"));
                s
            }
            Some("build") => {
                let errors = result["errors"].as_u64().unwrap_or(0);
                if errors > 0 {
                    format!("{check} exit {exit} · {errors} errors  (query {output_id})")
                } else {
                    format!("{check} exit {exit}  (query {output_id})")
                }
            }
            _ => {
                let lines = result["total_stdout_lines"].as_u64().unwrap_or(0);
                format!("{check} exit {exit} · {lines} lines  (query {output_id})")
            }
        }
    } else if result["timed_out"].as_bool().unwrap_or(false) {
        "✗ timed out".to_string()
    } else {
        let exit = result["exit_code"].as_i64().unwrap_or(0);
        let stdout_lines = result["stdout"]
            .as_str()
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let check = if exit == 0 { "✓" } else { "✗" };
        format!("{check} exit {exit} · {stdout_lines} lines")
    }
}

pub fn format_onboarding(result: &Value) -> String {
    let langs = result["languages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "?".to_string());
    let created = result["config_created"].as_bool().unwrap_or(false);
    let config_note = if created { " · config created" } else { "" };
    format!("[{langs}]{config_note}")
}

// ─── ANSI diff helpers ────────────────────────────────────────────────────────

const BOLD_CYAN: &str = "\x1b[1;36m";
const BOLD_GREEN: &str = "\x1b[1;32m";
const BOLD_RED: &str = "\x1b[1;31m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

const DIFF_PREVIEW_LINES: usize = 8;

/// Format a separator header line:  ─── tool_name: path ──────
pub fn render_diff_header(tool_name: &str, path: &str) -> String {
    let title = format!(" {tool_name}: {path} ");
    let pad = "─".repeat((60usize).saturating_sub(title.len()));
    format!("{BOLD_CYAN}───{title}{pad}{RESET}")
}

/// Render a unified-style diff between old_string and new_string.
/// start_line is the 1-indexed line where old_string begins in the file (optional).
pub fn render_edit_diff(
    _path: &str,
    old_string: &str,
    new_string: &str,
    start_line: Option<usize>,
) -> String {
    let mut out = String::new();
    let old_lines: Vec<&str> = old_string.lines().collect();
    let new_lines: Vec<&str> = new_string.lines().collect();
    let hunk_start = start_line.unwrap_or(1);
    let hunk = format!(
        "@@ -{hunk_start},{} +{hunk_start},{} @@",
        old_lines.len(),
        new_lines.len()
    );
    out.push_str(&format!("{DIM}{hunk}{RESET}\n"));
    for line in &old_lines {
        out.push_str(&format!("{RED}-{line}{RESET}\n"));
    }
    for line in &new_lines {
        out.push_str(&format!("{GREEN}+{line}{RESET}\n"));
    }
    out
}

/// Render a diff showing removed symbol (all lines red).
pub fn render_removal_diff(
    _path: &str,
    removed_content: &str,
    start_line: Option<usize>,
    name: &str,
) -> String {
    let lines: Vec<&str> = removed_content.lines().collect();
    let total = lines.len();
    let preview_count = DIFF_PREVIEW_LINES.min(total);
    let hunk_start = start_line.unwrap_or(1);
    let mut out = String::new();
    out.push_str(&format!(
        "{BOLD_RED}--- removed · {name} · {total} lines{RESET}\n"
    ));
    out.push_str(&format!("{DIM}@@ -{hunk_start},{total} @@{RESET}\n"));
    for line in &lines[..preview_count] {
        out.push_str(&format!("{RED}-{line}{RESET}\n"));
    }
    if total > preview_count {
        let remaining = total - preview_count;
        out.push_str(&format!("{DIM}···  ({remaining} more lines){RESET}\n"));
    }
    out
}

/// Render a diff showing inserted code (all lines green).
pub fn render_insert_diff(
    _path: &str,
    code: &str,
    at_line: Option<usize>,
    position: &str,
    near_symbol: &str,
) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let total = lines.len();
    let preview_count = DIFF_PREVIEW_LINES.min(total);
    let insert_line = at_line.unwrap_or(1);
    let mut out = String::new();
    out.push_str(&format!(
        "{BOLD_GREEN}+++ inserted {position} {near_symbol} · {total} lines{RESET}\n"
    ));
    out.push_str(&format!("{DIM}@@ +{insert_line},{total} @@{RESET}\n"));
    for line in &lines[..preview_count] {
        out.push_str(&format!("{GREEN}+{line}{RESET}\n"));
    }
    if total > preview_count {
        let remaining = total - preview_count;
        out.push_str(&format!("{DIM}···  ({remaining} more lines){RESET}\n"));
    }
    out
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
        assert!(
            result.chars().count() <= 25,
            "got len {} for '{}'",
            result.chars().count(),
            result
        );
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
            "shown": 50, "total": 50, "hint": "use more specific pattern"
        });
        let result = format_overflow(&ov);
        assert!(result.contains("first 50"));
    }

    // --- format_goto_definition tests ---

    #[test]
    fn goto_single_project_definition() {
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
        assert_eq!(
            result,
            "src/tools/output.rs:35\n\n  pub struct OutputGuard {"
        );
    }

    #[test]
    fn goto_single_external_definition() {
        let val = serde_json::json!({
            "definitions": [{
                "file": "/home/user/.rustup/toolchains/stable/lib.rs",
                "line": 100,
                "end_line": 110,
                "context": "pub enum Option<T> {",
                "source": "external"
            }],
            "from": "main.rs:5"
        });
        let result = format_goto_definition(&val);
        assert!(result.contains("(external)"));
        assert!(result.contains(":100"));
        assert!(result.contains("pub enum Option<T> {"));
    }

    #[test]
    fn goto_multiple_definitions() {
        let val = serde_json::json!({
            "definitions": [
                { "file": "src/a.rs", "line": 10, "end_line": 15, "context": "fn foo()", "source": "project" },
                { "file": "src/b.rs", "line": 20, "end_line": 25, "context": "fn foo()", "source": "project" }
            ],
            "from": "main.rs:1"
        });
        let result = format_goto_definition(&val);
        assert!(result.starts_with("2 definitions"));
        assert!(result.contains("src/a.rs:10"));
        assert!(result.contains("src/b.rs:20"));
    }

    #[test]
    fn goto_empty_definitions() {
        let val = serde_json::json!({ "definitions": [] });
        assert_eq!(format_goto_definition(&val), "");
    }

    #[test]
    fn goto_no_context() {
        let val = serde_json::json!({
            "definitions": [{
                "file": "src/lib.rs",
                "line": 1,
                "end_line": 1,
                "context": "",
                "source": "project"
            }],
            "from": "main.rs:1"
        });
        let result = format_goto_definition(&val);
        assert_eq!(result, "src/lib.rs:1");
    }

    #[test]
    fn goto_multiple_with_external() {
        let val = serde_json::json!({
            "definitions": [
                { "file": "src/a.rs", "line": 10, "end_line": 10, "context": "fn foo()", "source": "project" },
                { "file": "/ext/lib.rs", "line": 20, "end_line": 20, "context": "fn foo()", "source": "lib:serde" }
            ],
            "from": "main.rs:1"
        });
        let result = format_goto_definition(&val);
        assert!(result.contains("2 definitions"));
        assert!(result.contains("src/a.rs:10"));
        assert!(result.contains("(lib:serde)"));
    }

    // --- format_hover tests ---

    #[test]
    fn hover_with_code_fence() {
        let val = serde_json::json!({
            "content": "```rust\npub struct OutputGuard {\n    mode: OutputMode,\n}\n```\n\nProgressive disclosure guard.",
            "location": "output.rs:35"
        });
        let result = format_hover(&val);
        assert!(result.starts_with("output.rs:35"));
        assert!(result.contains("  pub struct OutputGuard {"));
        assert!(result.contains("  Progressive disclosure guard."));
        // Code fences should be stripped
        assert!(!result.contains("```"));
    }

    #[test]
    fn hover_plain_text_no_fences() {
        let val = serde_json::json!({
            "content": "Some plain documentation.",
            "location": "lib.rs:10"
        });
        let result = format_hover(&val);
        assert_eq!(result, "lib.rs:10\n\n  Some plain documentation.");
    }

    #[test]
    fn hover_no_location() {
        let val = serde_json::json!({
            "content": "```rust\nfn main() {}\n```"
        });
        let result = format_hover(&val);
        assert!(!result.contains("```"));
        assert!(result.contains("  fn main() {}"));
    }

    #[test]
    fn hover_empty_content() {
        let val = serde_json::json!({});
        assert_eq!(format_hover(&val), "");
    }

    #[test]
    fn hover_multiline_doc() {
        let val = serde_json::json!({
            "content": "```rust\nfn add(a: i32, b: i32) -> i32\n```\n\nAdds two numbers.\n\nReturns the sum.",
            "location": "math.rs:5"
        });
        let result = format_hover(&val);
        assert!(result.contains("  fn add(a: i32, b: i32) -> i32"));
        assert!(result.contains("  Adds two numbers."));
        assert!(result.contains("  Returns the sum."));
        assert!(!result.contains("```"));
    }

    // --- format_list_dir tests ---

    #[test]
    fn list_dir_basic() {
        let val = serde_json::json!({
            "entries": [
                "src/tools/ast.rs",
                "src/tools/config.rs",
                "src/tools/file.rs",
                "src/tools/git.rs",
                "src/tools/library.rs",
                "src/tools/memory.rs",
                "src/tools/mod.rs",
                "src/tools/output.rs",
                "src/tools/semantic.rs",
                "src/tools/symbol.rs",
                "src/tools/workflow.rs",
                "src/tools/user_format.rs"
            ]
        });
        let result = format_list_dir(&val);
        assert!(result.starts_with("src/tools — 12 entries"));
        // Should show short names without the common prefix
        assert!(result.contains("ast.rs"));
        assert!(result.contains("mod.rs"));
        assert!(result.contains("user_format.rs"));
        // Should NOT contain full paths in the body
        assert!(!result.contains("src/tools/ast.rs"));
    }

    #[test]
    fn list_dir_with_overflow() {
        let val = serde_json::json!({
            "entries": ["src/a.rs", "src/b.rs"],
            "overflow": { "shown": 200, "total": 350, "hint": "Use a more specific path" }
        });
        let result = format_list_dir(&val);
        assert!(result.contains("2 entries"));
        assert!(result.contains("200 of 350"));
        assert!(result.contains("Use a more specific path"));
    }

    #[test]
    fn list_dir_empty() {
        let val = serde_json::json!({ "entries": [] });
        assert_eq!(format_list_dir(&val), "(empty directory)");
    }

    #[test]
    fn list_dir_no_common_prefix() {
        let val = serde_json::json!({
            "entries": ["Cargo.toml", "README.md", "src/"]
        });
        let result = format_list_dir(&val);
        assert!(result.starts_with(". — 3 entries"));
        assert!(result.contains("Cargo.toml"));
        assert!(result.contains("README.md"));
        assert!(result.contains("src/"));
    }

    #[test]
    fn list_dir_recursive_deep_paths() {
        let val = serde_json::json!({
            "entries": [
                "src/lsp/client.rs",
                "src/lsp/ops.rs",
                "src/lsp/config.rs",
                "src/lsp/types.rs"
            ]
        });
        let result = format_list_dir(&val);
        assert!(result.starts_with("src/lsp — 4 entries"));
        assert!(result.contains("client.rs"));
        assert!(result.contains("ops.rs"));
    }

    #[test]
    fn list_dir_single_entry() {
        let val = serde_json::json!({
            "entries": ["src/main.rs"]
        });
        let result = format_list_dir(&val);
        assert!(result.contains("1 entries"));
        assert!(result.contains("main.rs"));
    }

    #[test]
    fn list_dir_directories_with_slash() {
        let val = serde_json::json!({
            "entries": ["src/tools/", "src/lsp/", "src/embed/"]
        });
        let result = format_list_dir(&val);
        assert!(result.contains("tools/"));
        assert!(result.contains("lsp/"));
        assert!(result.contains("embed/"));
    }

    #[test]
    fn list_dir_missing_entries() {
        let val = serde_json::json!({});
        assert_eq!(format_list_dir(&val), "");
    }

    // --- common_path_prefix tests ---

    #[test]
    fn prefix_empty_input() {
        assert_eq!(common_path_prefix(&[]), "");
    }

    #[test]
    fn prefix_single_path() {
        assert_eq!(common_path_prefix(&["src/tools/mod.rs"]), "src/tools/");
    }

    #[test]
    fn prefix_common_dir() {
        assert_eq!(
            common_path_prefix(&["src/tools/a.rs", "src/tools/b.rs"]),
            "src/tools/"
        );
    }

    #[test]
    fn prefix_no_common() {
        assert_eq!(common_path_prefix(&["Cargo.toml", "README.md"]), "");
    }

    #[test]
    fn prefix_partial_name_not_included() {
        // "src/to" is a shared prefix but not at a `/` boundary
        assert_eq!(
            common_path_prefix(&["src/tools/a.rs", "src/tokens/b.rs"]),
            "src/"
        );
    }

    // --- format_search_pattern tests ---

    #[test]
    fn search_simple_mode() {
        let val = serde_json::json!({
            "matches": [
                { "file": "src/tools/mod.rs", "line": 54, "content": "pub struct RecoverableError {" },
                { "file": "src/tools/mod.rs", "line": 60, "content": "    RecoverableError { error, hint }" },
                { "file": "src/server.rs", "line": 230, "content": "RecoverableError => {" }
            ],
            "total": 3
        });
        let result = format_search_pattern(&val);
        assert!(result.starts_with("3 matches"));
        assert!(result.contains("src/tools/mod.rs:54"));
        assert!(result.contains("src/server.rs:230"));
        assert!(result.contains("pub struct RecoverableError {"));
    }

    #[test]
    fn search_context_mode() {
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
        assert!(result.starts_with("1 match\n"));
        assert!(result.contains("src/tools/mod.rs"));
        assert!(result.contains("52   /// Soft error"));
        assert!(result.contains("53   pub struct RecoverableError {"));
        assert!(result.contains("54       pub error: String,"));
    }

    #[test]
    fn search_with_overflow() {
        let val = serde_json::json!({
            "matches": [
                { "file": "src/a.rs", "line": 1, "content": "match" }
            ],
            "total": 1,
            "overflow": { "shown": 50, "hint": "Showing first 50 matches (cap hit). Narrow with a more specific pattern." }
        });
        let result = format_search_pattern(&val);
        assert!(result.contains("first 50"));
        assert!(result.contains("Narrow with a more specific pattern"));
    }

    #[test]
    fn search_empty_matches() {
        let val = serde_json::json!({
            "matches": [],
            "total": 0
        });
        assert_eq!(format_search_pattern(&val), "0 matches");
    }

    #[test]
    fn search_single_match_singular() {
        let val = serde_json::json!({
            "matches": [
                { "file": "src/main.rs", "line": 1, "content": "fn main() {" }
            ],
            "total": 1
        });
        let result = format_search_pattern(&val);
        assert!(result.starts_with("1 match\n"));
        // Should say "match" not "matches"
        assert!(!result.starts_with("1 matches"));
    }

    #[test]
    fn search_context_mode_multiple_files() {
        let val = serde_json::json!({
            "matches": [
                {
                    "file": "src/a.rs",
                    "match_line": 10,
                    "start_line": 9,
                    "content": "// context\nfn foo() {"
                },
                {
                    "file": "src/b.rs",
                    "match_line": 5,
                    "start_line": 4,
                    "content": "// other\nfn bar() {"
                }
            ],
            "total": 2
        });
        let result = format_search_pattern(&val);
        assert!(result.contains("2 matches"));
        assert!(result.contains("src/a.rs"));
        assert!(result.contains("src/b.rs"));
        assert!(result.contains("9    // context"));
        assert!(result.contains("10   fn foo() {"));
        assert!(result.contains("4    // other"));
        assert!(result.contains("5    fn bar() {"));
    }

    #[test]
    fn search_simple_alignment() {
        let val = serde_json::json!({
            "matches": [
                { "file": "a.rs", "line": 1, "content": "short" },
                { "file": "very/long/path.rs", "line": 100, "content": "long" }
            ],
            "total": 2
        });
        let result = format_search_pattern(&val);
        // The shorter location should be padded to align content
        // "a.rs:1" is shorter than "very/long/path.rs:100"
        // Both content columns should be aligned
        assert!(result.contains("a.rs:1"));
        assert!(result.contains("very/long/path.rs:100"));
    }

    #[test]
    fn search_missing_matches_key() {
        let val = serde_json::json!({});
        assert_eq!(format_search_pattern(&val), "");
    }

    // --- format_find_symbol tests ---

    #[test]
    fn find_symbol_no_body() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "OutputGuard", "name_path": "OutputGuard",
                    "kind": "Struct", "file": "src/tools/output.rs",
                    "start_line": 35, "end_line": 50
                },
                {
                    "name": "cap_items", "name_path": "OutputGuard/cap_items",
                    "kind": "Function", "file": "src/tools/output.rs",
                    "start_line": 55, "end_line": 80
                }
            ],
            "total": 2
        });
        let result = format_find_symbol(&val);
        assert!(result.starts_with("2 matches\n"));
        assert!(result.contains("Struct"));
        assert!(result.contains("Function"));
        assert!(result.contains("OutputGuard"));
        assert!(result.contains("OutputGuard/cap_items"));
        assert!(result.contains("src/tools/output.rs:35-50"));
        assert!(result.contains("src/tools/output.rs:55-80"));
    }

    #[test]
    fn find_symbol_with_body() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "cap_items", "name_path": "OutputGuard/cap_items",
                    "kind": "Function", "file": "src/tools/output.rs",
                    "start_line": 55, "end_line": 80,
                    "body": "pub fn cap_items(&self) -> Option<OverflowInfo> {\n    // impl\n}"
                }
            ],
            "total": 1
        });
        let result = format_find_symbol(&val);
        assert!(result.starts_with("1 match\n"));
        assert!(result.contains("Function"));
        assert!(result.contains("OutputGuard/cap_items"));
        assert!(result.contains("      pub fn cap_items(&self) -> Option<OverflowInfo> {"));
        assert!(result.contains("      // impl"));
        assert!(result.contains("      }"));
    }

    #[test]
    fn find_symbol_with_overflow() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "foo", "name_path": "foo",
                    "kind": "Function", "file": "src/a.rs",
                    "start_line": 10, "end_line": 10
                }
            ],
            "total": 100,
            "overflow": {
                "shown": 20, "total": 100,
                "hint": "narrow with path=",
                "by_file": [["src/a.rs", 50], ["src/b.rs", 30]]
            }
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("20 matches (100 total)"));
        assert!(result.contains("20 of 100"));
        assert!(result.contains("narrow with path="));
    }

    #[test]
    fn find_symbol_empty() {
        let val = serde_json::json!({
            "symbols": [],
            "total": 0
        });
        assert_eq!(format_find_symbol(&val), "0 matches");
    }

    #[test]
    fn find_symbol_missing_symbols_key() {
        let val = serde_json::json!({});
        assert_eq!(format_find_symbol(&val), "");
    }

    #[test]
    fn find_symbol_alignment() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "Foo", "name_path": "Foo",
                    "kind": "Struct", "file": "src/a.rs",
                    "start_line": 1, "end_line": 5
                },
                {
                    "name": "bar_baz", "name_path": "bar_baz",
                    "kind": "Function", "file": "src/very/long/path.rs",
                    "start_line": 100, "end_line": 200
                }
            ],
            "total": 2
        });
        let result = format_find_symbol(&val);
        // Kind column should be padded: "Struct  " vs "Function"
        assert!(result.contains("Struct  "));
        assert!(result.contains("Function"));
        // Location column should be padded
        assert!(result.contains("src/a.rs:1-5"));
        assert!(result.contains("src/very/long/path.rs:100-200"));
    }

    #[test]
    fn find_symbol_single_line_location() {
        let val = serde_json::json!({
            "symbols": [
                {
                    "name": "X", "name_path": "X",
                    "kind": "Constant", "file": "src/lib.rs",
                    "start_line": 42, "end_line": 42
                }
            ],
            "total": 1
        });
        let result = format_find_symbol(&val);
        assert!(result.contains("src/lib.rs:42"));
        // Should NOT show "42-42"
        assert!(!result.contains("42-42"));
    }

    // --- format_list_symbols tests ---

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
        assert!(result.starts_with("src/tools/output.rs — 2 symbols\n"));
        assert!(result.contains("Enum"));
        assert!(result.contains("OutputMode"));
        assert!(result.contains("L10-15"));
        assert!(result.contains("Exploring"));
        assert!(result.contains("L11"));
        assert!(result.contains("Focused"));
        assert!(result.contains("L12"));
        assert!(result.contains("Struct"));
        assert!(result.contains("OutputGuard"));
        assert!(result.contains("L35-50"));
        // EnumMember children should NOT show their kind label
        assert!(!result.contains("EnumMember"));
    }

    #[test]
    fn list_symbols_directory_mode() {
        let val = serde_json::json!({
            "directory": "src/tools",
            "files": [
                {
                    "file": "src/tools/ast.rs",
                    "symbols": [
                        { "name": "ListFunctions", "name_path": "ListFunctions", "kind": "Struct", "start_line": 10, "end_line": 20 }
                    ]
                },
                {
                    "file": "src/tools/config.rs",
                    "symbols": [
                        { "name": "GetConfig", "name_path": "GetConfig", "kind": "Struct", "start_line": 5, "end_line": 15 },
                        { "name": "ActivateProject", "name_path": "ActivateProject", "kind": "Struct", "start_line": 20, "end_line": 30 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.starts_with("src/tools\n"));
        assert!(result.contains("src/tools/ast.rs — 1 symbol\n"));
        assert!(result.contains("src/tools/config.rs — 2 symbols\n"));
        assert!(result.contains("ListFunctions"));
        assert!(result.contains("GetConfig"));
        assert!(result.contains("ActivateProject"));
    }

    #[test]
    fn list_symbols_pattern_mode() {
        let val = serde_json::json!({
            "pattern": "src/**/*.rs",
            "files": [
                {
                    "file": "src/main.rs",
                    "symbols": [
                        { "name": "main", "name_path": "main", "kind": "Function", "start_line": 1, "end_line": 10 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.starts_with("src/**/*.rs\n"));
        assert!(result.contains("src/main.rs — 1 symbol\n"));
        assert!(result.contains("main"));
    }

    #[test]
    fn list_symbols_empty_file() {
        let val = serde_json::json!({
            "file": "src/empty.rs",
            "symbols": []
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("0 symbols"));
    }

    #[test]
    fn list_symbols_empty_directory() {
        let val = serde_json::json!({
            "directory": "src/empty",
            "files": []
        });
        let result = format_list_symbols(&val);
        assert_eq!(result, "src/empty — 0 symbols");
    }

    #[test]
    fn list_symbols_with_overflow() {
        let val = serde_json::json!({
            "directory": "src",
            "files": [
                {
                    "file": "src/a.rs",
                    "symbols": [
                        { "name": "Foo", "name_path": "Foo", "kind": "Struct", "start_line": 1, "end_line": 5 }
                    ]
                }
            ],
            "overflow": { "shown": 10, "total": 50, "hint": "Narrow with a more specific glob or file path" }
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("10 of 50"));
        assert!(result.contains("Narrow with a more specific glob"));
    }

    #[test]
    fn list_symbols_children_with_fields() {
        let val = serde_json::json!({
            "file": "src/model.rs",
            "symbols": [
                {
                    "name": "Config", "name_path": "Config",
                    "kind": "Struct", "start_line": 1, "end_line": 10,
                    "children": [
                        { "name": "port", "kind": "Field", "start_line": 2, "end_line": 2 },
                        { "name": "host", "kind": "Field", "start_line": 3, "end_line": 3 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        // Field children should NOT show kind label
        assert!(!result.contains("Field"));
        assert!(result.contains("port"));
        assert!(result.contains("host"));
        assert!(result.contains("L2"));
        assert!(result.contains("L3"));
    }

    #[test]
    fn list_symbols_children_with_methods() {
        let val = serde_json::json!({
            "file": "src/service.rs",
            "symbols": [
                {
                    "name": "Server", "name_path": "Server",
                    "kind": "Struct", "start_line": 1, "end_line": 50,
                    "children": [
                        { "name": "new", "kind": "Function", "start_line": 5, "end_line": 10 },
                        { "name": "run", "kind": "Function", "start_line": 12, "end_line": 40 }
                    ]
                }
            ]
        });
        let result = format_list_symbols(&val);
        // Function children SHOULD show kind label
        assert!(result.contains("Function  new"));
        assert!(result.contains("Function  run"));
    }

    #[test]
    fn list_symbols_missing_symbols_key() {
        let val = serde_json::json!({});
        assert_eq!(format_list_symbols(&val), "");
    }

    #[test]
    fn list_symbols_singular_symbol_word() {
        let val = serde_json::json!({
            "file": "src/single.rs",
            "symbols": [
                { "name": "main", "name_path": "main", "kind": "Function", "start_line": 1, "end_line": 5 }
            ]
        });
        let result = format_list_symbols(&val);
        assert!(result.contains("1 symbol\n"));
        // Should say "symbol" not "symbols"
        assert!(!result.contains("1 symbols"));
    }

    // --- format_semantic_search tests ---

    #[test]
    fn semantic_search_basic() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/tools/output.rs",
                    "language": "rust",
                    "content": "pub struct OutputGuard {\n    mode: OutputMode,\n}",
                    "start_line": 35,
                    "end_line": 50,
                    "score": 0.923,
                    "source": "project"
                },
                {
                    "file_path": "src/tools/mod.rs",
                    "language": "rust",
                    "content": "pub trait Tool {\n    fn name(&self) -> &str;\n}",
                    "start_line": 120,
                    "end_line": 140,
                    "score": 0.81,
                    "source": "project"
                }
            ],
            "total": 2
        });
        let result = format_semantic_search(&val);
        assert!(result.starts_with("2 results\n"));
        assert!(result.contains("0.92"));
        assert!(result.contains("0.81"));
        assert!(result.contains("src/tools/output.rs:35-50"));
        assert!(result.contains("src/tools/mod.rs:120-140"));
        assert!(result.contains("pub struct OutputGuard {"));
        assert!(result.contains("pub trait Tool {"));
    }

    #[test]
    fn semantic_search_single_result() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/main.rs",
                    "language": "rust",
                    "content": "fn main() {}",
                    "start_line": 1,
                    "end_line": 1,
                    "score": 0.95,
                    "source": "project"
                }
            ],
            "total": 1
        });
        let result = format_semantic_search(&val);
        assert!(result.starts_with("1 result\n"));
        // Should say "result" not "results"
        assert!(!result.starts_with("1 results"));
        assert!(result.contains("0.95"));
        assert!(result.contains("src/main.rs:1"));
    }

    #[test]
    fn semantic_search_empty() {
        let val = serde_json::json!({
            "results": [],
            "total": 0
        });
        assert_eq!(format_semantic_search(&val), "0 results");
    }

    #[test]
    fn semantic_search_missing_results() {
        let val = serde_json::json!({});
        assert_eq!(format_semantic_search(&val), "");
    }

    #[test]
    fn semantic_search_with_staleness() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/a.rs",
                    "content": "fn foo() {}",
                    "start_line": 1,
                    "end_line": 5,
                    "score": 0.9,
                    "source": "project"
                }
            ],
            "total": 1,
            "stale": true,
            "behind_commits": 5,
            "hint": "Index is behind HEAD. Run index_project to update."
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("5 commits behind HEAD"));
        assert!(result.contains("index_project"));
    }

    #[test]
    fn semantic_search_with_overflow() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/a.rs",
                    "content": "fn foo() {}",
                    "start_line": 1,
                    "end_line": 5,
                    "score": 0.9,
                    "source": "project"
                }
            ],
            "total": 50,
            "overflow": {
                "shown": 10,
                "total": 50,
                "hint": "Use detail_level='full' with offset for pagination"
            }
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("10 of 50"));
    }

    #[test]
    fn semantic_search_long_content_truncated() {
        let long_content = "a".repeat(80);
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/a.rs",
                    "content": long_content,
                    "start_line": 1,
                    "end_line": 10,
                    "score": 0.85,
                    "source": "project"
                }
            ],
            "total": 1
        });
        let result = format_semantic_search(&val);
        // Preview should be truncated to ~50 chars with "..."
        assert!(result.contains("..."));
        // Should not contain the full 80-char string
        assert!(!result.contains(&"a".repeat(80)));
    }

    #[test]
    fn semantic_search_score_alignment() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "a.rs",
                    "content": "short",
                    "start_line": 1, "end_line": 1,
                    "score": 0.9, "source": "project"
                },
                {
                    "file_path": "very/long/path/to/file.rs",
                    "content": "long path",
                    "start_line": 100, "end_line": 200,
                    "score": 0.85, "source": "project"
                }
            ],
            "total": 2
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("a.rs:1"));
        assert!(result.contains("very/long/path/to/file.rs:100-200"));
    }

    // --- format_read_file tests ---

    #[test]
    fn read_file_content_mode_basic() {
        let val = serde_json::json!({
            "content": "fn main() {\n    println!(\"hello\");\n}\n",
            "total_lines": 3,
            "source": "project"
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("3 lines\n"));
        assert!(result.contains("1| fn main() {"));
        assert!(result.contains("2|     println!(\"hello\");"));
        assert!(result.contains("3| }"));
    }

    #[test]
    fn read_file_content_mode_single_line() {
        let val = serde_json::json!({
            "content": "hello world",
            "total_lines": 1,
            "source": "project"
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("1 line\n"));
        assert!(!result.starts_with("1 lines"));
        assert!(result.contains("1| hello world"));
    }

    #[test]
    fn read_file_content_with_overflow() {
        let val = serde_json::json!({
            "content": "line1\nline2\n",
            "total_lines": 500,
            "source": "project",
            "overflow": { "shown": 200, "total": 500, "hint": "File has 500 lines. Use start_line/end_line to read specific ranges" }
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("500 lines\n"));
        assert!(result.contains("200 of 500"));
        assert!(result.contains("start_line/end_line"));
    }

    #[test]
    fn read_file_empty_content() {
        let val = serde_json::json!({
            "content": "",
            "total_lines": 0,
            "source": "project"
        });
        let result = format_read_file(&val);
        assert_eq!(result, "0 lines");
    }

    #[test]
    fn read_file_missing_content() {
        let val = serde_json::json!({});
        assert_eq!(format_read_file(&val), "");
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
            "hint": "Full file stored as @file_abc123. Query with: run_command(\"grep/sed/awk @file_abc123\")"
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("500 lines\n"));
        assert!(result.contains("Symbols:"));
        assert!(result.contains("Struct"));
        assert!(result.contains("OutputGuard"));
        assert!(result.contains("L35"));
        assert!(result.contains("Function"));
        assert!(result.contains("cap_items"));
        assert!(result.contains("L55"));
        assert!(result.contains("Buffer: @file_abc123"));
        assert!(result.contains("Full file stored as @file_abc123"));
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
        assert!(result.starts_with("200 lines (Markdown)\n"));
        assert!(result.contains("Headings:"));
        assert!(result.contains("# Title"));
        assert!(result.contains("## Section 1"));
        assert!(result.contains("## Section 2"));
        assert!(result.contains("Buffer: @file_xyz"));
    }

    #[test]
    fn read_file_config_summary() {
        let val = serde_json::json!({
            "type": "config",
            "line_count": 50,
            "preview": "[package]\nname = \"code-explorer\"\nversion = \"0.1.0\"",
            "file_id": "@file_cfg",
            "hint": "Full file stored as @file_cfg."
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("50 lines (Config)\n"));
        assert!(result.contains("Preview:"));
        assert!(result.contains("[package]"));
        assert!(result.contains("name = \"code-explorer\""));
        assert!(result.contains("Buffer: @file_cfg"));
    }

    #[test]
    fn read_file_generic_summary() {
        let val = serde_json::json!({
            "type": "generic",
            "line_count": 300,
            "head": "first line\nsecond line",
            "tail": "last line",
            "file_id": "@file_gen",
            "hint": "Full file stored as @file_gen."
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("300 lines\n"));
        assert!(result.contains("Head:"));
        assert!(result.contains("first line"));
        assert!(result.contains("second line"));
        assert!(result.contains("Tail:"));
        assert!(result.contains("last line"));
        assert!(result.contains("Buffer: @file_gen"));
    }

    #[test]
    fn read_file_source_summary_empty_symbols() {
        let val = serde_json::json!({
            "type": "source",
            "line_count": 100,
            "symbols": [],
            "file_id": "@file_empty",
            "hint": "Full file stored as @file_empty."
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("100 lines\n"));
        // No "Symbols:" section when empty
        assert!(!result.contains("Symbols:"));
        assert!(result.contains("Buffer: @file_empty"));
    }

    #[test]
    fn read_file_lineno_alignment() {
        // Content with >9 lines to test right-alignment of line numbers
        let content = (1..=12)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let val = serde_json::json!({
            "content": content,
            "total_lines": 12,
            "source": "project"
        });
        let result = format_read_file(&val);
        // Single-digit line numbers should be right-aligned (padded with space)
        assert!(result.contains(" 1| line 1"));
        assert!(result.contains(" 9| line 9"));
        // Double-digit line numbers should not be padded
        assert!(result.contains("10| line 10"));
        assert!(result.contains("12| line 12"));
    }

    #[test]
    fn read_file_source_summary_symbol_alignment() {
        let val = serde_json::json!({
            "type": "source",
            "line_count": 200,
            "symbols": [
                { "name": "Foo", "kind": "Struct", "line": 10 },
                { "name": "bar_function", "kind": "Function", "line": 50 }
            ],
            "file_id": "@file_align",
            "hint": "test"
        });
        let result = format_read_file(&val);
        // "Struct" should be padded to align with "Function"
        assert!(result.contains("Struct  "));
        assert!(result.contains("Function"));
    }

    #[test]
    fn read_file_markdown_empty_headings() {
        let val = serde_json::json!({
            "type": "markdown",
            "line_count": 50,
            "headings": [],
            "file_id": "@file_md",
            "hint": "test"
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("50 lines (Markdown)\n"));
        assert!(!result.contains("Headings:"));
        assert!(result.contains("Buffer: @file_md"));
    }

    #[test]
    fn find_references_basic() {
        let result = serde_json::json!({
            "references": [
                {"file": "src/foo.rs", "line": 10, "kind": "usage"},
                {"file": "src/bar.rs", "line": 20, "kind": "usage"},
                {"file": "src/foo.rs", "line": 30, "kind": "usage"}
            ],
            "total": 3
        });
        let text = format_find_references(&result);
        assert!(text.contains("3"), "should mention count");
        assert!(
            text.contains("refs") || text.contains("reference"),
            "should say refs or reference(s)"
        );
    }

    #[test]
    fn find_references_empty() {
        let result = serde_json::json!({ "references": [], "total": 0 });
        let text = format_find_references(&result);
        assert!(
            text.contains("No"),
            "should say 'No references found.', got: {}",
            text
        );
    }

    #[test]
    fn format_list_libraries_shows_names_and_status() {
        let result = serde_json::json!({
            "libraries": [
                {"name": "serde", "indexed": true},
                {"name": "tokio", "indexed": false}
            ]
        });
        let out = format_list_libraries(&result);
        assert!(out.contains("serde"), "should show library name, got: {out}");
        assert!(out.contains("tokio"), "should show library name, got: {out}");
        assert!(
            out.contains("indexed"),
            "should show index status, got: {out}"
        );
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn render_diff_header_contains_path() {
        let h = render_diff_header("edit_file", "src/server.rs");
        assert!(h.contains("edit_file"), "got: {h}");
        assert!(h.contains("src/server.rs"), "got: {h}");
        assert!(h.contains("\x1b[0m"), "no reset: {h}");
    }

    #[test]
    fn render_edit_diff_shows_minus_plus_lines() {
        let diff = render_edit_diff(
            "src/a.rs",
            "let old = 1;\nlet also_old = 2;",
            "let new = 3;",
            Some(88),
        );
        assert!(diff.contains("old"), "got: {diff}");
        assert!(diff.contains("new"), "got: {diff}");
        assert!(
            diff.contains("\x1b[31m") || diff.contains("\x1b[32m"),
            "no colors: {diff}"
        );
    }

    #[test]
    fn render_removal_diff_marks_all_lines_red() {
        let diff = render_removal_diff("src/a.rs", "fn old() {\n    1\n}", Some(10), "old");
        assert!(diff.contains("old"), "got: {diff}");
        assert!(diff.contains("\x1b[31m"), "no red: {diff}");
    }

    #[test]
    fn render_insert_diff_marks_all_lines_green() {
        let diff = render_insert_diff("src/a.rs", "fn new() {}", Some(42), "after", "my_sym");
        assert!(diff.contains("new"), "got: {diff}");
        assert!(diff.contains("\x1b[32m"), "no green: {diff}");
    }

    #[test]
    fn format_list_memories_shows_topic_names() {
        let result = serde_json::json!({
            "topics": ["architecture", "conventions", "gotchas"]
        });
        let out = format_list_memories(&result);
        assert!(out.contains("architecture"), "should list topic names");
        assert!(out.contains("conventions"), "should list topic names");
        assert!(out.contains("gotchas"), "should list topic names");
        assert!(out.contains('3'), "should include count");
    }

    #[test]
    fn format_list_memories_empty() {
        let result = serde_json::json!({ "topics": [] });
        let out = format_list_memories(&result);
        assert!(out.contains('0'), "should say 0 topics");
    }

    #[test]
    fn format_read_memory_shows_content() {
        let result = serde_json::json!({
            "topic": "architecture",
            "content": "## Layers\n\nAgent → Server → Tools"
        });
        let out = format_read_memory(&result);
        assert!(out.contains("architecture"), "should show topic");
        assert!(out.contains("Layers"), "should show content");
        assert!(out.contains("Agent → Server → Tools"), "should show full content");
    }

    #[test]
    fn format_read_memory_not_found_unchanged() {
        let result = serde_json::json!({ "topic": "missing", "content": null });
        let out = format_read_memory(&result);
        assert!(out.contains("not found"), "should say not found");
        assert!(out.contains("missing"), "should include topic name");
    }
}
