use serde_json::Value;

use crate::tools::RecoverableError;

pub const FILE_BUFFER_THRESHOLD: usize = 200;

pub struct SectionResult {
    pub content: String,
    pub line_range: (usize, usize), // 1-indexed, inclusive
    pub breadcrumb: Vec<String>,
    pub siblings: Vec<String>,
    pub format: String,
}

pub enum FileSummaryType {
    Source,
    Markdown,
    Json,
    Yaml,
    Toml,
    Config, // remaining: .xml, .ini, .env, .lock, .cfg
    Generic,
}

// Stubs — implementations replaced in GREEN phase
pub fn detect_file_type(path: &str) -> FileSummaryType {
    let lower = path.to_lowercase();
    const SOURCE_EXTS: &[&str] = &[
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".java", ".kt", ".kts", ".c", ".cpp",
        ".cc", ".cxx", ".h", ".swift", ".rb", ".cs", ".php", ".scala", ".ex", ".exs", ".hs",
        ".lua", ".sh", ".bash",
    ];
    const CONFIG_EXTS: &[&str] = &[".xml", ".ini", ".env", ".lock", ".cfg"];
    if SOURCE_EXTS.iter().any(|e| lower.ends_with(e)) {
        FileSummaryType::Source
    } else if lower.ends_with(".md") || lower.ends_with(".mdx") {
        FileSummaryType::Markdown
    } else if lower.ends_with(".json") {
        FileSummaryType::Json
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        FileSummaryType::Yaml
    } else if lower.ends_with(".toml") {
        FileSummaryType::Toml
    } else if CONFIG_EXTS.iter().any(|e| lower.ends_with(e)) {
        FileSummaryType::Config
    } else {
        FileSummaryType::Generic
    }
}

pub fn summarize_source(path: &str, content: &str) -> Value {
    let p = std::path::Path::new(path);
    let language = crate::ast::detect_language(p);
    let symbols =
        crate::ast::parser::extract_symbols_from_source(content, language, p).unwrap_or_default();

    if symbols.is_empty() {
        let mut result = summarize_generic_file(content);
        result["type"] = serde_json::json!("source");
        return result;
    }

    let names: Vec<serde_json::Value> = symbols
        .iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name_path,
                "kind": format!("{:?}", s.kind),
                "line": s.start_line + 1,
            })
        })
        .collect();

    serde_json::json!({
        "type": "source",
        "line_count": content.lines().count(),
        "symbols": names,
    })
}

pub fn summarize_markdown(content: &str) -> Value {
    let line_count = content.lines().count();
    let mut in_code_block = false;

    // First pass: collect heading positions
    let mut raw_headings: Vec<(String, usize, usize)> = Vec::new(); // (text, level, line_1indexed)
    for (idx, line) in content.lines().enumerate() {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        if let Some(level) = heading_level(line) {
            raw_headings.push((line.to_string(), level, idx + 1));
        }
    }

    // Second pass: compute end_line for each heading
    let mut headings: Vec<Value> = Vec::new();
    for (i, (text, level, line)) in raw_headings.iter().enumerate() {
        let end_line = raw_headings[i + 1..]
            .iter()
            .find(|(_, l, _)| *l <= *level)
            .map(|(_, _, next_line)| next_line - 1)
            .unwrap_or(line_count);
        headings.push(serde_json::json!({
            "heading": text,
            "level": level,
            "line": line,
            "end_line": end_line,
        }));
    }

    headings.truncate(30);

    serde_json::json!({
        "type": "markdown",
        "line_count": line_count,
        "headings": headings,
    })
}

fn heading_level(line: &str) -> Option<usize> {
    if !line.starts_with('#') {
        return None;
    }
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
        Some(hashes)
    } else {
        None
    }
}

