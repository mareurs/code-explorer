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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use std::sync::Arc;
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

        // Create .code-explorer dir for agent
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: LspManager::new_arc(),
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
        };
        assert!(GitBlame.call(json!({ "path": "x" }), &ctx).await.is_err());
    }
}
