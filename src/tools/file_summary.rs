use serde_json::Value;

pub const FILE_BUFFER_THRESHOLD: usize = 200;

pub enum FileSummaryType {
    Source,
    Markdown,
    Config,
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
    const CONFIG_EXTS: &[&str] = &[
        ".toml", ".yaml", ".yml", ".json", ".xml", ".ini", ".env", ".lock", ".cfg",
    ];
    if SOURCE_EXTS.iter().any(|e| lower.ends_with(e)) {
        FileSummaryType::Source
    } else if lower.ends_with(".md") || lower.ends_with(".mdx") {
        FileSummaryType::Markdown
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
    let headings: Vec<String> = content
        .lines()
        .filter(|l| l.starts_with("# ") || l.starts_with("## "))
        .take(20)
        .map(|l| l.to_string())
        .collect();
    serde_json::json!({
        "type": "markdown",
        "line_count": line_count,
        "headings": headings,
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
    fn detect_toml_as_config() {
        assert!(matches!(
            detect_file_type("Cargo.toml"),
            FileSummaryType::Config
        ));
        assert!(matches!(
            detect_file_type("config.yaml"),
            FileSummaryType::Config
        ));
        assert!(matches!(
            detect_file_type("data.json"),
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
    fn markdown_summary_extracts_h1_and_h2_only() {
        let content = "# Title\nsome text\n## Section\nmore text\n### Sub\nnope";
        let s = summarize_markdown(content);
        let headings = s["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].as_str().unwrap(), "# Title");
        assert_eq!(headings[1].as_str().unwrap(), "## Section");
        assert_eq!(s["line_count"].as_u64().unwrap(), 6);
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
}