pub fn extract_markdown_section(
    content: &str,
    heading_query: &str,
) -> Result<SectionResult, RecoverableError> {
    let summary = summarize_markdown(content);
    let headings = summary["headings"]
        .as_array()
        .ok_or_else(|| RecoverableError::new("no headings found in file"))?;

    if headings.is_empty() {
        return Err(RecoverableError::with_hint(
            "no headings found in file",
            "The file contains no Markdown headings to navigate",
        ));
    }

    // Find matching heading: exact first, then prefix, then substring
    let query_lower = heading_query.to_lowercase();
    let matched_idx = headings
        .iter()
        .position(|h| h["heading"].as_str().unwrap_or("") == heading_query)
        .or_else(|| {
            headings.iter().position(|h| {
                h["heading"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .starts_with(&query_lower)
            })
        })
        .or_else(|| {
            headings.iter().position(|h| {
                h["heading"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query_lower)
            })
        });

    let idx = matched_idx.ok_or_else(|| {
        let available: Vec<&str> = headings
            .iter()
            .filter_map(|h| h["heading"].as_str())
            .take(15)
            .collect();
        RecoverableError::with_hint(
            format!("heading '{}' not found", heading_query),
            format!("Available headings: {}", available.join(", ")),
        )
    })?;

    let matched = &headings[idx];
    let line = matched["line"].as_u64().unwrap_or(1) as usize;
    let end_line = matched["end_line"].as_u64().unwrap_or(1) as usize;
    let level = matched["level"].as_u64().unwrap_or(1) as usize;

    // Extract content
    let lines: Vec<&str> = content.lines().collect();
    let start = (line - 1).min(lines.len());
    let end = end_line.min(lines.len());
    let section_content = lines[start..end].join("\n");

    // Build breadcrumb: walk backwards collecting parents (lower level numbers)
    let mut breadcrumb = Vec::new();
    let mut current_level = level;
    for i in (0..=idx).rev() {
        let h_level = headings[i]["level"].as_u64().unwrap_or(1) as usize;
        if h_level < current_level || i == idx {
            breadcrumb.push(headings[i]["heading"].as_str().unwrap_or("?").to_string());
            current_level = h_level;
        }
    }
    breadcrumb.reverse();

    // Find siblings: same level headings (excluding the matched one)
    let siblings: Vec<String> = headings
        .iter()
        .filter(|h| {
            h["level"].as_u64().unwrap_or(0) as usize == level
                && h["heading"].as_str() != matched["heading"].as_str()
        })
        .map(|h| h["heading"].as_str().unwrap_or("?").to_string())
        .collect();

    Ok(SectionResult {
        content: section_content,
        line_range: (line, end_line),
        breadcrumb,
        siblings,
        format: "markdown".to_string(),
    })
}

pub fn summarize_json(content: &str) -> Value {
    let line_count = content.lines().count();

    let parsed: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => {
            let mut fallback = summarize_generic_file(content);
            fallback["type"] = serde_json::json!("json");
            return fallback;
        }
    };

    let schema = match &parsed {
        Value::Object(map) => {
            let keys: Vec<Value> = map
                .iter()
                .take(30)
                .map(|(k, v)| {
                    let mut entry = serde_json::json!({
                        "path": format!("$.{}", k),
                        "type": json_type_name(v),
                    });
                    match v {
                        Value::Object(m) => {
                            entry["count"] = serde_json::json!(m.len());
                        }
                        Value::Array(a) => {
                            entry["count"] = serde_json::json!(a.len());
                        }
                        _ => {}
                    }
                    entry
                })
                .collect();
            serde_json::json!({ "root_type": "object", "keys": keys })
        }
        Value::Array(arr) => {
            let element_type = arr
                .first()
                .map(json_type_name)
                .unwrap_or_else(|| "unknown".to_string());
            serde_json::json!({
                "root_type": "array",
                "count": arr.len(),
                "element_type": element_type,
            })
        }
        other => serde_json::json!({ "root_type": json_type_name(other) }),
    };

    serde_json::json!({
        "type": "json",
        "line_count": line_count,
        "schema": schema,
    })
}

fn json_type_name(v: &Value) -> String {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
    .to_string()
}

