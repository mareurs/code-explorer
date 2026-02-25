//! Git tools: blame, log, diff.

use anyhow::anyhow;
use serde_json::{json, Value};
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
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("git_blame: not yet wired to git::blame::blame_file"))
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
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("git_log: not yet wired to git::file_log"))
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
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("git_diff: not yet wired to git2 diff"))
    }
}
