//! Git tools: blame, log, diff.

use super::{Tool, ToolContext};
use serde_json::{json, Value};
use std::path::Path;

pub struct GitBlame;

#[async_trait::async_trait]
impl Tool for GitBlame {
    fn name(&self) -> &str {
        "git_blame"
    }
    fn description(&self) -> &str {
        "Return line-level blame for a file: who last changed each line and in which commit."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "File path relative to project root" },
                "start_line": { "type": "integer" },
                "end_line": { "type": "integer" },
                "detail_level": { "type": "string", "description": "Output detail: omit for compact (default), 'full' for all lines" },
                "offset": { "type": "integer", "description": "Skip this many lines (focused mode pagination)" },
                "limit": { "type": "integer", "description": "Max lines per page (focused mode, default 50)" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file = super::require_str_param(&input, "path")?;
        let root = ctx.agent.require_project_root().await?;

        let security = ctx.agent.security_config().await;
        crate::util::path_security::validate_read_path(file, Some(&root), &security)?;

        // git2 requires a project-relative path. If the caller passed an absolute
        // path (which validate_read_path accepts), strip the project root prefix.
        let rel_path = {
            let p = Path::new(file);
            if p.is_absolute() {
                p.strip_prefix(&root).map_err(|_| {
                    super::RecoverableError::with_hint(
                        format!("path `{}` is outside the project root", file),
                        format!(
                            "git_blame requires a project-relative path (e.g. `src/foo.rs`). \
                             Got absolute path `{}` which is not under `{}`.",
                            file,
                            root.display()
                        ),
                    )
                })?
            } else {
                p
            }
        };

        let lines = crate::git::blame::blame_file(&root, rel_path)?;

        // Optional line range filter
        let start = input["start_line"].as_u64().map(|n| n as usize);
        let end = input["end_line"].as_u64().map(|n| n as usize);
        let filtered: Vec<_> = lines
            .into_iter()
            .filter(|l| {
                if let Some(s) = start {
                    if l.line < s {
                        return false;
                    }
                }
                if let Some(e) = end {
                    if l.line > e {
                        return false;
                    }
                }
                true
            })
            .collect();

        let guard = super::output::OutputGuard::from_input(&input);
        let total = filtered.len();
        let (filtered, overflow) = guard.cap_items(
            filtered,
            "Use start_line/end_line to narrow, or detail_level='full' for all lines",
        );
        let mut result = json!({ "lines": filtered, "total": total });
        if let Some(ov) = overflow {
            result["overflow"] = super::output::OutputGuard::overflow_json(&ov);
        }
        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_git_blame(result))
    }
}

