//! File system tools: read, write, search, list.

use anyhow::Result;
use serde_json::{json, Value};

use crate::util::text::extract_lines;
use super::{Tool, ToolContext};

// ── read_file ────────────────────────────────────────────────────────────────

pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str { "read_file" }

    fn description(&self) -> &str {
        "Read the contents of a file. Optionally restrict to a line range."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "File path relative to project root" },
                "start_line": { "type": "integer", "description": "First line to return (1-indexed)" },
                "end_line": { "type": "integer", "description": "Last line to return (1-indexed, inclusive)" }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> Result<Value> {
        let path = input["path"].as_str().ok_or_else(|| anyhow::anyhow!("missing path"))?;
        let text = std::fs::read_to_string(path)?;
        let content = match (input["start_line"].as_u64(), input["end_line"].as_u64()) {
            (Some(start), Some(end)) => extract_lines(&text, start as usize, end as usize),
            _ => text,
        };
        Ok(json!({ "content": content }))
    }
}

// ── list_dir ────────────────────────────────────────────────────────────────

pub struct ListDir;

#[async_trait::async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str { "list_dir" }

    fn description(&self) -> &str {
        "List files and directories. Pass recursive=true for a full tree."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string" },
                "recursive": { "type": "boolean", "default": false }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> Result<Value> {
        let path = input["path"].as_str().unwrap_or(".");
        let recursive = input["recursive"].as_bool().unwrap_or(false);
        let depth = if recursive { usize::MAX } else { 1 };

        let entries: Vec<String> = walkdir::WalkDir::new(path)
            .max_depth(depth)
            .into_iter()
            .flatten()
            .filter(|e| e.depth() > 0)
            .map(|e| {
                let suffix = if e.file_type().is_dir() { "/" } else { "" };
                format!("{}{}", e.path().display(), suffix)
            })
            .collect();

        Ok(json!({ "entries": entries }))
    }
}

// ── search_for_pattern ───────────────────────────────────────────────────────

pub struct SearchForPattern;

#[async_trait::async_trait]
impl Tool for SearchForPattern {
    fn name(&self) -> &str { "search_for_pattern" }

    fn description(&self) -> &str {
        "Search the codebase for a regex pattern. Returns matching lines with file and line number."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern" },
                "path": { "type": "string", "description": "Directory to search (default: project root)" },
                "max_results": { "type": "integer", "default": 50 }
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> Result<Value> {
        let pattern = input["pattern"].as_str().ok_or_else(|| anyhow::anyhow!("missing pattern"))?;
        let search_path = input["path"].as_str().unwrap_or(".");
        let max = input["max_results"].as_u64().unwrap_or(50) as usize;

        let re = regex::Regex::new(pattern)?;
        let mut matches = vec![];

        let walker = ignore::WalkBuilder::new(search_path).build();
        'outer: for entry in walker.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    matches.push(json!({
                        "file": entry.path().display().to_string(),
                        "line": i + 1,
                        "content": line
                    }));
                    if matches.len() >= max {
                        break 'outer;
                    }
                }
            }
        }

        Ok(json!({ "matches": matches, "total": matches.len() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use serde_json::json;
    use tempfile::tempdir;

    async fn test_ctx() -> ToolContext {
        ToolContext { agent: Agent::new(None).await.unwrap() }
    }

    // ── ReadFile ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_returns_full_content() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "hello world").unwrap();

        let result = ReadFile.call(json!({ "path": file.to_str().unwrap() }), &ctx).await.unwrap();
        assert_eq!(result["content"], "hello world");
    }

    #[tokio::test]
    async fn read_file_with_line_range() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let file = dir.path().join("lines.txt");
        std::fs::write(&file, "line1\nline2\nline3\nline4\nline5").unwrap();

        let result = ReadFile.call(json!({
            "path": file.to_str().unwrap(),
            "start_line": 2,
            "end_line": 4
        }), &ctx).await.unwrap();

        assert_eq!(result["content"], "line2\nline3\nline4");
    }

    #[tokio::test]
    async fn read_file_missing_errors() {
        let ctx = test_ctx().await;
        let result = ReadFile.call(json!({ "path": "/no/such/file.txt" }), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_missing_path_param_errors() {
        let ctx = test_ctx().await;
        let result = ReadFile.call(json!({}), &ctx).await;
        assert!(result.is_err());
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
            .call(json!({ "path": dir.path().to_str().unwrap(), "recursive": false }), &ctx)
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
            .call(json!({ "path": dir.path().to_str().unwrap(), "recursive": true }), &ctx)
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

    // ── SearchForPattern ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_finds_matching_line() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn main() {}\nlet x = 42;\n").unwrap();

        let result = SearchForPattern
            .call(json!({ "pattern": "fn main", "path": dir.path().to_str().unwrap() }), &ctx)
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

        let result = SearchForPattern
            .call(json!({ "pattern": "xyz_not_present", "path": dir.path().to_str().unwrap() }), &ctx)
            .await
            .unwrap();

        assert_eq!(result["matches"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn search_respects_max_results() {
        let ctx = test_ctx().await;
        let dir = tempdir().unwrap();
        let content = (0..20).map(|i| format!("match_{}", i)).collect::<Vec<_>>().join("\n");
        std::fs::write(dir.path().join("data.txt"), &content).unwrap();

        let result = SearchForPattern
            .call(json!({
                "pattern": "match_",
                "path": dir.path().to_str().unwrap(),
                "max_results": 5
            }), &ctx)
            .await
            .unwrap();

        assert_eq!(result["matches"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn search_invalid_regex_errors() {
        let ctx = test_ctx().await;
        let result = SearchForPattern.call(json!({ "pattern": "[invalid" }), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn search_missing_pattern_errors() {
        let ctx = test_ctx().await;
        let result = SearchForPattern.call(json!({}), &ctx).await;
        assert!(result.is_err());
    }
}