/// Extract a JSON subtree by path. Returns (pretty-printed content, type name, optional count).
pub fn extract_json_path(
    content: &str,
    path: &str,
) -> Result<(String, String, Option<usize>), RecoverableError> {
    let parsed: Value = serde_json::from_str(content).map_err(|e| {
        RecoverableError::with_hint(
            format!("failed to parse JSON: {}", e),
            "Ensure the file contains valid JSON",
        )
    })?;

    let segments = parse_json_path_segments(path)?;

    let mut current = &parsed;
    for seg in &segments {
        current = resolve_json_segment(current, seg).ok_or_else(|| {
            let available = match current {
                Value::Object(m) => m.keys().take(10).cloned().collect::<Vec<_>>().join(", "),
                Value::Array(a) => format!(
                    "array with {} elements (0..{})",
                    a.len(),
                    a.len().saturating_sub(1)
                ),
                _ => format!("{} (not navigable)", json_type_name(current)),
            };
            RecoverableError::with_hint(
                format!("path segment '{}' not found at '{}'", seg, path),
                format!("Available: {}", available),
            )
        })?;
    }

    let pretty = serde_json::to_string_pretty(current).unwrap_or_else(|_| current.to_string());
    let type_name = json_type_name(current);
    let count = match current {
        Value::Object(m) => Some(m.len()),
        Value::Array(a) => Some(a.len()),
        _ => None,
    };

    Ok((pretty, type_name, count))
}

fn parse_json_path_segments(path: &str) -> Result<Vec<String>, RecoverableError> {
    let path = path
        .strip_prefix("$.")
        .or_else(|| path.strip_prefix("$"))
        .unwrap_or(path);
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let mut segments = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            continue;
        }
        if let Some(bracket_pos) = part.find('[') {
            let key = &part[..bracket_pos];
            if !key.is_empty() {
                segments.push(key.to_string());
            }
            let idx_str = &part[bracket_pos..];
            segments.push(idx_str.to_string());
        } else {
            segments.push(part.to_string());
        }
    }
    Ok(segments)
}

fn resolve_json_segment<'a>(value: &'a Value, segment: &str) -> Option<&'a Value> {
    if segment.starts_with('[') && segment.ends_with(']') {
        let idx: usize = segment[1..segment.len() - 1].parse().ok()?;
        value.as_array()?.get(idx)
    } else {
        value.get(segment)
    }
}

pub fn summarize_toml(content: &str) -> Value {
    let line_count = content.lines().count();

    // Scan for TOML table headers: [name] or [[name]]
    let mut sections: Vec<(String, usize)> = Vec::new(); // (header, line_1indexed)
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            sections.push((trimmed.to_string(), idx + 1));
        }
    }

    // If no table headers found, try parsing as TOML and list top-level keys
    if sections.is_empty() {
        if let Ok(table) = content.parse::<toml::Table>() {
            let keys: Vec<Value> = table
                .keys()
                .take(20)
                .map(|k| {
                    let line = find_toml_key_line(content, k);
                    serde_json::json!({ "key": k, "line": line.unwrap_or(0) })
                })
                .collect();
            return serde_json::json!({
                "type": "toml",
                "format": "toml",
                "line_count": line_count,
                "keys": keys,
            });
        }
        let mut fallback = summarize_generic_file(content);
        fallback["type"] = serde_json::json!("toml");
        fallback["format"] = serde_json::json!("toml");
        return fallback;
    }

    // Compute end_line for each section
    let mut result_sections: Vec<Value> = Vec::new();
    for (i, (header, line)) in sections.iter().enumerate() {
        let end_line = sections
            .get(i + 1)
            .map(|(_, next)| next - 1)
            .unwrap_or(line_count);
        result_sections.push(serde_json::json!({
            "key": header,
            "line": line,
            "end_line": end_line,
        }));
    }
    result_sections.truncate(30);

    serde_json::json!({
        "type": "toml",
        "format": "toml",
        "line_count": line_count,
        "sections": result_sections,
    })
}

fn find_toml_key_line(content: &str, key: &str) -> Option<u64> {
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with(key) && trimmed[key.len()..].trim_start().starts_with('=') {
            return Some((idx + 1) as u64);
        }
    }
    None
}

