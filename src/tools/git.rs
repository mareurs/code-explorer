//! Git tools: blame, log, diff.

use serde_json::{json, Value};
use std::path::Path;
use super::{Tool, ToolContext};

pub struct GitBlame;
pub struct GitLog;
pub struct GitDiff;

#[async_trait::async_trait]
impl Tool for GitBlame {
    fn name(&self) -> &str { "git_blame" }
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
                "end_line": { "type": "integer" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file = input["path"].as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' parameter"))?;
        let root = ctx.agent.require_project_root().await?;

        let lines = crate::git::blame::blame_file(&root, Path::new(file))?;

        // Optional line range filter
        let start = input["start_line"].as_u64().map(|n| n as usize);
        let end = input["end_line"].as_u64().map(|n| n as usize);
        let filtered: Vec<_> = lines.into_iter().filter(|l| {
            if let Some(s) = start { if l.line < s { return false; } }
            if let Some(e) = end { if l.line > e { return false; } }
            true
        }).collect();

        Ok(json!({ "lines": filtered, "total": filtered.len() }))
    }
}

#[async_trait::async_trait]
impl Tool for GitLog {
    fn name(&self) -> &str { "git_log" }
    fn description(&self) -> &str { "Show commit history for a file or the whole project." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (omit for project-wide log)" },
                "limit": { "type": "integer", "default": 20 }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let root = ctx.agent.require_project_root().await?;
        let limit = input["limit"].as_u64().unwrap_or(20) as usize;

        let repo = crate::git::open_repo(&root)?;

        if let Some(path) = input["path"].as_str() {
            let commits = crate::git::file_log(&repo, Path::new(path), limit)?;
            Ok(json!({ "commits": commits, "file": path }))
        } else {
            // Project-wide log: walk HEAD without file filtering
            let mut revwalk = repo.revwalk()?;
            revwalk.push_head()?;
            revwalk.set_sorting(git2::Sort::TIME)?;

            let mut commits = vec![];
            for oid in revwalk.take(limit) {
                let oid = oid?;
                let commit = repo.find_commit(oid)?;
                commits.push(crate::git::CommitSummary {
                    sha: format!("{:.8}", commit.id()),
                    message: commit.summary().unwrap_or("<no message>").to_string(),
                    author: commit.author().name().unwrap_or("unknown").to_string(),
                    timestamp: commit.time().seconds(),
                });
            }
            Ok(json!({ "commits": commits }))
        }
    }
}

#[async_trait::async_trait]
impl Tool for GitDiff {
    fn name(&self) -> &str { "git_diff" }
    fn description(&self) -> &str {
        "Show the diff of uncommitted changes, or against a specific commit."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Restrict diff to this file (optional)" },
                "commit": { "type": "string", "description": "Commit SHA to diff against (default: HEAD)" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let root = ctx.agent.require_project_root().await?;
        let repo = crate::git::open_repo(&root)?;

        let file = input["path"].as_str().map(Path::new);
        let commit = input["commit"].as_str();

        let diff_text = crate::git::diff_workdir(&repo, file, commit)?;
        Ok(json!({ "diff": diff_text }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
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
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[]).unwrap();

        // Create .code-explorer dir for agent
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (dir, ToolContext { agent })
    }

    #[tokio::test]
    async fn blame_returns_lines() {
        let (_dir, ctx) = git_test_ctx().await;
        let result = GitBlame.call(json!({ "path": "hello.rs" }), &ctx).await.unwrap();
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
        std::fs::write(dir.path().join("hello.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("hello.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add functions", &tree, &[&head]).unwrap();

        let result = GitBlame.call(json!({
            "path": "hello.rs",
            "start_line": 2,
            "end_line": 2
        }), &ctx).await.unwrap();
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["line"], 2);
    }

    #[tokio::test]
    async fn log_returns_commits() {
        let (_dir, ctx) = git_test_ctx().await;
        let result = GitLog.call(json!({ "path": "hello.rs" }), &ctx).await.unwrap();
        let commits = result["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0]["message"], "initial commit");
    }

    #[tokio::test]
    async fn log_project_wide() {
        let (_dir, ctx) = git_test_ctx().await;
        let result = GitLog.call(json!({}), &ctx).await.unwrap();
        let commits = result["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 1);
    }

    #[tokio::test]
    async fn diff_shows_uncommitted_changes() {
        let (dir, ctx) = git_test_ctx().await;
        // Modify the file without committing
        std::fs::write(dir.path().join("hello.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();

        let result = GitDiff.call(json!({}), &ctx).await.unwrap();
        let diff = result["diff"].as_str().unwrap();
        assert!(diff.contains("+"), "diff should contain additions");
    }

    #[tokio::test]
    async fn diff_empty_when_clean() {
        let (_dir, ctx) = git_test_ctx().await;
        let result = GitDiff.call(json!({}), &ctx).await.unwrap();
        let diff = result["diff"].as_str().unwrap();
        assert!(diff.is_empty(), "clean workdir should have empty diff");
    }

    #[tokio::test]
    async fn tools_error_without_project() {
        let ctx = ToolContext { agent: Agent::new(None).await.unwrap() };
        assert!(GitBlame.call(json!({ "path": "x" }), &ctx).await.is_err());
        assert!(GitLog.call(json!({}), &ctx).await.is_err());
        assert!(GitDiff.call(json!({}), &ctx).await.is_err());
    }
}
