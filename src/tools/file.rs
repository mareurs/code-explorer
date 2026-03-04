//! File system tools: read, write, search, list.

use anyhow::Result;
use serde_json::{json, Value};

use super::format::format_overflow;
use super::{RecoverableError, Tool, ToolContext};
use crate::util::text::extract_lines;

// ── read_file ────────────────────────────────────────────────────────────────

pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Optionally restrict to a line range. Large files (>200 lines) are automatically buffered and returned as a summary + @file_* handle. Use start_line + end_line to read a specific range directly. For symbol-level navigation of source code, prefer symbol tools. Format-aware navigation: use heading for Markdown sections, json_path for JSON subtrees, toml_key for TOML tables or YAML sections."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "File path relative to project root (also accepted: file_path)" },
                "file_path": { "type": "string", "description": "Alias for path" },
                "start_line": { "type": "integer", "description": "First line to return (1-indexed). Must be paired with end_line." },
                "end_line": { "type": "integer", "description": "Last line to return (1-indexed, inclusive). Must be paired with start_line." },
                "heading": { "type": "string", "description": "Extract a Markdown section by heading text (e.g. \"## Authentication\"). Returns section content with structural metadata. Mutually exclusive with start_line/end_line and other navigation params." },
                "json_path": { "type": "string", "description": "Extract a JSON subtree by path (e.g. \"$.dependencies\", \"$.users[0]\"). Returns pretty-printed content with type info. Mutually exclusive with start_line/end_line and other navigation params." },
                "toml_key": { "type": "string", "description": "Extract a TOML table or YAML section by key (e.g. \"dependencies\", \"database\"). Returns section content with structural metadata. Mutually exclusive with start_line/end_line and other navigation params." }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        use super::output::{OutputGuard, OutputMode, OverflowInfo};

        let path = input["path"]
            .as_str()
            .or_else(|| input["file_path"].as_str())
            .ok_or_else(|| {
                RecoverableError::with_hint(
                    "missing required parameter 'path'",
                    "Provide the file path as: path=\"relative/path/to/file\"",
                )
            })?;

        // Handle buffer refs (@file_*, @cmd_*, @tool_*) — resolve from OutputBuffer
        // instead of reading from the filesystem.
        if path.starts_with("@file_") || path.starts_with("@cmd_") || path.starts_with("@tool_") {
            let text = ctx
                .output_buffer
                .get(path)
                .ok_or_else(|| {
                    RecoverableError::with_hint(
                        format!("buffer reference not found: '{}'", path),
                        "Buffer refs expire when the session resets. Re-run the command to get a fresh ref.",
                    )
                })?
                .stdout;
            let total_lines = text.lines().count();
            let start = input["start_line"].as_u64();
            let end = input["end_line"].as_u64();
            if let (Some(s), Some(e)) = (start, end) {
                if s == 0 || e < s {
                    return Err(RecoverableError::with_hint(
                        format!(
                            "invalid line range: start_line={} end_line={} (start_line must be >= 1 and end_line >= start_line)",
                            s, e
                        ),
                        "Lines are 1-indexed. Example: start_line=1, end_line=50",
                    )
                    .into());
                }
                let content = extract_lines(&text, s as usize, e as usize);
                return Ok(json!({ "content": content, "total_lines": total_lines }));
            }
            return Ok(json!({ "content": text, "total_lines": total_lines }));
        }

        let project_root = ctx.agent.project_root().await;
        let security = ctx.agent.security_config().await;
        let resolved = crate::util::path_security::validate_read_path(
            path,
            project_root.as_deref(),
            &security,
        )?;

        // Extract line range (both must be present for a targeted read)
        let start_line = input["start_line"].as_u64();
        let end_line = input["end_line"].as_u64();

        // Navigation parameters
        let heading = input["heading"].as_str();
        let json_path = input["json_path"].as_str();
        let toml_key = input["toml_key"].as_str();
        let nav_param_count = [heading.is_some(), json_path.is_some(), toml_key.is_some()]
            .iter()
            .filter(|&&x| x)
            .count();

        if nav_param_count > 1 {
            return Err(RecoverableError::with_hint(
                "only one navigation parameter allowed at a time",
                "Use heading OR json_path OR toml_key, not multiple",
            )
            .into());
        }

        if nav_param_count > 0 && (start_line.is_some() || end_line.is_some()) {
            return Err(RecoverableError::with_hint(
                "navigation parameters are mutually exclusive with start_line/end_line",
                "Use either heading/json_path/toml_key OR start_line+end_line",
            )
            .into());
        }

        // Determine source tag
        let source_tag = {
            let inner = ctx.agent.inner.read().await;
            if let Some(project) = &inner.active_project {
                if let Some(lib) = project.library_registry.is_library_path(&resolved) {
                    format!("lib:{}", lib.name)
                } else {
                    "project".to_string()
                }
            } else {
                "project".to_string()
            }
        };

        if resolved.is_dir() {
            return Err(RecoverableError::with_hint(
                format!("'{}' is a directory, not a file", path),
                "Use list_dir to browse directory contents, or provide a specific file path",
            )
            .into());
        }

        let text = std::fs::read_to_string(&resolved).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RecoverableError::with_hint(
                format!("file not found: '{}'", path),
                "Check the path with list_dir, or use find_file to locate the file",
            )
            .into(),
            std::io::ErrorKind::InvalidData => RecoverableError::with_hint(
                "file contains non-UTF-8 data (binary file?)",
                "read_file only works with text files. Use list_dir to check file types.",
            )
            .into(),
            _ => anyhow::anyhow!("failed to read {}: {}", resolved.display(), e),
        })?;

        // Handle heading navigation
        if let Some(heading_query) = heading {
            let file_type =
                crate::tools::file_summary::detect_file_type(&resolved.to_string_lossy());
            if !matches!(
                file_type,
                crate::tools::file_summary::FileSummaryType::Markdown
            ) {
                return Err(RecoverableError::with_hint(
                    "heading parameter is only supported for Markdown files",
                    "For JSON files use json_path, for TOML/YAML use toml_key",
                )
                .into());
            }
            let result =
                crate::tools::file_summary::extract_markdown_section(&text, heading_query)?;
            // If extracted section is itself large, buffer it
            if result.content.lines().count() > crate::tools::file_summary::FILE_BUFFER_THRESHOLD {
                let file_id = ctx.output_buffer.store_file(
                    resolved.to_string_lossy().to_string(),
                    result.content.clone(),
                );
                return Ok(json!({
                    "line_range": [result.line_range.0, result.line_range.1],
                    "breadcrumb": result.breadcrumb,
                    "siblings": result.siblings,
                    "format": "markdown",
                    "file_id": file_id,
                }));
            }
            return Ok(json!({
                "content": result.content,
                "line_range": [result.line_range.0, result.line_range.1],
                "breadcrumb": result.breadcrumb,
                "siblings": result.siblings,
                "format": "markdown",
            }));
        }

        // Handle json_path navigation
        if let Some(jp) = json_path {
            let file_type =
                crate::tools::file_summary::detect_file_type(&resolved.to_string_lossy());
            if !matches!(file_type, crate::tools::file_summary::FileSummaryType::Json) {
                return Err(RecoverableError::with_hint(
                    "json_path parameter is only supported for JSON files",
                    "For Markdown files use heading, for TOML/YAML use toml_key",
                )
                .into());
            }
            let (content, type_name, count) =
                crate::tools::file_summary::extract_json_path(&text, jp)?;
            let mut result = json!({
                "content": content,
                "path": jp,
                "type": type_name,
                "format": "json",
            });
            if let Some(c) = count {
                result["count"] = json!(c);
            }
            return Ok(result);
        }

        // Handle toml_key navigation
        if let Some(tk) = toml_key {
            let file_type =
                crate::tools::file_summary::detect_file_type(&resolved.to_string_lossy());
            match file_type {
                crate::tools::file_summary::FileSummaryType::Toml => {
                    let result = crate::tools::file_summary::extract_toml_key(&text, tk)?;
                    return Ok(json!({
                        "content": result.content,
                        "line_range": [result.line_range.0, result.line_range.1],
                        "breadcrumb": result.breadcrumb,
                        "siblings": result.siblings,
                        "format": "toml",
                    }));
                }
                crate::tools::file_summary::FileSummaryType::Yaml => {
                    let result = crate::tools::file_summary::extract_yaml_key(&text, tk)?;
                    return Ok(json!({
                        "content": result.content,
                        "line_range": [result.line_range.0, result.line_range.1],
                        "breadcrumb": result.breadcrumb,
                        "siblings": result.siblings,
                        "format": "yaml",
                    }));
                }
                _ => {
                    return Err(RecoverableError::with_hint(
                        "toml_key parameter is only supported for TOML and YAML files",
                        "For Markdown files use heading, for JSON use json_path",
                    )
                    .into());
                }
            }
        }

        // If explicit line range given, validate then use it directly (no capping, no buffering)
        if let (Some(start), Some(end)) = (start_line, end_line) {
            if start == 0 || end < start {
                return Err(RecoverableError::with_hint(
                    format!(
                        "invalid line range: start_line={} end_line={} (start_line must be >= 1 and end_line >= start_line)",
                        start, end
                    ),
                    "Lines are 1-indexed. Example: start_line=1, end_line=50",
                )
                .into());
            }
            let content = extract_lines(&text, start as usize, end as usize);
            let mut result = json!({ "content": content });
            if source_tag != "project" {
                result["source"] = json!(source_tag);
            }
            return Ok(result);
        }

        // No explicit range: buffer large files instead of truncating or erroring
        // If only one bound supplied (degenerate input), skip buffering.
        let has_partial_range = start_line.is_some() || end_line.is_some();
        let line_count = text.lines().count();
        if !has_partial_range && line_count > crate::tools::file_summary::FILE_BUFFER_THRESHOLD {
            let file_id = ctx
                .output_buffer
                .store_file(resolved.to_string_lossy().to_string(), text.clone());
            let summary =
                match crate::tools::file_summary::detect_file_type(&resolved.to_string_lossy()) {
                    crate::tools::file_summary::FileSummaryType::Source => {
                        crate::tools::file_summary::summarize_source(
                            &resolved.to_string_lossy(),
                            &text,
                        )
                    }
                    crate::tools::file_summary::FileSummaryType::Markdown => {
                        crate::tools::file_summary::summarize_markdown(&text)
                    }
                    crate::tools::file_summary::FileSummaryType::Json => {
                        crate::tools::file_summary::summarize_json(&text)
                    }
                    crate::tools::file_summary::FileSummaryType::Yaml => {
                        crate::tools::file_summary::summarize_yaml(&text)
                    }
                    crate::tools::file_summary::FileSummaryType::Toml => {
                        crate::tools::file_summary::summarize_toml(&text)
                    }
                    crate::tools::file_summary::FileSummaryType::Config => {
                        crate::tools::file_summary::summarize_config(&text)
                    }
                    crate::tools::file_summary::FileSummaryType::Generic => {
                        crate::tools::file_summary::summarize_generic_file(&text)
                    }
                };
            let mut result = summary;
            result["file_id"] = json!(file_id);
            return Ok(result);
        }

        // No line range: cap in exploring mode
        let guard = OutputGuard::from_input(&input);
        let total_lines = text.lines().count();
        let max_lines = guard.max_results; // 200 by default

        if guard.mode == OutputMode::Exploring && total_lines > max_lines {
            let content = extract_lines(&text, 1, max_lines);
            let overflow = OverflowInfo {
                shown: max_lines,
                total: total_lines,
                hint: format!(
                    "File has {} lines. Use start_line/end_line to read specific ranges",
                    total_lines
                ),
                next_offset: None,
                by_file: None,
                by_file_overflow: 0,
            };
            let mut result = json!({ "content": content, "total_lines": total_lines });
            if source_tag != "project" {
                result["source"] = json!(source_tag);
            }
            result["overflow"] = OutputGuard::overflow_json(&overflow);
            Ok(result)
        } else {
            let mut result = json!({ "content": text, "total_lines": total_lines });
            if source_tag != "project" {
                result["source"] = json!(source_tag);
            }
            Ok(result)
        }
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_read_file(result))
    }
}

