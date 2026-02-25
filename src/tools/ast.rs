//! AST tools backed by tree-sitter.

use anyhow::anyhow;
use serde_json::{json, Value};
use super::{Tool, ToolContext};

pub struct ListFunctions;
pub struct ExtractDocstrings;

#[async_trait::async_trait]
impl Tool for ListFunctions {
    fn name(&self) -> &str { "list_functions" }
    fn description(&self) -> &str {
        "List all function/method signatures in a file using tree-sitter."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": { "path": { "type": "string" } }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("list_functions: not yet wired to tree-sitter"))
    }
}

#[async_trait::async_trait]
impl Tool for ExtractDocstrings {
    fn name(&self) -> &str { "extract_docstrings" }
    fn description(&self) -> &str {
        "Extract all docstrings and top-level comments from a file."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": { "path": { "type": "string" } }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("extract_docstrings: not yet wired to tree-sitter"))
    }
}