pub fn summarize_yaml(content: &str) -> Value {
    let line_count = content.lines().count();

    // Scan for top-level keys: lines starting at column 0, containing ':'
    let mut sections: Vec<(String, usize)> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() || trimmed == "---" || trimmed == "..." {
            continue;
        }
        // Top-level key: starts at column 0 (no leading whitespace), has a colon
        if !line.starts_with(' ') && !line.starts_with('\t') {
            if let Some(colon_pos) = trimmed.find(':') {
                let key = trimmed[..colon_pos].trim().to_string();
                if !key.is_empty() && !key.starts_with('-') {
                    sections.push((key, idx + 1));
                }
            }
        }
    }

    if sections.is_empty() {
        let mut fallback = summarize_generic_file(content);
        fallback["type"] = serde_json::json!("yaml");
        fallback["format"] = serde_json::json!("yaml");
        return fallback;
    }

    // Compute end_line for each section
    let mut result_sections: Vec<Value> = Vec::new();
    for (i, (key, line)) in sections.iter().enumerate() {
        let end_line = sections
            .get(i + 1)
            .map(|(_, next)| next - 1)
            .unwrap_or(line_count);
        result_sections.push(serde_json::json!({
            "key": key,
            "line": line,
            "end_line": end_line,
        }));
    }
    result_sections.truncate(30);

    serde_json::json!({
        "type": "yaml",
        "format": "yaml",
        "line_count": line_count,
        "sections": result_sections,
    })
}

pub fn summarize_config(content: &str) -> Value {
    let line_count = content.lines().count();
    let preview: String = content.lines().take(30).collect::<Vec<_>>().join("\n");
    serde_json::json!({
        "type": "config",
        "line_count": line_count,
        "preview": preview,
    })
}

pub fn summarize_generic_file(content: &str) -> Value {
    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();
    let head: String = lines
        .iter()
        .take(20)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let tail_start = line_count.saturating_sub(10);
    let tail: String = lines[tail_start..].join("\n");
    serde_json::json!({
        "type": "generic",
        "line_count": line_count,
        "head": head,
        "tail": tail,
    })
}

pub fn extract_toml_key(content: &str, key: &str) -> Result<SectionResult, RecoverableError> {
    let summary = summarize_toml(content);

    // Check sections first (table headers like [dependencies])
    if let Some(sections) = summary["sections"].as_array() {
        // Match against table names: key could be "dependencies" matching "[dependencies]"
        let table_name = format!("[{}]", key);
        let array_name = format!("[[{}]]", key);
        if let Some(matched) = sections.iter().find(|s| {
            let k = s["key"].as_str().unwrap_or("");
            k == table_name || k == array_name || k == key
        }) {
            let line = matched["line"].as_u64().unwrap_or(1) as usize;
            let end_line = matched["end_line"].as_u64().unwrap_or(1) as usize;
            let lines: Vec<&str> = content.lines().collect();
            let start = (line - 1).min(lines.len());
            let end = end_line.min(lines.len());
            let section_content = lines[start..end].join("\n");
            let siblings: Vec<String> = sections
                .iter()
                .filter_map(|s| s["key"].as_str())
                .filter(|k| *k != matched["key"].as_str().unwrap_or(""))
                .map(|s| s.to_string())
                .collect();
            return Ok(SectionResult {
                content: section_content,
                line_range: (line, end_line),
                breadcrumb: vec![matched["key"].as_str().unwrap_or("?").to_string()],
                siblings,
                format: "toml".to_string(),
            });
        }

        // Not found in sections — show available
        let available: Vec<&str> = sections.iter().filter_map(|s| s["key"].as_str()).collect();
        return Err(RecoverableError::with_hint(
            format!("key '{}' not found in TOML", key),
            format!("Available sections: {}", available.join(", ")),
        ));
    }

    // No sections found — check top-level keys
    if let Some(keys) = summary["keys"].as_array() {
        let available: Vec<&str> = keys.iter().filter_map(|k| k["key"].as_str()).collect();
        // Try to find the key and extract via parsing
        if let Ok(table) = content.parse::<toml::Table>() {
            let segments: Vec<&str> = key.split('.').collect();
            let mut current: &toml::Value = &toml::Value::Table(table);
            for seg in &segments {
                current = current.get(seg).ok_or_else(|| {
                    RecoverableError::with_hint(
                        format!("key '{}' not found in TOML", key),
                        format!("Available keys: {}", available.join(", ")),
                    )
                })?;
            }
            let pretty =
                toml::to_string_pretty(current).unwrap_or_else(|_| format!("{:?}", current));
            return Ok(SectionResult {
                content: pretty,
                line_range: (1, content.lines().count()),
                breadcrumb: segments.iter().map(|s| s.to_string()).collect(),
                siblings: Vec::new(),
                format: "toml".to_string(),
            });
        }
    }

    Err(RecoverableError::with_hint(
        format!("key '{}' not found in TOML", key),
        "File could not be parsed as TOML",
    ))
}