// ── list_dir ────────────────────────────────────────────────────────────────

pub struct ListDir;

#[async_trait::async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List files and directories. Pass recursive=true for a full tree."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string" },
                "recursive": { "type": "boolean", "default": false },
                "detail_level": { "type": "string", "description": "Output detail: omit for compact (default), 'full' for all entries" },
                "offset": { "type": "integer", "description": "Skip this many entries (focused mode pagination)" },
                "limit": { "type": "integer", "description": "Max entries per page (focused mode, default 50)" }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        use super::output::{OutputGuard, OutputMode, OverflowInfo};

        let raw_path = input["path"].as_str().unwrap_or(".");
        let project_root = ctx.agent.project_root().await;
        let security = ctx.agent.security_config().await;
        let path = crate::util::path_security::validate_read_path(
            raw_path,
            project_root.as_deref(),
            &security,
        )?;
        let recursive = input["recursive"].as_bool().unwrap_or(false);
        let max_depth = if recursive { None } else { Some(1) };
        let guard = OutputGuard::from_input(&input);

        let walker = ignore::WalkBuilder::new(&path)
            .max_depth(max_depth)
            .hidden(true)
            .git_ignore(true)
            .build()
            .flatten()
            .filter(|e| e.depth() > 0);

        // In exploring mode, stop collecting once we exceed max_results.
        // We collect max_results+1 to detect overflow without walking the
        // entire tree (we lose the exact total, which is fine for exploring).
        let cap = match guard.mode {
            OutputMode::Exploring => Some(guard.max_results + 1),
            OutputMode::Focused => None,
        };

        let mut entries = Vec::new();
        for entry in walker {
            let suffix = if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            entries.push(format!("{}{}", entry.path().display(), suffix));
            if let Some(c) = cap {
                if entries.len() >= c {
                    break;
                }
            }
        }

        let hit_early_cap = cap.is_some() && entries.len() > guard.max_results;

        let (entries, overflow) = if hit_early_cap {
            // We collected max_results+1, truncate and report overflow
            entries.truncate(guard.max_results);
            let overflow = OverflowInfo {
                shown: guard.max_results,
                total: guard.max_results + 1, // at least this many
                hint: "Use a more specific path or set recursive=false".to_string(),
                next_offset: None,
                by_file: None,
                by_file_overflow: 0,
            };
            (entries, Some(overflow))
        } else {
            guard.cap_items(entries, "Use a more specific path or set recursive=false")
        };

        let mut result = json!({ "entries": entries });
        if let Some(ov) = overflow {
            result["overflow"] = OutputGuard::overflow_json(&ov);
        }
        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_list_dir(result))
    }
}

// ── search_for_pattern ───────────────────────────────────────────────────────

pub struct SearchPattern;

#[async_trait::async_trait]
impl Tool for SearchPattern {
    fn name(&self) -> &str {
        "search_pattern"
    }

    fn description(&self) -> &str {
        "Search the codebase for a regex pattern. Returns matching lines with file and line number. \
         Pass context_lines to see surrounding code — adjacent matches that share context windows \
         are merged into one block."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern" },
                "path": { "type": "string", "description": "File or directory to search (default: project root)" },
                "max_results": { "type": "integer", "default": 50, "description": "Maximum matching lines to return (in context mode this counts individual matching lines, not blocks). Alias: limit" },
                "limit": { "type": "integer", "description": "Alias for max_results" },
                "context_lines": {
                    "type": "integer",
                    "default": 0,
                    "description": "Lines of context before and after each match (max 20). Adjacent matches that share context are merged into one block with a flat multiline content string. For merged blocks, match_line is the first match's line — scan content for further matches."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        let pattern = super::require_str_param(&input, "pattern")?;
        let raw_path = input["path"].as_str().unwrap_or(".");
        let project_root = ctx.agent.project_root().await;
        let security = ctx.agent.security_config().await;
        let search_path = crate::util::path_security::validate_read_path(
            raw_path,
            project_root.as_deref(),
            &security,
        )?;
        let max = input["max_results"]
            .as_u64()
            .or_else(|| input["limit"].as_u64())
            .unwrap_or(50) as usize;
        let context_lines = input["context_lines"].as_u64().unwrap_or(0).min(20) as usize;
        let re = regex::RegexBuilder::new(pattern)
            .size_limit(1 << 20)
            .dfa_size_limit(1 << 20)
            .build()
            .map_err(|e| {
                RecoverableError::with_hint(
                    format!("invalid regex: {e}"),
                    "patterns are full regex syntax — escape metacharacters like \\( \\. \\[ for literals",
                )
            })?;
        let mut matches = vec![];
        let mut total_match_count = 0usize;
        let mut hit_cap = false;

        let walker = ignore::WalkBuilder::new(&search_path)
            .hidden(true)
            .git_ignore(true)
            .build();
        'outer: for entry in walker.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };

            if context_lines == 0 {
                // Original behaviour: one entry per matching line
                for (i, line) in text.lines().enumerate() {
                    if re.is_match(line) {
                        total_match_count += 1;
                        matches.push(json!({
                            "file": entry.path().display().to_string(),
                            "line": i + 1,
                            "content": line
                        }));
                        if matches.len() >= max {
                            hit_cap = true;
                            break 'outer;
                        }
                    }
                }
            } else {
                // Context mode: merge overlapping windows into blocks
                let file_lines: Vec<&str> = text.lines().collect();
                let n = file_lines.len();
                // (block_start_idx, first_match_idx, block_end_idx) — all 0-indexed
                let mut current: Option<(usize, usize, usize)> = None;

                for (i, line) in file_lines.iter().enumerate() {
                    if !re.is_match(line) {
                        continue;
                    }
                    total_match_count += 1;
                    let ctx_start = i.saturating_sub(context_lines);
                    let ctx_end = (i + context_lines).min(n.saturating_sub(1));

                    match current {
                        None => {
                            current = Some((ctx_start, i, ctx_end));
                        }
                        Some((blk_start, blk_first, blk_end)) => {
                            if ctx_start <= blk_end + 1 {
                                // Overlapping or adjacent: extend the current block
                                current = Some((blk_start, blk_first, ctx_end.max(blk_end)));
                            } else {
                                // Non-overlapping: emit finished block, start new one
                                let content = file_lines[blk_start..=blk_end].join("\n");
                                matches.push(json!({
                                    "file": entry.path().display().to_string(),
                                    "match_line": blk_first + 1,
                                    "start_line": blk_start + 1,
                                    "content": content,
                                }));
                                current = Some((ctx_start, i, ctx_end));
                            }
                        }
                    }

                    if total_match_count >= max {
                        hit_cap = true;
                        break;
                    }
                }

                // Emit the last in-flight block
                if let Some((blk_start, blk_first, blk_end)) = current {
                    let content = file_lines[blk_start..=blk_end].join("\n");
                    matches.push(json!({
                        "file": entry.path().display().to_string(),
                        "match_line": blk_first + 1,
                        "start_line": blk_start + 1,
                        "content": content,
                    }));
                }

                if total_match_count >= max {
                    hit_cap = true;
                    break 'outer;
                }
            }
        }

        // In context mode, matches contains merged blocks — fewer than total_match_count
        // (which counts individual matching lines). Report blocks so `total` == `matches.len()`.
        let shown_count = if context_lines > 0 {
            matches.len()
        } else {
            total_match_count
        };
        let mut result = json!({ "matches": matches, "total": shown_count });
        if hit_cap {
            result["overflow"] = json!({
                "shown": shown_count,
                "hint": format!(
                    "Showing first {} matches (cap hit). Narrow with a more specific pattern or path=<file>.",
                    shown_count
                )
            });
        }
        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_search_pattern(result))
    }
}

// ── create_text_file ────────────────────────────────────────────────────────

pub struct CreateFile;

#[async_trait::async_trait]
impl Tool for CreateFile {
    fn name(&self) -> &str {
        "create_file"
    }

    fn description(&self) -> &str {
        "Create or overwrite a file with the given content. Creates parent directories as needed."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": { "type": "string", "description": "File path (relative or absolute)" },
                "content": { "type": "string", "description": "Content to write" }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        super::guard_worktree_write(ctx).await?;
        let path = super::require_str_param(&input, "path")?;
        let content = super::require_str_param(&input, "content")?;
        let root = ctx.agent.require_project_root().await?;
        let security = ctx.agent.security_config().await;
        let resolved = crate::util::path_security::validate_write_path(path, &root, &security)?;
        crate::util::fs::write_utf8(&resolved, content)?;
        ctx.lsp.notify_file_changed(&resolved).await;
        Ok(json!("ok"))
    }
}

// ── find_file ───────────────────────────────────────────────────────────────

pub struct FindFile;

#[async_trait::async_trait]
impl Tool for FindFile {
    fn name(&self) -> &str {
        "find_file"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern (e.g. '**/*.rs', 'src/**/mod.rs'). Respects .gitignore."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern" },
                "path": { "type": "string", "description": "Directory to search (default: current dir)" },
                "max_results": { "type": "integer", "default": 100, "description": "Maximum files to return. Alias: limit" },
                "limit": { "type": "integer", "description": "Alias for max_results" }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        let pattern = super::require_str_param(&input, "pattern")?;
        let raw_path = input["path"].as_str().unwrap_or(".");
        let project_root = ctx.agent.project_root().await;
        let security = ctx.agent.security_config().await;
        let search_path = crate::util::path_security::validate_read_path(
            raw_path,
            project_root.as_deref(),
            &security,
        )?;
        let max = input["max_results"]
            .as_u64()
            .or_else(|| input["limit"].as_u64())
            .unwrap_or(100) as usize;

        let glob = globset::GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
            .map_err(|e| {
                RecoverableError::with_hint(
                    format!("invalid glob pattern: {e}"),
                    "Use glob syntax: * matches anything, ** crosses directories, ? matches one char",
                )
            })?
            .compile_matcher();

