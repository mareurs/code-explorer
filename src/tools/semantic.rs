//! Semantic search tools backed by the embedding index.

use anyhow::anyhow;
use serde_json::{json, Value};
use super::{Tool, ToolContext};

pub struct SemanticSearch;
pub struct IndexProject;
pub struct IndexStatus;

#[async_trait::async_trait]
impl Tool for SemanticSearch {
    fn name(&self) -> &str { "semantic_search" }
    fn description(&self) -> &str {
        "Find code by natural language description or code snippet. \
         Returns ranked chunks with file path, line range, and similarity score."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language description or code snippet to search for"
                },
                "limit": { "type": "integer", "default": 10 },
                "refresh": { "type": "boolean", "default": false,
                    "description": "Incrementally re-index changed files before searching" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("semantic_search: not yet wired to embed::index (run index_project first)"))
    }
}

#[async_trait::async_trait]
impl Tool for IndexProject {
    fn name(&self) -> &str { "index_project" }
    fn description(&self) -> &str {
        "Build or incrementally update the semantic search index for the active project."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "force": { "type": "boolean", "default": false,
                    "description": "Force full reindex, ignoring cached file hashes" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("index_project: not yet wired to embed::index::build_index"))
    }
}

#[async_trait::async_trait]
impl Tool for IndexStatus {
    fn name(&self) -> &str { "index_status" }
    fn description(&self) -> &str {
        "Show index stats: file count, chunk count, model, last update."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("index_status: not yet implemented"))
    }
}