pub fn extract_yaml_key(content: &str, key: &str) -> Result<SectionResult, RecoverableError> {
    let summary = summarize_yaml(content);

    if let Some(sections) = summary["sections"].as_array() {
        if let Some(matched) = sections
            .iter()
            .find(|s| s["key"].as_str().unwrap_or("") == key)
        {
            let line = matched["line"].as_u64().unwrap_or(1) as usize;
            let end_line = matched["end_line"].as_u64().unwrap_or(1) as usize;
            let lines: Vec<&str> = content.lines().collect();
            let start = (line - 1).min(lines.len());
            let end = end_line.min(lines.len());
            let section_content = lines[start..end].join("\n");
            let siblings: Vec<String> = sections
                .iter()
                .filter_map(|s| s["key"].as_str())
                .filter(|k| *k != key)
                .map(|s| s.to_string())
                .collect();
            return Ok(SectionResult {
                content: section_content,
                line_range: (line, end_line),
                breadcrumb: vec![key.to_string()],
                siblings,
                format: "yaml".to_string(),
            });
        }

        let available: Vec<String> = sections
            .iter()
            .filter_map(|s| s["key"].as_str().map(|s| s.to_string()))
            .collect();
        return Err(RecoverableError::with_hint(
            format!("key '{}' not found in YAML", key),
            format!("Available keys: {}", available.join(", ")),
        ));
    }

    Err(RecoverableError::with_hint(
        format!("key '{}' not found in YAML", key),
        "No top-level keys found in file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_rust_as_source() {
        assert!(matches!(
            detect_file_type("src/main.rs"),
            FileSummaryType::Source
        ));
        assert!(matches!(
            detect_file_type("lib.py"),
            FileSummaryType::Source
        ));
    }

    #[test]
    fn detect_md_as_markdown() {
        assert!(matches!(
            detect_file_type("README.md"),
            FileSummaryType::Markdown
        ));
        assert!(matches!(
            detect_file_type("docs/guide.mdx"),
            FileSummaryType::Markdown
        ));
    }

    #[test]
    fn detect_json_as_json() {
        assert!(matches!(
            detect_file_type("data.json"),
            FileSummaryType::Json
        ));
        assert!(matches!(
            detect_file_type("package.json"),
            FileSummaryType::Json
        ));
    }

    #[test]
    fn detect_yaml_as_yaml() {
        assert!(matches!(
            detect_file_type("config.yaml"),
            FileSummaryType::Yaml
        ));
        assert!(matches!(
            detect_file_type("docker-compose.yml"),
            FileSummaryType::Yaml
        ));
    }

    #[test]
    fn detect_toml_as_toml() {
        assert!(matches!(
            detect_file_type("Cargo.toml"),
            FileSummaryType::Toml
        ));
        assert!(matches!(
            detect_file_type("pyproject.toml"),
            FileSummaryType::Toml
        ));
    }

    #[test]
    fn detect_other_config_still_works() {
        // .xml, .ini, .env, .lock, .cfg stay as Config
        assert!(matches!(
            detect_file_type("web.xml"),
            FileSummaryType::Config
        ));
        assert!(matches!(detect_file_type(".env"), FileSummaryType::Config));
        assert!(matches!(
            detect_file_type("Cargo.lock"),
            FileSummaryType::Config
        ));
    }

    #[test]
    fn detect_unknown_as_generic() {
        assert!(matches!(
            detect_file_type("data.csv"),
            FileSummaryType::Generic
        ));
        assert!(matches!(
            detect_file_type("Makefile"),
            FileSummaryType::Generic
        ));
    }

    #[test]
    fn markdown_summary_basic_structure() {
        let content = "# Title\nsome text\n## Section\nmore text\n### Sub\nnope";
        let s = summarize_markdown(content);
        let headings = s["headings"].as_array().unwrap();
        // Now includes H3
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0]["heading"].as_str().unwrap(), "# Title");
        assert_eq!(headings[0]["level"].as_u64().unwrap(), 1);
        assert_eq!(headings[1]["heading"].as_str().unwrap(), "## Section");
        assert_eq!(headings[2]["heading"].as_str().unwrap(), "### Sub");
        assert_eq!(s["line_count"].as_u64().unwrap(), 6);
    }

    #[test]
    fn markdown_summary_includes_line_ranges() {
        let content =
            "# Title\ntext\n## Section A\nmore text\nstill more\n## Section B\nfinal text";
        let s = summarize_markdown(content);
        let headings = s["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0]["heading"].as_str().unwrap(), "# Title");
        assert_eq!(headings[0]["level"].as_u64().unwrap(), 1);
        assert_eq!(headings[0]["line"].as_u64().unwrap(), 1);
        assert_eq!(headings[0]["end_line"].as_u64().unwrap(), 7); // H1 covers everything
        assert_eq!(headings[1]["heading"].as_str().unwrap(), "## Section A");
        assert_eq!(headings[1]["line"].as_u64().unwrap(), 3);
        assert_eq!(headings[1]["end_line"].as_u64().unwrap(), 5);
        assert_eq!(headings[2]["line"].as_u64().unwrap(), 6);
        assert_eq!(headings[2]["end_line"].as_u64().unwrap(), 7);
    }

    #[test]
    fn markdown_summary_includes_h3_headings() {
        let content = "# Top\n## Mid\n### Deep\ntext\n## Other";
        let s = summarize_markdown(content);
        let headings = s["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 4);
        assert_eq!(headings[2]["heading"].as_str().unwrap(), "### Deep");
        assert_eq!(headings[2]["level"].as_u64().unwrap(), 3);
    }

    #[test]
    fn markdown_summary_ignores_headings_in_code_blocks() {
        let content = "# Real\n```\n# Not a heading\n## Also not\n```\n## Real Too";
        let s = summarize_markdown(content);
        let headings = s["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0]["heading"].as_str().unwrap(), "# Real");
        assert_eq!(headings[1]["heading"].as_str().unwrap(), "## Real Too");
    }

    #[test]
    fn config_summary_returns_first_30_lines() {
        let content: String = (1..=50).map(|i| format!("key_{} = {}\n", i, i)).collect();
        let s = summarize_config(&content);
        let preview = s["preview"].as_str().unwrap();
        assert!(preview.contains("key_1"));
        assert!(!preview.contains("key_31"));
        assert!(
            preview.contains("key_30"),
            "preview should include up to line 30"
        );
        assert_eq!(s["line_count"].as_u64().unwrap(), 50);
    }

    #[test]
    fn generic_summary_includes_head_and_tail() {
        let content: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        let s = summarize_generic_file(&content);
        assert!(s["head"].as_str().unwrap().contains("line 1"));
        assert!(!s["head"].as_str().unwrap().contains("line 21"));
        assert!(
            s["head"].as_str().unwrap().contains("line 20"),
            "head should include line 20"
        );
        assert!(s["tail"].as_str().unwrap().contains("line 100"));
        assert!(
            !s["tail"].as_str().unwrap().contains("line 90"),
            "tail should not include line 90"
        );
        assert!(
            s["tail"].as_str().unwrap().contains("line 91"),
            "tail should start at line 91"
        );
        assert_eq!(s["line_count"].as_u64().unwrap(), 100);
    }

    #[test]
    fn json_summary_shows_top_level_keys() {
        let content = r#"{
  "name": "my-project",
  "version": "1.0.0",
  "dependencies": {
    "serde": "1.0",
    "tokio": "1.0"
  },
  "scripts": {
    "build": "cargo build"
  }
}"#;
        let s = summarize_json(content);
        assert_eq!(s["type"].as_str().unwrap(), "json");
        let schema = &s["schema"];
        assert_eq!(schema["root_type"].as_str().unwrap(), "object");
        let keys = schema["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 4);
        assert_eq!(keys[0]["path"].as_str().unwrap(), "$.name");
        assert_eq!(keys[0]["type"].as_str().unwrap(), "string");
        assert_eq!(keys[2]["path"].as_str().unwrap(), "$.dependencies");
        assert_eq!(keys[2]["type"].as_str().unwrap(), "object");
        assert_eq!(keys[2]["count"].as_u64().unwrap(), 2);
    }

    #[test]
    fn json_summary_handles_root_array() {
        let content = r#"[{"id": 1}, {"id": 2}, {"id": 3}]"#;
        let s = summarize_json(content);
        let schema = &s["schema"];
        assert_eq!(schema["root_type"].as_str().unwrap(), "array");
        assert_eq!(schema["count"].as_u64().unwrap(), 3);
        assert_eq!(schema["element_type"].as_str().unwrap(), "object");
    }

    #[test]
    fn json_summary_handles_malformed_json() {
        let content = "{ not valid json !!";
        let s = summarize_json(content);
        assert_eq!(s["type"].as_str().unwrap(), "json");
        assert!(s["head"].is_string()); // generic fallback shape
    }

    #[test]
    fn toml_summary_shows_tables() {
        let content = "[package]\nname = \"foo\"\nversion = \"1.0\"\n\n[dependencies]\nserde = \"1.0\"\ntokio = \"1.0\"\n\n[dev-dependencies]\ntempfile = \"3\"";
        let s = summarize_toml(content);
        assert_eq!(s["type"].as_str().unwrap(), "toml");
        assert_eq!(s["format"].as_str().unwrap(), "toml");
        let sections = s["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0]["key"].as_str().unwrap(), "[package]");
        assert!(sections[0]["line"].as_u64().unwrap() >= 1);
        assert!(sections[0]["end_line"].as_u64().is_some());
    }

    #[test]
    fn toml_summary_handles_nested_tables() {
        let content = "[package]\nname = \"foo\"\n\n[profile.release]\nopt-level = 3\nlto = true";
        let s = summarize_toml(content);
        let sections = s["sections"].as_array().unwrap();
        assert!(sections
            .iter()
            .any(|s| s["key"].as_str().unwrap() == "[profile.release]"));
    }

    #[test]
    fn toml_summary_handles_malformed() {
        let content = "not valid toml [[[";
        let s = summarize_toml(content);
        assert_eq!(s["type"].as_str().unwrap(), "toml");
        assert!(s["line_count"].as_u64().is_some());
    }

    #[test]
    fn yaml_summary_shows_top_level_keys() {
        let content =
            "database:\n  host: localhost\n  port: 5432\nserver:\n  port: 8080\nlogging:\n  level: debug";
        let s = summarize_yaml(content);
        assert_eq!(s["type"].as_str().unwrap(), "yaml");
        assert_eq!(s["format"].as_str().unwrap(), "yaml");
        let sections = s["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0]["key"].as_str().unwrap(), "database");
        assert_eq!(sections[0]["line"].as_u64().unwrap(), 1);
        assert_eq!(sections[0]["end_line"].as_u64().unwrap(), 3);
        assert_eq!(sections[1]["key"].as_str().unwrap(), "server");
        assert_eq!(sections[2]["key"].as_str().unwrap(), "logging");
    }

    #[test]
    fn yaml_summary_skips_comments_and_directives() {
        let content = "---\n# A comment\nfoo:\n  bar: 1\nbaz:\n  qux: 2\n...";
        let s = summarize_yaml(content);
        let sections = s["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0]["key"].as_str().unwrap(), "foo");
        assert_eq!(sections[1]["key"].as_str().unwrap(), "baz");
    }

    #[test]
    fn yaml_summary_handles_empty_file() {
        let content = "# just a comment\n---";
        let s = summarize_yaml(content);
        assert_eq!(s["type"].as_str().unwrap(), "yaml");
        // Falls back to generic
        assert!(s["head"].is_string());
    }

    #[test]
    fn extract_markdown_section_exact_match() {
        let content = "# Intro\nwelcome\n## Setup\ndo this\nand that\n## Usage\nuse it";
        let result = extract_markdown_section(content, "## Setup").unwrap();
        assert_eq!(result.content, "## Setup\ndo this\nand that");
        assert_eq!(result.line_range, (3, 5));
        assert_eq!(result.breadcrumb, vec!["# Intro", "## Setup"]);
        assert_eq!(result.siblings, vec!["## Usage"]);
    }

    #[test]
    fn extract_markdown_section_prefix_match() {
        let content = "# Title\n## Authentication Guide\ndetails here";
        let result = extract_markdown_section(content, "## Auth").unwrap();
        assert!(result.content.contains("Authentication Guide"));
    }

    #[test]
    fn extract_markdown_section_not_found() {
        let content = "# Title\n## Setup\ntext";
        let result = extract_markdown_section(content, "## Nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn extract_markdown_section_no_headings() {
        let content = "just some text\nno headings here";
        let result = extract_markdown_section(content, "## Anything");
        assert!(result.is_err());
    }

    #[test]
    fn extract_json_path_top_level_key() {
        let content = r#"{"name": "test", "deps": {"a": 1, "b": 2}}"#;
        let (result, type_name, count) = extract_json_path(content, "$.deps").unwrap();
        assert!(result.contains("\"a\""));
        assert!(result.contains("\"b\""));
        assert_eq!(type_name, "object");
        assert_eq!(count, Some(2));
    }

    #[test]
    fn extract_json_path_nested() {
        let content = r#"{"db": {"connection": {"host": "localhost", "port": 5432}}}"#;
        let (result, _, _) = extract_json_path(content, "$.db.connection").unwrap();
        assert!(result.contains("localhost"));
    }

    #[test]
    fn extract_json_path_array_index() {
        let content = r#"{"users": [{"name": "alice"}, {"name": "bob"}]}"#;
        let (result, _, _) = extract_json_path(content, "$.users[0]").unwrap();
        assert!(result.contains("alice"));
        assert!(!result.contains("bob"));
    }

    #[test]
    fn extract_json_path_not_found() {
        let content = r#"{"name": "test"}"#;
        let result = extract_json_path(content, "$.nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn extract_json_path_root() {
        let content = r#"{"a": 1}"#;
        let (result, type_name, count) = extract_json_path(content, "$").unwrap();
        assert!(result.contains("\"a\""));
        assert_eq!(type_name, "object");
        assert_eq!(count, Some(1));
    }

    #[test]
    fn extract_toml_key_table() {
        let content =
            "[package]\nname = \"foo\"\n\n[dependencies]\nserde = \"1.0\"\ntokio = \"1.0\"";
        let result = extract_toml_key(content, "dependencies").unwrap();
        assert!(result.content.contains("serde"));
        assert!(result.content.contains("tokio"));
        assert_eq!(result.format, "toml");
        assert!(result.siblings.iter().any(|s| s.contains("package")));
    }

    #[test]
    fn extract_toml_key_not_found() {
        let content = "[package]\nname = \"foo\"";
        let result = extract_toml_key(content, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn extract_yaml_key_section() {
        let content = "database:\n  host: localhost\n  port: 5432\nserver:\n  port: 8080";
        let result = extract_yaml_key(content, "database").unwrap();
        assert!(result.content.contains("host"));
        assert!(result.content.contains("localhost"));
        assert_eq!(result.format, "yaml");
        assert!(result.siblings.iter().any(|s| s == "server"));
    }

    #[test]
    fn extract_yaml_key_not_found() {
        let content = "database:\n  host: localhost\nserver:\n  port: 8080";
        let result = extract_yaml_key(content, "nonexistent");
        assert!(result.is_err());
    }
}