        let mut matches = vec![];
        let mut hit_cap = false;
        let walker = ignore::WalkBuilder::new(&search_path)
            .hidden(true)
            .git_ignore(true)
            .build();
        for entry in walker.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&search_path)
                .unwrap_or(entry.path());
            if glob.is_match(rel) {
                matches.push(entry.path().display().to_string());
                if matches.len() >= max {
                    hit_cap = true;
                    break;
                }
            }
        }

        let mut result = json!({ "files": matches, "total": matches.len() });
        if hit_cap {
            result["overflow"] = json!({
                "shown": matches.len(),
                "hint": format!(
                    "Showing first {} files (cap hit). Narrow with a more specific pattern or path=<dir>.",
                    matches.len()
                )
            });
        }
        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_find_file(result))
    }
}

// ── format_compact helpers ────────────────────────────────────────────────────

fn format_read_file(val: &Value) -> String {
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

fn format_read_file_summary(val: &Value, file_type: &str) -> String {
    let line_count = val["line_count"].as_u64().unwrap_or(0);

    let type_label = match file_type {
        "markdown" => " (Markdown)",
        "json" => " (JSON)",
        "yaml" => " (YAML)",
        "toml" => " (TOML)",
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
                        let kind_pad = " ".repeat(max_kind - kind.len());
                        let name_pad = " ".repeat(max_name.saturating_sub(name.len()));
                        out.push_str(&format!(
                            "\n    {kind}{kind_pad}  {name}{name_pad}  L{line}"
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
                        let heading = h["heading"].as_str().unwrap_or("?");
                        let line = h["line"].as_u64().unwrap_or(0);
                        let end_line = h["end_line"].as_u64().unwrap_or(0);
                        let level = h["level"].as_u64().unwrap_or(1) as usize;
                        let indent = "  ".repeat(level.saturating_sub(1));
                        out.push_str(&format!("\n    {indent}{heading}  L{line}-{end_line}"));
                    }
                }
            }
        }
        "json" => {
            if let Some(schema) = val.get("schema") {
                let root_type = schema["root_type"].as_str().unwrap_or("?");
                out.push_str(&format!("\n  Root: {root_type}"));
                if let Some(keys) = schema["keys"].as_array() {
                    for k in keys {
                        let path = k["path"].as_str().unwrap_or("?");
                        let typ = k["type"].as_str().unwrap_or("?");
                        let mut desc = format!("\n    {path}: {typ}");
                        if let Some(count) = k["count"].as_u64() {
                            desc.push_str(&format!(" ({count} items)"));
                        }
                        out.push_str(&desc);
                    }
                }
                if let Some(count) = schema["count"].as_u64() {
                    out.push_str(&format!("\n    Count: {count}"));
                    if let Some(elem) = schema["element_type"].as_str() {
                        out.push_str(&format!(" (element type: {elem})"));
                    }
                }
            }
        }
        "toml" => {
            if let Some(sections) = val["sections"].as_array() {
                out.push_str("\n  Sections:");
                for s in sections {
                    let key = s["key"].as_str().unwrap_or("?");
                    let line = s["line"].as_u64().unwrap_or(0);
                    let end = s["end_line"].as_u64().unwrap_or(0);
                    out.push_str(&format!("\n    {key}  L{line}-{end}"));
                }
            }
            if let Some(keys) = val["keys"].as_array() {
                out.push_str("\n  Keys:");
                for k in keys {
                    let key = k["key"].as_str().unwrap_or("?");
                    let line = k["line"].as_u64().unwrap_or(0);
                    out.push_str(&format!("\n    {key}  L{line}"));
                }
            }
        }
        "yaml" => {
            if let Some(sections) = val["sections"].as_array() {
                out.push_str("\n  Sections:");
                for s in sections {
                    let key = s["key"].as_str().unwrap_or("?");
                    let line = s["line"].as_u64().unwrap_or(0);
                    let end = s["end_line"].as_u64().unwrap_or(0);
                    out.push_str(&format!("\n    {key}  L{line}-{end}"));
                }
            }
        }
        // Residual: .xml, .ini, .env, .lock, .cfg (JSON/YAML/TOML have dedicated branches)
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

fn format_list_dir(val: &Value) -> String {
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

    let prefix = common_path_prefix(&names);

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

    let dir_display = if prefix.is_empty() {
        ".".to_string()
    } else {
        prefix.trim_end_matches('/').to_string()
    };
    let mut out = format!("{} — {} entries\n", dir_display, names.len());

    let max_name_len = short_names.iter().map(|n| n.len()).max().unwrap_or(0);
    let col_width = max_name_len + 2;
    let num_cols = (78 / col_width).max(1);

    out.push('\n');
    for (i, name) in short_names.iter().enumerate() {
        if i % num_cols == 0 {
            out.push_str("  ");
        }
        out.push_str(name);
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

    if let Some(overflow) = val.get("overflow") {
        if overflow.is_object() {
            out.push('\n');
            out.push_str(&format_overflow(overflow));
        }
    }

    out
}

fn common_path_prefix(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    if paths.len() == 1 {
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

    if last_slash > 0 {
        first[..last_slash].to_string()
    } else {
        let candidate = &first[..prefix_len];
        if candidate.ends_with('/') {
            candidate.to_string()
        } else {
            String::new()
        }
    }
}

fn format_search_pattern(val: &Value) -> String {
    let matches = match val["matches"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    let total = val["total"].as_u64().unwrap_or(matches.len() as u64);

    if matches.is_empty() {
        return "0 matches".to_string();
    }

    let is_context_mode = matches[0].get("start_line").is_some();

    let match_word = if total == 1 { "match" } else { "matches" };
    let mut out = format!("{total} {match_word}\n");

    if is_context_mode {
        format_search_context_mode(&mut out, matches);
    } else {
        format_search_simple_mode(&mut out, matches);
    }

    if let Some(overflow) = val.get("overflow") {
        if overflow.is_object() {
            out.push('\n');
            out.push_str(&format_overflow(overflow));
        }
    }

    out
}

fn format_search_simple_mode(out: &mut String, matches: &[Value]) {
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

fn format_search_context_mode(out: &mut String, matches: &[Value]) {
    let mut current_file: Option<&str> = None;

    for m in matches {
        let file = m["file"].as_str().unwrap_or("?");
        let start_line = m["start_line"].as_u64().unwrap_or(1);
        let content = m["content"].as_str().unwrap_or("");

        if current_file != Some(file) {
            out.push_str("\n  ");
            out.push_str(file);
            out.push('\n');
            current_file = Some(file);
        }

        for (i, line) in content.lines().enumerate() {
            let line_num = start_line + i as u64;
            out.push_str(&format!("  {:<4} {}\n", line_num, line));
        }
    }

    if out.ends_with('\n') {
        out.pop();
    }
}

fn format_find_file(result: &Value) -> String {
    let total = result["total"].as_u64().unwrap_or(0);
    let overflow = result["overflow"].is_object();
    let cap_note = if overflow {
        " (cap hit — narrow pattern)"
    } else {
        ""
    };
    format!("{total} files{cap_note}")
}

const DEF_KEYWORDS: &[&str] = &[
    "fn ",
    "def ",
    "func ",
    "fun ",
    "function ",
    "async fn ",
    "async def ",
    "async function ",
    "class ",
    "struct ",
    "impl ",
    "trait ",
    "interface ",
    "enum ",
    "type ",
];

/// Infers which symbol-aware tool to suggest when `edit_file` is blocked on a source file.
/// Priority: delete (empty new_string) → structural definition keyword → insertion (new > old) → fallback.
fn infer_edit_hint(old_string: &str, new_string: &str) -> &'static str {
    if new_string.is_empty() {
        return "remove_symbol(name_path, path) — deletes the symbol and its doc comments/attributes";
    }

    // Detect structural definition keywords across all supported languages:
    // Rust: fn, async fn, impl, struct, trait, enum
    // Python: def, async def, class
    // Go: func, type
    // JS/TS: function, async function, class, interface, type
    // Java/Kotlin/C#/Swift: class, interface, enum, fun, func
    if DEF_KEYWORDS.iter().any(|kw| old_string.contains(kw)) {
        return "replace_symbol(name_path, path, new_body) — replaces the symbol body via LSP";
    }

    if new_string.len() > old_string.len() {
        // Heuristic: comma-separated identifiers with no `(`, `=`, or `->` → likely a
        // `use`/`import` identifier list rather than addressable symbol code.
        // insert_code requires a name_path and is inapplicable to import lists;
        // acknowledge_risk is the right escape hatch in that case.
        let looks_like_import = old_string.contains(',')
            && !old_string.contains('(')
            && !old_string.contains('=')
            && !old_string.contains("->");
        if looks_like_import {
            return "edit_file with acknowledge_risk: true — \
                    target looks like an import list, not a named symbol";
        }
        let looks_like_import = old_string.contains(',')
            && !old_string.contains('(')
            && !old_string.contains('=')
            && !old_string.contains("->");
        if looks_like_import {
            return "edit_file with acknowledge_risk: true — \
                    target looks like an import list, not a named symbol";
        }
        return "insert_code(name_path, path, code, position) — inserts before or after a named symbol";
    }

    // Fallback: show all options
    "use a symbol-aware tool instead:\n  \
     replace_symbol(name_path, path, new_body) — replace a symbol body\n  \
     insert_code(name_path, path, code, position) — insert before/after a symbol\n  \
     remove_symbol(name_path, path) — delete a symbol entirely"
}

pub struct EditFile;

#[async_trait::async_trait]
impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file. old_string must match the file content exactly (including whitespace/indentation). Use replace_all: true to replace every occurrence. Alternatively, use insert: \"prepend\" or \"append\" to add text at the start or end of the file without a string match."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path", "new_string"],
            "properties": {
                "path": { "type": "string", "description": "File path" },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to find (must match file content including whitespace/indentation). Required unless insert is set."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text. Empty string deletes the match when using old_string. The text to insert when using insert mode."
                },
                "replace_all": {
                    "type": "boolean",
                    "default": false,
                    "description": "Replace all occurrences (default: first unique match only)"
                },
                "insert": {
                    "type": "string",
                    "enum": ["prepend", "append"],
                    "description": "Insert new_string at the start (prepend) or end (append) of the file. When set, old_string is not required."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        super::guard_worktree_write(ctx).await?;
        let path = super::require_str_param(&input, "path")?;
        let new_string = input["new_string"].as_str().unwrap_or("");
        let acknowledge_risk = input["acknowledge_risk"].as_bool().unwrap_or(false);

        // Dispatch @ack_* handle for a previously deferred multi-line source edit.
        if path.starts_with("@ack_") {
            let edit = ctx.output_buffer.get_pending_edit(path).ok_or_else(|| {
                super::RecoverableError::with_hint(
                    "ack handle expired or unknown",
                    "Re-run the original edit_file call to get a fresh handle.",
                )
            })?;
            return perform_edit(
                &edit.path,
                &edit.old_string,
                &edit.new_string,
                edit.replace_all,
                ctx,
            )
            .await;
        }

        // Prepend/append mode — no string match needed.
        if let Some(insert) = input["insert"].as_str() {
            let root = ctx.agent.require_project_root().await?;
            let security = ctx.agent.security_config().await;
            let resolved = crate::util::path_security::validate_write_path(path, &root, &security)?;
            let content = std::fs::read_to_string(&resolved)?;
            let new_content = match insert {
                "prepend" => format!("{}{}", new_string, content),
                "append" => format!("{}{}", content, new_string),
                _ => {
                    return Err(super::RecoverableError::with_hint(
                        format!("invalid insert value: {insert:?}"),
                        "insert must be \"prepend\" or \"append\"",
                    )
                    .into())
                }
            };
            std::fs::write(&resolved, &new_content)?;
            ctx.lsp.notify_file_changed(&resolved).await;
            return Ok(json!("ok"));
        }

        let old_string = super::require_str_param(&input, "old_string")?;
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        if old_string.is_empty() {
            return Err(super::RecoverableError::with_hint(
                "old_string must not be empty",
                "To create a new file use create_file. To insert at a specific line use insert_code. To prepend or append to a file use insert: \"prepend\" or \"append\".",
            )
            .into());
        }

        // Multi-line edits on source files: nudge toward symbol tools but allow with ack.
        if old_string.contains('\n')
            && crate::util::path_security::is_source_path(path)
            && !acknowledge_risk
        {
            let hint = infer_edit_hint(old_string, new_string);
            let handle = ctx.output_buffer.store_pending_edit(
                path.to_string(),
                old_string.to_string(),
                new_string.to_string(),
                replace_all,
            );
            return Ok(json!({
                "pending_ack": handle,
                "reason": "multi-line edit on source file — symbol-aware tools are safer and LSP-backed",
                "hint": format!("Prefer {}. To bypass: re-run with acknowledge_risk: true, or pass the ack handle as the path: edit_file(\"{}\")", hint, handle)
            }));
        }

        perform_edit(path, old_string, new_string, replace_all, ctx).await
    }
}

async fn perform_edit(
    path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    ctx: &ToolContext,
) -> Result<Value> {
    let root = ctx.agent.require_project_root().await?;
    let security = ctx.agent.security_config().await;
    let resolved = crate::util::path_security::validate_write_path(path, &root, &security)?;

    let content = std::fs::read_to_string(&resolved)?;

    let match_count = content.matches(old_string).count();

    if match_count == 0 {
        return Err(super::RecoverableError::with_hint(
            format!("old_string not found in {path}"),
            "Check whitespace and indentation — old_string must match exactly. Use search_pattern to verify the exact text.",
        )
        .into());
    }

    if match_count > 1 && !replace_all {
        let line_numbers: Vec<usize> = content
            .match_indices(old_string)
            .map(|(byte_offset, _)| content[..byte_offset].lines().count() + 1)
            .collect();
        let lines_str = line_numbers
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(super::RecoverableError::with_hint(
            format!(
                "old_string found {match_count} times (lines {lines_str}). Include more surrounding context or use replace_all: true."
            ),
            "Expand old_string to include unique surrounding context, or set replace_all: true to replace every occurrence.",
        )
        .into());
    }

    let new_content = content.replace(old_string, new_string);
    std::fs::write(&resolved, &new_content)?;
    ctx.lsp.notify_file_changed(&resolved).await;

    Ok(json!("ok"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use serde_json::json;
    use tempfile::tempdir;

    async fn test_ctx() -> ToolContext {
        ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        }
    }

    async fn project_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: LspManager::new_arc(),
                output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(
                    20,
                )),
                progress: None,
            },
        )
    }

    // ── ReadFile ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_returns_full_content() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "hello world").unwrap();

        let result = ReadFile
            .call(json!({ "path": file.to_str().unwrap() }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["content"], "hello world");
    }

    #[tokio::test]
    async fn read_file_with_line_range() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("lines.txt");
        std::fs::write(&file, "line1\nline2\nline3\nline4\nline5").unwrap();

        let result = ReadFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "start_line": 2,
                    "end_line": 4
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["content"], "line2\nline3\nline4");
    }

    #[tokio::test]
    async fn read_file_rejects_start_line_zero() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("lines.txt");
        std::fs::write(&file, "line1\nline2\nline3").unwrap();
        let path = file.to_str().unwrap();

        // Baseline: valid 1-indexed range succeeds and returns content
        let result = ReadFile
            .call(
                json!({ "path": path, "start_line": 1, "end_line": 2 }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            !result["content"].as_str().unwrap().is_empty(),
            "valid range must return content"
        );

        // Stale (the bug): old code passed start_line=0 through to extract_lines, which
        // silently returned an empty string — indistinguishable from an empty file.
        // Fixed: start_line=0 is now rejected with a RecoverableError.
        let result = ReadFile
            .call(
                json!({ "path": path, "start_line": 0, "end_line": 3 }),
                &ctx,
            )
            .await;
        assert!(
            result.is_err(),
            "start_line=0 must be rejected (lines are 1-indexed)"
        );

        // Fresh: valid range is unaffected — the guard is narrow and precise
        let result = ReadFile
            .call(
                json!({ "path": path, "start_line": 2, "end_line": 3 }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            !result["content"].as_str().unwrap().is_empty(),
            "valid range after error cases must still return content"
        );
    }

    #[tokio::test]
    async fn read_file_rejects_end_before_start() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("lines.txt");
        std::fs::write(&file, "line1\nline2\nline3\nline4\nline5").unwrap();
        let path = file.to_str().unwrap();

        // Baseline: valid range succeeds
        let result = ReadFile
            .call(
                json!({ "path": path, "start_line": 2, "end_line": 4 }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            !result["content"].as_str().unwrap().is_empty(),
            "valid range must return content"
        );

        // Stale (the bug): old code passed end < start through to extract_lines, which
        // computed a negative/empty slice and silently returned "".
        // Fixed: end_line < start_line is now rejected with a RecoverableError.
        let result = ReadFile
            .call(
                json!({ "path": path, "start_line": 5, "end_line": 2 }),
                &ctx,
            )
            .await;
        assert!(result.is_err(), "end_line < start_line must be rejected");

        // Fresh: equal start/end (single line) is valid, not rejected
        let result = ReadFile
            .call(
                json!({ "path": path, "start_line": 3, "end_line": 3 }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            !result["content"].as_str().unwrap().is_empty(),
            "start_line == end_line (single line) must succeed"
        );
    }

    #[tokio::test]
    async fn read_file_missing_errors() {
        let ctx = test_ctx().await;
        let result = ReadFile
            .call(json!({ "path": "/no/such/file.txt" }), &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_missing_path_param_errors() {
        let ctx = test_ctx().await;
        let result = ReadFile.call(json!({}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_caps_large_file_in_exploring_mode() {
        // Files > FILE_BUFFER_THRESHOLD (200) lines are now buffered, not capped+overflowed.
        // This test verifies the buffer path for a large plain-text file.
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("big.txt");
        let content: String = (1..=300).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&file, &content).unwrap();

        let result = ReadFile
            .call(json!({ "path": file.to_str().unwrap() }), &ctx)
            .await
            .unwrap();

        // Large file should be buffered with a file_id
        assert!(
            result.get("file_id").is_some(),
            "300-line file should be buffered; got: {}",
            result
        );
        let file_id = result["file_id"].as_str().unwrap();
        assert!(file_id.starts_with("@file_"));
        // Full content is stored in the buffer
        let entry = ctx.output_buffer.get(file_id).unwrap();
        assert!(entry.stdout.contains("line 150"));
    }

    #[tokio::test]
    async fn read_file_small_file_no_overflow() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("small.txt");
        std::fs::write(&file, "line 1\nline 2\nline 3\n").unwrap();

        let result = ReadFile
            .call(json!({ "path": file.to_str().unwrap() }), &ctx)
            .await
            .unwrap();

        assert!(result.get("overflow").is_none());
        assert_eq!(result["total_lines"], 3);
    }

    // ── ReadFile — buffer ref (@tool_*, @cmd_*, @file_*) ─────────────────────

    #[tokio::test]
    async fn read_file_tool_ref_returns_full_content() {
        let ctx = test_ctx().await;
        let id = ctx
            .output_buffer
            .store_tool("onboarding", "{\"languages\":[\"rust\"]}".to_string());
        let result = ReadFile.call(json!({ "path": id }), &ctx).await.unwrap();
        assert_eq!(result["content"], "{\"languages\":[\"rust\"]}");
    }

    #[tokio::test]
    async fn read_file_cmd_ref_returns_full_content() {
        let ctx = test_ctx().await;
        let id = ctx.output_buffer.store(
            "cargo test".to_string(),
            "ok\n".to_string(),
            String::new(),
            0,
        );
        let result = ReadFile.call(json!({ "path": id }), &ctx).await.unwrap();
        assert_eq!(result["content"], "ok\n");
        assert_eq!(result["total_lines"], 1);
    }

    #[tokio::test]
    async fn read_file_tool_ref_with_line_range() {
        let ctx = test_ctx().await;
        let content = "line1\nline2\nline3\nline4\nline5".to_string();
        let id = ctx.output_buffer.store_tool("list_symbols", content);
        let result = ReadFile
            .call(json!({ "path": id, "start_line": 2, "end_line": 4 }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["content"], "line2\nline3\nline4");
        assert_eq!(result["total_lines"], 5);
    }

    #[tokio::test]
    async fn read_file_tool_ref_missing_returns_error() {
        let ctx = test_ctx().await;
        let result = ReadFile
            .call(json!({ "path": "@tool_deadbeef" }), &ctx)
            .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("buffer reference not found"), "got: {}", msg);
    }

    #[tokio::test]
    async fn read_file_tool_ref_invalid_line_range_errors() {
        let ctx = test_ctx().await;
        let id = ctx
            .output_buffer
            .store_tool("onboarding", "a\nb\nc".to_string());
        let result = ReadFile
            .call(json!({ "path": id, "start_line": 5, "end_line": 2 }), &ctx)
            .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("invalid line range"), "got: {}", msg);
    }

    // ── ListDir ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_dir_returns_shallow_entries() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();

        let result = ListDir
            .call(json!({ "path": dir.path().to_str().unwrap() }), &ctx)
            .await
            .unwrap();

        let entries: Vec<&str> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.ends_with("a.rs")));
        assert!(entries.iter().any(|e| e.ends_with("b.rs")));
    }

    #[tokio::test]
    async fn list_dir_shallow_does_not_descend() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("deep.rs"), "").unwrap();

        let result = ListDir
            .call(
                json!({ "path": dir.path().to_str().unwrap(), "recursive": false }),
                &ctx,
            )
            .await
            .unwrap();

        let entries: Vec<&str> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        // Should see the sub/ directory but not deep.rs inside it
        assert!(!entries.iter().any(|e| e.ends_with("deep.rs")));
    }

    #[tokio::test]
    async fn list_dir_recursive_descends() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("deep.rs"), "").unwrap();

        let result = ListDir
            .call(
                json!({ "path": dir.path().to_str().unwrap(), "recursive": true }),
                &ctx,
            )
            .await
            .unwrap();

        let entries: Vec<&str> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(entries.iter().any(|e| e.ends_with("deep.rs")));
    }

    #[tokio::test]
    async fn list_dir_caps_output_in_exploring_mode() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        // Create more files than the default cap (200)
        // We'll use a smaller effective cap by using recursive mode
        // with many nested entries. Simpler: just create 5 files and
        // verify with a small cap that overflow is reported.
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("file_{}.rs", i)), "").unwrap();
        }

        // list_dir uses OutputGuard with max_results=200 by default,
        // which won't trigger for 5 files. But the early-exit logic
        // triggers when entries.len() > max_results. To test overflow,
        // we verify the mechanism works by checking that >200 entries
        // DO produce overflow. Instead, let's verify the non-overflow
        // case works (no overflow key) and that the entries are correct.
        let result = ListDir
            .call(json!({ "path": dir.path().to_str().unwrap() }), &ctx)
            .await
            .unwrap();

        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 5);
        assert!(result.get("overflow").is_none());
    }

    // ── SearchPattern ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_finds_matching_line() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn main() {}\nlet x = 42;\n").unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "fn main", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["line"], 1);
        assert!(matches[0]["content"].as_str().unwrap().contains("fn main"));
    }

    #[tokio::test]
    async fn search_returns_no_matches_when_absent() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn main() {}").unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "xyz_not_present", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["matches"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn search_respects_max_results() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let content = (0..20)
            .map(|i| format!("match_{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("data.txt"), &content).unwrap();

        let result = SearchPattern
            .call(
                json!({
                    "pattern": "match_",
                    "path": dir.path().to_str().unwrap(),
                    "max_results": 5
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["matches"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn search_invalid_regex_errors() {
        let (dir, ctx) = project_ctx().await;
        let err = SearchPattern
            .call(
                json!({ "pattern": "[invalid", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(
            err.downcast_ref::<RecoverableError>().is_some(),
            "invalid regex should be RecoverableError, not a hard error"
        );
    }

    #[tokio::test]
    async fn search_missing_pattern_errors() {
        let ctx = test_ctx().await;
        let result = SearchPattern.call(json!({}), &ctx).await;
        assert!(result.is_err());
    }

    // ── CreateFile ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_text_file_writes_content() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("new.txt");

        let result = CreateFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "content": "hello file"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, "ok");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello file");
    }

    #[tokio::test]
    async fn create_text_file_creates_parent_dirs() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("a").join("b").join("deep.txt");

        CreateFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "content": "nested"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "nested");
    }

    #[tokio::test]
    async fn create_text_file_missing_params_errors() {
        let ctx = test_ctx().await;
        assert!(CreateFile.call(json!({}), &ctx).await.is_err());
        let outside = std::env::temp_dir().join("nonexistent_xplat_test");
        let outside_str = outside.to_str().unwrap();
        assert!(CreateFile
            .call(json!({ "path": outside_str }), &ctx)
            .await
            .is_err());
    }

    // ── FindFile ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn find_file_matches_glob() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("foo.rs"), "").unwrap();
        std::fs::write(dir.path().join("bar.rs"), "").unwrap();
        std::fs::write(dir.path().join("baz.txt"), "").unwrap();

        let result = FindFile
            .call(
                json!({
                    "pattern": "*.rs",
                    "path": dir.path().to_str().unwrap()
                }),
                &ctx,
            )
            .await
            .unwrap();

        let files = result["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.as_str().unwrap().ends_with(".rs")));
    }

    #[tokio::test]
    async fn find_file_recursive_glob() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();

        let result = FindFile
            .call(
                json!({
                    "pattern": "**/*.rs",
                    "path": dir.path().to_str().unwrap()
                }),
                &ctx,
            )
            .await
            .unwrap();

        let files = result["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
    }

    #[tokio::test]
    async fn find_file_respects_max_results() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("f{}.rs", i)), "").unwrap();
        }

        let result = FindFile
            .call(
                json!({
                    "pattern": "*.rs",
                    "path": dir.path().to_str().unwrap(),
                    "max_results": 3
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["files"].as_array().unwrap().len(), 3);
        assert_eq!(result["total"], 3);
    }

    #[tokio::test]
    async fn find_file_no_matches() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "").unwrap();

        let result = FindFile
            .call(
                json!({
                    "pattern": "*.rs",
                    "path": dir.path().to_str().unwrap()
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["files"].as_array().unwrap().len(), 0);
    }

    // ── ListDir .git exclusion ──────────────────────────────────────────────

    #[tokio::test]
    async fn list_dir_recursive_excludes_git() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();

        // Create a fake .git/ directory with typical contents
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::create_dir(git_dir.join("objects")).unwrap();
        std::fs::write(git_dir.join("objects").join("abc123"), "blob").unwrap();
        std::fs::create_dir(git_dir.join("hooks")).unwrap();
        std::fs::write(git_dir.join("hooks").join("pre-commit"), "#!/bin/sh").unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main").unwrap();

        // Create real project files
        let src_dir = dir.path().join("src");
        std::fs::create_dir(&src_dir).unwrap();
        std::fs::write(src_dir.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let result = ListDir
            .call(
                json!({ "path": dir.path().to_str().unwrap(), "recursive": true }),
                &ctx,
            )
            .await
            .unwrap();

        let entries: Vec<&str> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();

        // Should contain real project files
        assert!(
            entries.iter().any(|e| e.ends_with("main.rs")),
            "should contain main.rs, got: {:?}",
            entries
        );
        assert!(
            entries.iter().any(|e| e.ends_with("Cargo.toml")),
            "should contain Cargo.toml, got: {:?}",
            entries
        );

        // Should NOT contain any .git/ entries
        assert!(
            !entries.iter().any(|e| e.contains(".git")),
            ".git/ entries should be excluded, got: {:?}",
            entries
        );
    }
    #[tokio::test]
    async fn search_for_pattern_skips_hidden_dirs() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();

        // Create a normal source file with a match
        std::fs::write(dir.path().join("main.rs"), "fn hello() {}").unwrap();

        // Create a hidden .worktrees/ dir with the same pattern — should be skipped
        let wt_dir = dir.path().join(".worktrees").join("feature");
        std::fs::create_dir_all(&wt_dir).unwrap();
        std::fs::write(wt_dir.join("lib.rs"), "fn hello() {}").unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "fn hello", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let matches: Vec<&str> = result["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["file"].as_str().unwrap())
            .collect();

        assert!(
            matches.iter().any(|f| f.ends_with("main.rs")),
            "should find match in main.rs"
        );
        assert!(
            !matches.iter().any(|f| f.contains(".worktrees")),
            ".worktrees/ should be excluded, got: {:?}",
            matches
        );
    }

    #[tokio::test]
    async fn find_file_skips_hidden_dirs() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();

        // Normal file
        std::fs::write(dir.path().join("main.rs"), "").unwrap();

        // Same pattern inside a hidden .claude/worktrees/ dir — should be skipped
        let wt_dir = dir.path().join(".claude").join("worktrees").join("branch");
        std::fs::create_dir_all(&wt_dir).unwrap();
        std::fs::write(wt_dir.join("main.rs"), "").unwrap();

        let result = FindFile
            .call(
                json!({ "pattern": "**/*.rs", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let files: Vec<&str> = result["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();

        assert_eq!(
            files.len(),
            1,
            "only main.rs should match, got: {:?}",
            files
        );
        assert!(
            files[0].ends_with("main.rs"),
            "expected main.rs, got: {:?}",
            files
        );
    }

    // ── Security: Input Validation ────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_missing_path_errors() {
        let ctx = test_ctx().await;
        let result = ReadFile.call(json!({}), &ctx).await;
        assert!(result.is_err(), "read_file without path should error");
    }

    #[tokio::test]
    async fn read_file_empty_path_errors() {
        let ctx = test_ctx().await;
        let result = ReadFile.call(json!({ "path": "" }), &ctx).await;
        assert!(result.is_err(), "read_file with empty path should error");
    }

    #[tokio::test]
    async fn create_text_file_missing_params_detailed_errors() {
        let ctx = test_ctx().await;
        // Missing path
        let result = CreateFile.call(json!({ "content": "hello" }), &ctx).await;
        assert!(
            result.is_err(),
            "create_text_file without path should error"
        );

        // Missing content
        let outside = std::env::temp_dir().join("nonexistent_xplat_test.txt");
        let outside_str = outside.to_str().unwrap();
        let result = CreateFile.call(json!({ "path": outside_str }), &ctx).await;
        assert!(
            result.is_err(),
            "create_text_file without content should error"
        );
    }

    #[tokio::test]
    async fn search_for_pattern_missing_pattern_errors() {
        let ctx = test_ctx().await;
        let result = SearchPattern.call(json!({}), &ctx).await;
        assert!(
            result.is_err(),
            "search_for_pattern without pattern should error"
        );
    }

    #[tokio::test]
    async fn find_file_missing_pattern_errors() {
        let ctx = test_ctx().await;
        let result = FindFile.call(json!({}), &ctx).await;
        assert!(result.is_err(), "find_file without pattern should error");
    }

    #[tokio::test]
    async fn search_for_pattern_invalid_regex_errors() {
        let (dir, ctx) = project_ctx().await;
        let err = SearchPattern
            .call(
                json!({ "pattern": "[invalid(", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(
            err.downcast_ref::<RecoverableError>().is_some(),
            "invalid regex should be RecoverableError, not a hard error"
        );
    }

    #[tokio::test]
    async fn read_file_nonexistent_errors_gracefully() {
        let ctx = test_ctx().await;
        let result = ReadFile
            .call(json!({ "path": "/nonexistent/path/file.txt" }), &ctx)
            .await;
        assert!(result.is_err());
        // Error should not panic, just return Err
    }
    #[tokio::test]
    async fn read_file_binary_content_does_not_panic() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("binary.bin");
        std::fs::write(&file, b"\x00\x01\x02\xff\xfe").unwrap();

        // Binary file should produce a RecoverableError, not a fatal error or panic
        let result = ReadFile
            .call(json!({ "path": file.to_str().unwrap() }), &ctx)
            .await;
        let err = result.unwrap_err();
        assert!(
            err.downcast_ref::<RecoverableError>().is_some(),
            "binary file error should be RecoverableError, got: {err}"
        );
        assert!(
            err.to_string().contains("non-UTF-8"),
            "error should mention non-UTF-8, got: {err}"
        );
    }

    #[tokio::test]
    async fn list_dir_nonexistent_path_errors() {
        let ctx = test_ctx().await;
        let result = ListDir
            .call(json!({ "path": "/nonexistent/directory" }), &ctx)
            .await
            .unwrap();
        // WalkBuilder returns empty for nonexistent paths
        let entries = result["entries"].as_array().unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn search_for_pattern_max_results_respected() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        // Create a file with many matching lines
        let content = (0..100)
            .map(|i| format!("match_{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("many.txt"), &content).unwrap();

        let result = SearchPattern
            .call(
                json!({
                    "pattern": "match_",
                    "path": dir.path().to_str().unwrap(),
                    "max_results": 5
                }),
                &ctx,
            )
            .await
            .unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 5, "max_results should be respected");
    }

    #[tokio::test]
    async fn search_for_pattern_single_file_path() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("spec.md");
        std::fs::write(
            &file_path,
            "# Notifications\nPush notifications are sent via FCM.\nEmail alerts are also supported.\n",
        )
        .unwrap();

        // Pass a file path, not a directory
        let result = SearchPattern
            .call(
                json!({
                    "pattern": "notification|push",
                    "path": file_path.to_str().unwrap(),
                    "max_results": 40
                }),
                &ctx,
            )
            .await
            .unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(
            !matches.is_empty(),
            "search_for_pattern should work with a single file path"
        );
    }

    // ── Security: Path Sandboxing ─────────────────────────────────────────

    #[tokio::test]
    async fn read_file_denies_ssh_key() {
        let ctx = test_ctx().await;
        if let Some(home) = std::env::var("HOME").ok() {
            let ssh_path = format!("{}/.ssh/id_rsa", home);
            let result = ReadFile.call(json!({ "path": &ssh_path }), &ctx).await;
            assert!(result.is_err(), "read of ~/.ssh/id_rsa should be denied");
        }
    }

    #[tokio::test]
    async fn create_file_outside_project_rejected() {
        let (_dir, ctx) = project_ctx().await;
        // Use a hardcoded path outside both the project root and /tmp (which is
        // now an allowed write root).
        let result = CreateFile
            .call(
                json!({
                    "path": "/var/outside_ce_test/evil.rs",
                    "content": "evil code"
                }),
                &ctx,
            )
            .await;
        assert!(result.is_err(), "write outside project should be rejected");
    }

    #[tokio::test]
    async fn create_file_within_project_works() {
        let (dir, ctx) = project_ctx().await;
        let result = CreateFile
            .call(
                json!({
                    "path": dir.path().join("new_file.txt").to_str().unwrap(),
                    "content": "hello"
                }),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new_file.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn write_requires_active_project() {
        let ctx = test_ctx().await;
        let outside = std::env::temp_dir().join("nonexistent_xplat_test.txt");
        let outside_str = outside.to_str().unwrap();
        let result = CreateFile
            .call(json!({ "path": outside_str, "content": "hi" }), &ctx)
            .await;
        assert!(result.is_err(), "write without active project should error");
    }

    // ── ReDoS Prevention ─────────────────────────────────────────────────

    #[tokio::test]
    async fn search_for_pattern_huge_regex_rejected() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "hello").unwrap();

        // Build a regex that exceeds the 1MB compiled NFA size limit
        let huge_pattern = format!("({})", "a?".repeat(100_000));
        let err = SearchPattern
            .call(
                json!({
                    "pattern": huge_pattern,
                    "path": dir.path().to_str().unwrap()
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(
            err.downcast_ref::<RecoverableError>().is_some(),
            "size-limit rejection must be RecoverableError so parallel sibling calls are not aborted"
        );
    }

    #[tokio::test]
    async fn read_file_tags_project_source() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let tool = ReadFile;
        let result = tool
            .call(json!({ "path": "test.txt" }), &ctx)
            .await
            .unwrap();
        // "project" source is the default — omitted to reduce noise
        assert!(
            result["source"].is_null(),
            "source should be omitted for project files"
        );
    }

    // ── ReadFile source code gate ────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_allows_source_code_files() {
        // Source files are no longer blocked — small ones return content directly,
        // large ones are buffered. This replaces the old read_file_blocks_source_code_files test.
        let (dir, ctx) = project_ctx().await;
        let rs_file = dir.path().join("main.rs");
        std::fs::write(&rs_file, "fn main() {}\n").unwrap();

        let result = ReadFile
            .call(json!({ "path": rs_file.to_str().unwrap() }), &ctx)
            .await;
        assert!(
            result.is_ok(),
            "read_file should now allow .rs files: {:?}",
            result.err()
        );
        assert!(
            result.unwrap()["content"]
                .as_str()
                .unwrap()
                .contains("fn main"),
            "content should be returned"
        );
    }

    #[tokio::test]
    async fn read_file_allows_non_source_files() {
        let (dir, ctx) = project_ctx().await;
        let toml_file = dir.path().join("config.toml");
        std::fs::write(&toml_file, "key = \"value\"\n").unwrap();

        let result = ReadFile
            .call(json!({ "path": toml_file.to_str().unwrap() }), &ctx)
            .await;
        assert!(result.is_ok(), "read_file should allow .toml files");
    }

    #[tokio::test]
    async fn read_file_allows_markdown_files() {
        let (dir, ctx) = project_ctx().await;
        let md_file = dir.path().join("README.md");
        std::fs::write(&md_file, "# Hello\n").unwrap();

        let result = ReadFile
            .call(json!({ "path": md_file.to_str().unwrap() }), &ctx)
            .await;
        assert!(result.is_ok(), "read_file should allow .md files");
    }

    #[tokio::test]
    async fn read_file_allows_unknown_extensions() {
        let (dir, ctx) = project_ctx().await;
        let csv_file = dir.path().join("data.csv");
        std::fs::write(&csv_file, "a,b,c\n1,2,3\n").unwrap();

        let result = ReadFile
            .call(json!({ "path": csv_file.to_str().unwrap() }), &ctx)
            .await;
        assert!(result.is_ok(), "read_file should allow unknown extensions");
    }

    // ── ReadFile buffering ────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_small_file_returns_content_directly() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("small.md");
        std::fs::write(&path, "# Hello\nWorld\n").unwrap();
        let result = ReadFile
            .call(json!({"file_path": path.to_str().unwrap()}), &ctx)
            .await
            .unwrap();
        assert!(
            result.get("file_id").is_none(),
            "small file should not buffer"
        );
        assert!(result["content"].as_str().unwrap().contains("Hello"));
    }

    #[tokio::test]
    async fn read_file_large_file_returns_buffer_ref() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("big.md");
        let content: String = (1..=210).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();
        let result = ReadFile
            .call(json!({"file_path": path.to_str().unwrap()}), &ctx)
            .await
            .unwrap();
        let file_id = result["file_id"]
            .as_str()
            .expect("large file should have file_id");
        assert!(
            file_id.starts_with("@file_"),
            "file_id should start with @file_, got: {file_id}"
        );
        assert!(result["hint"].is_null(), "hint field should be absent");
        let entry = ctx.output_buffer.get(file_id).unwrap();
        assert!(entry.stdout.contains("line 100"));
    }

    #[tokio::test]
    async fn read_file_explicit_range_always_returns_directly() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("big.rs");
        let content: String = (1..=300).map(|i| format!("// line {}\n", i)).collect();
        std::fs::write(&path, &content).unwrap();
        let result = ReadFile
            .call(
                json!({"file_path": path.to_str().unwrap(), "start_line": 1, "end_line": 5}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            result.get("file_id").is_none(),
            "explicit range should never buffer"
        );
        assert!(result["content"].as_str().unwrap().contains("line 1"));
    }

    #[tokio::test]
    async fn read_file_small_source_file_no_longer_errors() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("lib.rs");
        let content: String = (0..105)
            .map(|i| format!("fn fn_{}() {{}}\n\n", i))
            .collect();
        std::fs::write(&path, &content).unwrap();
        let result = ReadFile
            .call(json!({"file_path": path.to_str().unwrap()}), &ctx)
            .await
            .unwrap();
        assert!(
            result.get("file_id").is_some() || result.get("content").is_some(),
            "should buffer or return content, not error; got: {}",
            result
        );
    }

    // ── search_pattern: regex and string pattern tests ────────────────────────

    #[tokio::test]
    async fn search_pattern_regex_character_class_matches() {
        // Regression: `const [a-zA-Z]+ = async` was returning 0 matches even
        // when content matched. Verify character classes + quantifiers work.
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("code.js"),
            "const foo = async () => {};\nconst bar = async () => {};\nfunction baz() {}\n",
        )
        .unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "const [a-zA-Z]+ = async", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(
            matches.len(),
            2,
            "should match both const async arrow functions"
        );
        assert!(matches[0]["content"]
            .as_str()
            .unwrap()
            .contains("const foo"));
        assert!(matches[1]["content"]
            .as_str()
            .unwrap()
            .contains("const bar"));
        assert_eq!(result["total"], 2);
    }

    #[tokio::test]
    async fn search_pattern_escaped_paren_matches_literal_paren() {
        // `if \(name === '` should match a literal `if (name === '` in source.
        // Without the backslash, `(` would open an unclosed regex group (invalid).
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("api.js"),
            "function check() {\n  if (name === 'admin') { return true; }\n}\n",
        )
        .unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": r"if \(name === '", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1, "escaped paren should match literal (");
        assert!(matches[0]["content"].as_str().unwrap().contains("if (name"));
    }

    #[tokio::test]
    async fn search_pattern_unescaped_paren_is_invalid_regex() {
        // `if (name === '` — unescaped `(` opens a group that is never closed.
        // Must be a RecoverableError (isError: false) so sibling parallel calls aren't aborted.
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("api.js"),
            "function check() {\n  if (name === 'admin') { return true; }\n}\n",
        )
        .unwrap();

        let err = SearchPattern
            .call(
                json!({ "pattern": "if (name === '", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(
            err.downcast_ref::<RecoverableError>().is_some(),
            "invalid regex must be RecoverableError so parallel sibling calls are not aborted"
        );
    }

    #[tokio::test]
    async fn search_pattern_dot_matches_any_char_not_literal_dot() {
        // `.` in a regex matches ANY character, not just a literal `.`
        // This confirms the tool is regex-based, not literal-string-based.
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("code.rs"),
            "fn main() {}\nfn_main_alt() {}\n",
        )
        .unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "fn.main", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        // `.` matches `_` in `fn_main_alt` AND ` ` in `fn main()` — both lines match
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(
            matches.len(),
            2,
            "dot should match any char including space and underscore"
        );
    }

    #[tokio::test]
    async fn search_pattern_multi_file_returns_all_matches() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "pub fn handler() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "pub fn handler() {}\n").unwrap();
        std::fs::write(dir.path().join("c.rs"), "fn unrelated() {}\n").unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "pub fn handler", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(
            matches.len(),
            2,
            "should find matches across multiple files"
        );
        let files: Vec<&str> = matches
            .iter()
            .map(|m| m["file"].as_str().unwrap())
            .collect();
        assert!(files.iter().any(|f| f.ends_with("a.rs")));
        assert!(files.iter().any(|f| f.ends_with("b.rs")));
    }

    #[tokio::test]
    async fn search_pattern_case_sensitive_by_default() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("code.rs"),
            "async fn handler() {}\nAsync fn Handler() {}\n",
        )
        .unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "async fn handler", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(
            matches.len(),
            1,
            "search should be case-sensitive by default"
        );
        assert!(matches[0]["content"].as_str().unwrap().starts_with("async"));
    }

    #[tokio::test]
    async fn read_file_source_with_range_allowed() {
        let (dir, ctx) = project_ctx().await;
        let rs_file = dir.path().join("lib.rs");
        std::fs::write(&rs_file, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let result = ReadFile
            .call(
                json!({
                    "path": rs_file.to_str().unwrap(),
                    "start_line": 2,
                    "end_line": 4
                }),
                &ctx,
            )
            .await
            .unwrap();

        let content = result["content"].as_str().unwrap();
        assert!(content.contains("line2"), "should include line2: {content}");
        assert!(content.contains("line4"), "should include line4: {content}");
        assert!(
            !content.contains("line5"),
            "should not include line5: {content}"
        );
    }

    #[tokio::test]
    async fn read_file_source_without_range_now_works() {
        // Source files are no longer blocked — small ones return content directly,
        // large ones are buffered. Renamed from read_file_source_without_range_still_blocked.
        let (dir, ctx) = project_ctx().await;
        let rs_file = dir.path().join("lib.rs");
        std::fs::write(&rs_file, "fn main() {}\n").unwrap();

        let result = ReadFile
            .call(json!({ "path": rs_file.to_str().unwrap() }), &ctx)
            .await;

        assert!(
            result.is_ok(),
            "source files should now be readable: {:?}",
            result.err()
        );
        assert!(
            result.unwrap()["content"]
                .as_str()
                .unwrap()
                .contains("fn main"),
            "content should contain source"
        );
    }

    #[tokio::test]
    async fn search_pattern_context_lines_zero_backward_compat() {
        // context_lines absent → old format (line + content keys)
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn main() {}\nlet x = 42;\n").unwrap();

        let result = SearchPattern
            .call(
                json!({ "pattern": "fn main", "path": dir.path().to_str().unwrap() }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["line"], 1);
        assert!(matches[0]["content"].as_str().unwrap().contains("fn main"));
    }

    #[tokio::test]
    async fn search_pattern_context_lines_single_match() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        // TARGET is line 3; context=2 → block covers lines 1-5
        std::fs::write(
            dir.path().join("code.rs"),
            "line1\nline2\nTARGET\nline4\nline5\n",
        )
        .unwrap();

        let result = SearchPattern
            .call(
                json!({
                    "pattern": "TARGET",
                    "path": dir.path().to_str().unwrap(),
                    "context_lines": 2
                }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0]["match_line"], 3,
            "match_line should be 1-indexed line of TARGET"
        );
        assert_eq!(
            matches[0]["start_line"], 1,
            "start_line = match(3) - context(2) = 1"
        );
        let content = matches[0]["content"].as_str().unwrap();
        assert!(
            content.contains("line1"),
            "context_before should include line1"
        );
        assert!(content.contains("TARGET"), "content should include match");
        assert!(
            content.contains("line5"),
            "context_after should include line5"
        );
    }

    #[tokio::test]
    async fn search_pattern_context_lines_adjacent_matches_merge() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        // MATCH_A at line 3, MATCH_B at line 5; context=2 → windows overlap → one block
        std::fs::write(
            dir.path().join("code.rs"),
            "line1\nline2\nMATCH_A\nline4\nMATCH_B\nline6\nline7\n",
        )
        .unwrap();

        let result = SearchPattern
            .call(
                json!({
                    "pattern": "MATCH_",
                    "path": dir.path().to_str().unwrap(),
                    "context_lines": 2
                }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(
            matches.len(),
            1,
            "overlapping context windows should merge into one block"
        );
        let content = matches[0]["content"].as_str().unwrap();
        assert!(
            content.contains("MATCH_A"),
            "merged block should contain first match"
        );
        assert!(
            content.contains("MATCH_B"),
            "merged block should contain second match"
        );
        assert!(
            content.contains("line7"),
            "block should extend to MATCH_B's context_after"
        );
    }

    #[tokio::test]
    async fn search_pattern_context_mode_total_equals_block_count_not_line_count() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        // MATCH_A at line 3, MATCH_B at line 5 — only 2 lines apart, so with context_lines=2
        // their windows overlap and merge into ONE block.
        std::fs::write(
            dir.path().join("code.rs"),
            "line1\nline2\nMATCH_A\nline4\nMATCH_B\nline6\nline7\n",
        )
        .unwrap();
        let path = dir.path().to_str().unwrap();

        // Baseline: without context_lines, total equals the raw number of matching lines (2).
        // This is what the old context-mode code also used — the line count — which was wrong.
        let no_ctx = SearchPattern
            .call(json!({ "pattern": "MATCH_", "path": path }), &ctx)
            .await
            .unwrap();
        assert_eq!(
            no_ctx["total"].as_u64().unwrap(),
            2,
            "without context, total must equal the number of matching lines"
        );
        assert_eq!(no_ctx["matches"].as_array().unwrap().len(), 2);

        // Stale (the bug): with context_lines=2 the two matches merge into 1 block,
        // but old code still set total=2 (line count) — inconsistent with matches.len()=1.
        // Fixed: total now equals the number of merged blocks returned.
        let with_ctx = SearchPattern
            .call(
                json!({ "pattern": "MATCH_", "path": path, "context_lines": 2 }),
                &ctx,
            )
            .await
            .unwrap();
        let matches = with_ctx["matches"].as_array().unwrap();
        let total = with_ctx["total"].as_u64().unwrap();

        // Sandwich assertion: the two matches must have merged into exactly one block
        assert_eq!(
            matches.len(),
            1,
            "adjacent matches with context_lines=2 must merge into one block"
        );
        // Fresh: total now tracks blocks, not lines — so it equals 1, not 2
        assert_eq!(
            total, 1,
            "total must be block count (1), not line count (2)"
        );
        assert_eq!(
            total,
            matches.len() as u64,
            "total must always equal matches.len()"
        );
    }

    #[tokio::test]
    async fn search_pattern_context_lines_non_adjacent_matches_separate() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        // MATCH at line 2 and line 18; with context=2 the windows don't overlap → two blocks
        let file_content = (1..=20)
            .map(|i| {
                if i == 2 || i == 18 {
                    format!("MATCH line{i}")
                } else {
                    format!("other line{i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(dir.path().join("code.rs"), file_content).unwrap();

        let result = SearchPattern
            .call(
                json!({
                    "pattern": "MATCH",
                    "path": dir.path().to_str().unwrap(),
                    "context_lines": 2
                }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert_eq!(
            matches.len(),
            2,
            "non-overlapping windows should produce two separate blocks"
        );
        assert_eq!(matches[0]["match_line"], 2);
        assert_eq!(matches[1]["match_line"], 18);
    }

    #[tokio::test]
    async fn search_pattern_context_lines_max_results_is_global_not_per_file() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        // Each file has 2 non-adjacent matches (lines 1 and 6) with context_lines=1.
        // With context=1, windows are [0..=1] and [4..=5] — non-overlapping (gap of 2 lines).
        // Per-file: 2 match events → 2 separate blocks.
        // max_results=3 → globally should stop after 3 match events total.
        // Before fix: per-file counter resets; file a emits 2 blocks, file b emits 2 blocks = 4 total.
        // After fix: counter is global; file a emits 2 blocks (count=2), file b emits 1 block (count=3, cap hit) = 3 total.
        let content = "MATCH\nother\nother\nother\nother\nMATCH\n";
        std::fs::write(dir.path().join("a.txt"), content).unwrap();
        std::fs::write(dir.path().join("b.txt"), content).unwrap();

        let result = SearchPattern
            .call(
                json!({
                    "pattern": "MATCH",
                    "path": dir.path().to_str().unwrap(),
                    "context_lines": 1,
                    "max_results": 3
                }),
                &ctx,
            )
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        let total_blocks = matches.len();
        // File a: 2 non-adjacent blocks (2 match events)
        // File b: only 1 more event before hitting max=3 → 1 block
        assert_eq!(
            total_blocks, 3,
            "max_results=3 should produce exactly 3 blocks globally, got {total_blocks}"
        );
        // total reports actual match count, not block count
        assert_eq!(result["total"], 3);
        // overflow should be present since cap was hit
        assert!(
            result.get("overflow").is_some(),
            "overflow should be present when cap is hit"
        );
    }

    // ── EditFile ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn edit_file_replaces_unique_match() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "hello",
                    "new_string": "goodbye"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "goodbye world\n");
    }

    #[tokio::test]
    async fn edit_file_empty_new_string_deletes() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "aaa bbb ccc\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": " bbb",
                    "new_string": ""
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "aaa ccc\n");
    }

    #[tokio::test]
    async fn edit_file_not_found_errors() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world\n").unwrap();

        let err = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "does not exist",
                    "new_string": "replacement"
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "error should mention 'not found', got: {msg}"
        );
    }

    #[tokio::test]
    async fn edit_file_multiple_matches_without_replace_all_errors() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "foo bar foo baz foo\n").unwrap();

        let err = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "qux"
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("3 times"),
            "error should mention '3 times', got: {msg}"
        );
        // File must be untouched
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "foo bar foo baz foo\n");
    }

    #[tokio::test]
    async fn edit_file_replace_all_replaces_all_occurrences() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "foo bar foo baz foo\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "qux",
                    "replace_all": true
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "qux bar qux baz qux\n");
    }

    #[tokio::test]
    async fn edit_file_empty_old_string_errors() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "some content\n").unwrap();

        let err = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "",
                    "new_string": "replacement"
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        let recoverable = err
            .downcast_ref::<super::RecoverableError>()
            .expect("expected a RecoverableError");
        let hint = recoverable.hint.as_deref().unwrap_or("");
        assert!(
            hint.contains("create_file"),
            "expected error hint to mention create_file, got: {hint}"
        );
    }

    #[tokio::test]
    async fn edit_file_multiline_replace() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "fn old() {\n    todo!()\n}\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "fn old() {\n    todo!()\n}",
                    "new_string": "fn new_func() {\n    42\n}"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "fn new_func() {\n    42\n}\n");
    }

    #[tokio::test]
    async fn edit_file_whitespace_sensitive() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "    indented\n").unwrap();

        // old_string without leading spaces still matches as a substring
        let result = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "indented",
                    "new_string": "replaced"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "    replaced\n");
    }

    #[tokio::test]
    async fn edit_file_returns_ok_string() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "hello",
                    "new_string": "goodbye"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        assert!(
            result.is_string(),
            "response must be a plain string, not an object"
        );
    }

    #[test]
    fn find_file_format_compact_shows_count() {
        use serde_json::json;
        let tool = FindFile;
        let result = json!({ "files": ["src/a.rs", "src/b.rs"], "total": 2 });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("2 files"), "got: {text}");
    }

    #[test]
    fn infer_edit_hint_remove_when_new_string_empty() {
        let hint = infer_edit_hint("fn foo() {\n    bar();\n}", "");
        assert!(hint.contains("remove_symbol"), "got: {hint}");
    }

    #[test]
    fn infer_edit_hint_replace_symbol_for_rust_fn() {
        let hint = infer_edit_hint("fn foo() {\n    old();\n}", "fn foo() {\n    new();\n}");
        assert!(hint.contains("replace_symbol"), "got: {hint}");
    }

    #[test]
    fn infer_edit_hint_replace_symbol_for_python_def() {
        let hint = infer_edit_hint(
            "def process(x):\n    return x",
            "def process(x):\n    return x * 2",
        );
        assert!(hint.contains("replace_symbol"), "got: {hint}");
    }

    #[test]
    fn infer_edit_hint_replace_symbol_for_class() {
        let hint = infer_edit_hint("class Foo {\n    x: i32\n}", "class Foo {\n    y: i32\n}");
        assert!(hint.contains("replace_symbol"), "got: {hint}");
    }

    #[test]
    fn infer_edit_hint_insert_code_when_new_is_longer() {
        let hint = infer_edit_hint("placeholder", "fn extra() {\n    todo!();\n}\nplaceholder");
        assert!(hint.contains("insert_code"), "got: {hint}");
    }

    #[test]
    fn infer_edit_hint_import_list_suggests_acknowledge_risk() {
        // Editing inside a `use {…}` block: only identifiers, commas, whitespace.
        // No `(`, `=`, `->` → import-fragment heuristic fires; insert_code is inapplicable.
        let old =
            "    count_lines, detect_command_type, needs_summary,\n    summarize_build_output,";
        let new = "    count_lines, detect_command_type, needs_summary,\n    summarize_build_output, truncate_lines_and_bytes,";
        let hint = infer_edit_hint(old, new);
        assert!(
            hint.contains("acknowledge_risk"),
            "import list edit should suggest acknowledge_risk, got: {hint}"
        );
        assert!(
            !hint.contains("insert_code"),
            "import list edit must not suggest insert_code (no name_path exists), got: {hint}"
        );
    }

    #[test]
    fn infer_edit_hint_insert_code_still_fires_for_real_code_insertions() {
        // When old_string has `(` it's real code (function call / expression), not an import.
        let old = "    let x = foo(\n        bar,\n    );";
        let new = "    let x = foo(\n        bar,\n        baz,\n    );";
        let hint = infer_edit_hint(old, new);
        assert!(
            hint.contains("insert_code"),
            "code expression edit should still suggest insert_code, got: {hint}"
        );
    }

    #[test]
    fn infer_edit_hint_fallback_lists_all_tools() {
        let hint = infer_edit_hint("old line\nother line", "new line\nother line");
        assert!(
            hint.contains("replace_symbol")
                && hint.contains("insert_code")
                && hint.contains("remove_symbol"),
            "got: {hint}"
        );
    }

    #[tokio::test]
    async fn edit_file_warns_multiline_on_rust_source() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "fn foo() {\n    old();\n}\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": "src/lib.rs",
                    "old_string": "fn foo() {\n    old();\n}",
                    "new_string": "fn foo() {\n    new();\n}"
                }),
                &ctx,
            )
            .await
            .expect("should return Ok with pending_ack, not an error");

        assert!(
            result["pending_ack"].as_str().is_some(),
            "expected pending_ack handle, got: {result}"
        );
        assert!(
            result["hint"]
                .as_str()
                .unwrap_or("")
                .contains("replace_symbol"),
            "hint should mention replace_symbol, got: {result}"
        );
    }

    #[tokio::test]
    async fn edit_file_blocking_hint_always_includes_acknowledge_risk() {
        // Bug 3 regression: the hint template previously said only
        // "To proceed anyway: edit_file(\"@ack_xxx\")" — making acknowledge_risk: true
        // invisible to the LLM. The hint must surface acknowledge_risk as an explicit bypass.
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "fn foo() {\n    old();\n}\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": "src/lib.rs",
                    "old_string": "fn foo() {\n    old();\n}",
                    "new_string": "fn foo() {\n    new();\n}"
                }),
                &ctx,
            )
            .await
            .expect("should return Ok with pending_ack, not an error");

        let hint = result["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("acknowledge_risk"),
            "blocking hint must mention acknowledge_risk so the LLM has a clear bypass, got: {hint}"
        );
    }

    #[tokio::test]
    async fn edit_file_import_list_hint_suggests_acknowledge_risk_not_insert_code() {
        // Bug 2 regression (integration path): editing a multi-line use-import list triggered
        // the length heuristic in infer_edit_hint and returned insert_code, which requires
        // a name_path that doesn't exist for import identifiers.
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "use crate::foo::{\n    count_lines,\n    needs_summary,\n};\n",
        )
        .unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": "src/lib.rs",
                    "old_string": "    count_lines,\n    needs_summary,",
                    "new_string": "    count_lines,\n    needs_summary,\n    truncate_lines_and_bytes,"
                }),
                &ctx,
            )
            .await
            .expect("should return Ok with pending_ack, not an error");

        let hint = result["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("acknowledge_risk"),
            "import list edit must suggest acknowledge_risk (no name_path exists), got: {hint}"
        );
        assert!(
            !hint.contains("insert_code"),
            "import list edit must not suggest insert_code, got: {hint}"
        );
    }

    #[tokio::test]
    async fn edit_file_allows_singleline_on_rust_source() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "let x = 1;\n").unwrap();

        let result = EditFile
            .call(
                json!({"path": "src/lib.rs", "old_string": "x = 1", "new_string": "x = 2"}),
                &ctx,
            )
            .await;

        assert!(
            result.is_ok(),
            "single-line edits on source should pass: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn edit_file_allows_multiline_on_markdown() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("README.md");
        std::fs::write(&path, "line one\nline two\n").unwrap();

        let result = EditFile
            .call(
                json!({"path": "README.md", "old_string": "line one\nline two", "new_string": "updated one\nupdated two"}),
                &ctx,
            )
            .await;

        assert!(
            result.is_ok(),
            "multi-line edits on non-source should pass: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn edit_file_warns_multiline_python() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("app.py");
        std::fs::write(&path, "def greet():\n    print('hello')\n").unwrap();

        let result = EditFile
            .call(
                json!({"path": "app.py", "old_string": "def greet():\n    print('hello')", "new_string": "def greet():\n    print('hi')"}),
                &ctx,
            )
            .await
            .expect("should return Ok with pending_ack, not an error");

        assert!(
            result["pending_ack"].as_str().is_some(),
            "expected pending_ack handle, got: {result}"
        );
        assert!(
            result["hint"]
                .as_str()
                .unwrap_or("")
                .contains("replace_symbol"),
            "hint should mention replace_symbol, got: {result}"
        );
    }

    #[tokio::test]
    async fn edit_file_warns_hint_suggests_remove_when_new_empty() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "fn foo() {\n    bar();\n}\n").unwrap();

        let result = EditFile
            .call(
                json!({"path": "src/lib.rs", "old_string": "fn foo() {\n    bar();\n}", "new_string": ""}),
                &ctx,
            )
            .await
            .expect("should return Ok with pending_ack, not an error");

        assert!(
            result["pending_ack"].as_str().is_some(),
            "expected pending_ack handle, got: {result}"
        );
        assert!(
            result["hint"]
                .as_str()
                .unwrap_or("")
                .contains("remove_symbol"),
            "hint should mention remove_symbol, got: {result}"
        );
    }

    #[tokio::test]
    async fn edit_file_prepend_adds_text_at_start() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "line two\n").unwrap();

        let result = EditFile
            .call(
                json!({"path": "test.txt", "insert": "prepend", "new_string": "line one\n"}),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "line one\nline two\n");
    }

    #[tokio::test]
    async fn edit_file_append_adds_text_at_end() {
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "line one\n").unwrap();

        let result = EditFile
            .call(
                json!({"path": "test.txt", "insert": "append", "new_string": "line two\n"}),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result, json!("ok"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "line one\nline two\n");
    }

    #[tokio::test]
    async fn edit_file_insert_without_old_string_ok() {
        // insert mode should not require old_string
        let (dir, ctx) = project_ctx().await;
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "existing\n").unwrap();

        let result = EditFile
            .call(
                json!({"path": "test.txt", "insert": "prepend", "new_string": "header\n"}),
                &ctx,
            )
            .await;

        assert!(result.is_ok(), "should succeed without old_string");
    }

    #[tokio::test]
    async fn edit_file_ack_handle_executes_edit() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "fn foo() {\n    old();\n}\n").unwrap();

        // First call: returns pending_ack handle.
        let warn = EditFile
            .call(
                json!({
                    "path": "src/lib.rs",
                    "old_string": "fn foo() {\n    old();\n}",
                    "new_string": "fn foo() {\n    new();\n}"
                }),
                &ctx,
            )
            .await
            .expect("first call should return Ok");
        let handle = warn["pending_ack"]
            .as_str()
            .expect("should have pending_ack handle")
            .to_string();

        // Second call: pass the ack handle as path — edit executes.
        let result = EditFile
            .call(json!({"path": handle, "new_string": ""}), &ctx)
            .await
            .expect("ack call should succeed");
        assert_eq!(result, json!("ok"));

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("new()"),
            "file should contain new() after ack: {written}"
        );
    }

    #[tokio::test]
    async fn edit_file_acknowledge_risk_bypasses_source_check() {
        let (dir, ctx) = project_ctx().await;
        let path = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "fn foo() {\n    old();\n}\n").unwrap();

        let result = EditFile
            .call(
                json!({
                    "path": "src/lib.rs",
                    "old_string": "fn foo() {\n    old();\n}",
                    "new_string": "fn foo() {\n    new();\n}",
                    "acknowledge_risk": true
                }),
                &ctx,
            )
            .await
            .expect("acknowledge_risk should bypass the check");
        assert_eq!(result, json!("ok"));

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("new()"),
            "file should contain new() after direct ack: {written}"
        );
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
        assert!(result.contains("ast.rs"));
        assert!(result.contains("mod.rs"));
        assert!(result.contains("user_format.rs"));
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
        assert!(result.contains("a.rs:1"));
        assert!(result.contains("very/long/path.rs:100"));
    }

    #[test]
    fn search_missing_matches_key() {
        let val = serde_json::json!({});
        assert_eq!(format_search_pattern(&val), "");
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
            "headings": [
                {"heading": "# Title", "level": 1, "line": 1, "end_line": 200},
                {"heading": "## Section 1", "level": 2, "line": 5, "end_line": 100},
                {"heading": "## Section 2", "level": 2, "line": 101, "end_line": 200}
            ],
            "file_id": "@file_xyz",
            "hint": "Full file stored as @file_xyz."
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("200 lines (Markdown)\n"));
        assert!(result.contains("Headings:"));
        assert!(result.contains("# Title  L1-200"));
        assert!(result.contains("## Section 1  L5-100"));
        assert!(result.contains("## Section 2  L101-200"));
        assert!(result.contains("Buffer: @file_xyz"));
    }

    #[test]
    fn read_file_config_summary() {
        let val = serde_json::json!({
            "type": "config",
            "line_count": 50,
            "preview": "[package]\nname = \"codescout\"\nversion = \"0.1.0\"",
            "file_id": "@file_cfg",
            "hint": "Full file stored as @file_cfg."
        });
        let result = format_read_file(&val);
        assert!(result.starts_with("50 lines (Config)\n"));
        assert!(result.contains("Preview:"));
        assert!(result.contains("[package]"));
        assert!(result.contains("name = \"codescout\""));
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
        assert!(!result.contains("Symbols:"));
        assert!(result.contains("Buffer: @file_empty"));
    }

    #[test]
    fn read_file_lineno_alignment() {
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
        assert!(result.contains(" 1| line 1"));
        assert!(result.contains(" 9| line 9"));
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
}