fn format_git_blame(result: &Value) -> String {
    let file = result["file"].as_str().unwrap_or("?");
    let lines = match result["lines"].as_array() {
        Some(l) => l,
        None => return file.to_string(),
    };
    let line_count = lines.len();

    let mut author_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for l in lines.iter() {
        if let Some(author) = l["author"].as_str() {
            *author_counts.entry(author).or_insert(0) += 1;
        }
    }

    if author_counts.len() <= 1 {
        let author_note = author_counts
            .keys()
            .next()
            .map(|a| format!(" · {a}"))
            .unwrap_or_default();
        return format!("{file} · {line_count} lines{author_note}");
    }

    let mut authors: Vec<(&str, usize)> = author_counts.into_iter().collect();
    authors.sort_by(|a, b| b.1.cmp(&a.1));

    let name_width = authors.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let mut out = format!("{file} · {line_count} lines");
    const MAX_AUTHORS: usize = 5;
    for (author, count) in authors.iter().take(MAX_AUTHORS) {
        let label = if *count == 1 { "line" } else { "lines" };
        out.push_str(&format!("\n  {author:<name_width$}  {count} {label}"));
    }
    let hidden = authors.len().saturating_sub(MAX_AUTHORS);
    if hidden > 0 {
        out.push_str(&format!("\n  … +{hidden} more authors"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use tempfile::tempdir;

    /// Create a temp git repo with one commit and return the context.
    async fn git_test_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // Create a file and commit it
        let file_path = dir.path().join("hello.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("hello.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();

        // Create .codescout dir for agent
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
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

    #[tokio::test]
    async fn blame_returns_lines() {
        let (_dir, ctx) = git_test_ctx().await;
        let result = GitBlame
            .call(json!({ "path": "hello.rs" }), &ctx)
            .await
            .unwrap();
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["line"], 1);
        assert!(lines[0]["content"].as_str().unwrap().contains("fn main"));
        assert_eq!(lines[0]["author"], "Test");
    }

    #[tokio::test]
    async fn blame_with_line_range() {
        let (dir, ctx) = git_test_ctx().await;
        // Add more lines
        std::fs::write(
            dir.path().join("hello.rs"),
            "fn a() {}\nfn b() {}\nfn c() {}\n",
        )
        .unwrap();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("hello.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add functions", &tree, &[&head])
            .unwrap();

        let result = GitBlame
            .call(
                json!({
                    "path": "hello.rs",
                    "start_line": 2,
                    "end_line": 2
                }),
                &ctx,
            )
            .await
            .unwrap();
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["line"], 2);
    }

    #[tokio::test]
    async fn blame_accepts_absolute_path() {
        let (dir, ctx) = git_test_ctx().await;
        let abs_path = dir.path().join("hello.rs");
        let result = GitBlame
            .call(json!({ "path": abs_path.to_str().unwrap() }), &ctx)
            .await
            .unwrap();
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0]["content"].as_str().unwrap().contains("fn main"));
    }

    #[tokio::test]
    async fn tools_error_without_project() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        assert!(GitBlame.call(json!({ "path": "x" }), &ctx).await.is_err());
    }

    #[test]
    fn git_blame_format_compact_shows_lines() {
        let tool = GitBlame;
        let result = json!({ "lines": [{"line":1},{"line":2}], "file": "src/a.rs" });
        let text = tool.format_compact(&result).unwrap();
        assert!(text.contains("2"), "got: {text}");
        assert!(text.contains("src/a.rs"), "got: {text}");
    }

    // BUG-017: git_blame must work when the active project root is a subdirectory
    // of the git repo root. `blame_file` must compute a repo-relative path that
    // prefixes the project-root offset so git2 can find the file in the tree.
    #[tokio::test]
    async fn blame_works_when_project_root_is_git_subdirectory() {
        let dir = tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // Create a file inside a subdirectory
        let subdir = dir.path().join("subproject");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(
            subdir.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let mut index = repo.index().unwrap();
        index.add_path(Path::new("subproject/lib.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();

        // Activate project root at the subdirectory (not the git repo root)
        std::fs::create_dir_all(subdir.join(".codescout")).unwrap();
        let agent = Agent::new(Some(subdir.clone())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };

        let result = GitBlame
            .call(json!({ "path": "lib.rs" }), &ctx)
            .await
            .expect("blame should work when project root is a git subdirectory");

        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0]["content"].as_str().unwrap().contains("add"));
    }

    // --- format_git_blame tests ---

    #[test]
    fn format_git_blame_shows_author_breakdown() {
        let lines: Vec<serde_json::Value> = vec![
            serde_json::json!({"author": "alice", "line": 1}),
            serde_json::json!({"author": "alice", "line": 2}),
            serde_json::json!({"author": "alice", "line": 3}),
            serde_json::json!({"author": "bob", "line": 4}),
            serde_json::json!({"author": "bob", "line": 5}),
            serde_json::json!({"author": "carol", "line": 6}),
        ];
        let result = serde_json::json!({ "file": "src/main.rs", "lines": lines });
        let out = format_git_blame(&result);
        assert!(out.contains("src/main.rs"), "should show file");
        assert!(out.contains("alice"), "should show author");
        assert!(out.contains("bob"), "should show author");
        assert!(out.contains("carol"), "should show author");
    }

    #[test]
    fn format_git_blame_single_author_no_breakdown() {
        let lines: Vec<serde_json::Value> = (0..5)
            .map(|i| serde_json::json!({"author": "solo", "line": i}))
            .collect();
        let result = serde_json::json!({ "file": "src/lib.rs", "lines": lines });
        let out = format_git_blame(&result);
        assert!(out.contains("src/lib.rs"), "should show file");
        assert!(!out.contains('\n'), "no breakdown for single author");
    }
}
