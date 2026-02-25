//! Symbol-level tools backed by the LSP client.

use anyhow::anyhow;
use serde_json::{json, Value};
use super::{Tool, ToolContext};

pub struct FindSymbol;
pub struct FindReferencingSymbols;
pub struct GetSymbolsOverview;
pub struct ReplaceSymbolBody;
pub struct InsertBeforeSymbol;
pub struct InsertAfterSymbol;
pub struct RenameSymbol;

#[async_trait::async_trait]
impl Tool for FindSymbol {
    fn name(&self) -> &str { "find_symbol" }
    fn description(&self) -> &str {
        "Find symbols by name pattern across the project. Supports substring and regex matching."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string" },
                "substring_matching": { "type": "boolean", "default": true },
                "relative_path": { "type": "string", "description": "Restrict search to this file or directory" },
                "include_body": { "type": "boolean", "default": false },
                "depth": { "type": "integer", "default": 0 }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("find_symbol: not yet implemented (requires LSP)"))
    }
}

#[async_trait::async_trait]
impl Tool for FindReferencingSymbols {
    fn name(&self) -> &str { "find_referencing_symbols" }
    fn description(&self) -> &str { "Find all symbols that reference the given symbol." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("find_referencing_symbols: not yet implemented (requires LSP)"))
    }
}

#[async_trait::async_trait]
impl Tool for GetSymbolsOverview {
    fn name(&self) -> &str { "get_symbols_overview" }
    fn description(&self) -> &str { "Return a tree of top-level symbols in a file or directory." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "relative_path": { "type": "string" },
                "depth": { "type": "integer", "default": 1 }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("get_symbols_overview: not yet implemented (requires LSP or tree-sitter)"))
    }
}

#[async_trait::async_trait]
impl Tool for ReplaceSymbolBody {
    fn name(&self) -> &str { "replace_symbol_body" }
    fn description(&self) -> &str {
        "Replace the entire body of a named symbol with new source code."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "new_body"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "new_body": { "type": "string" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("replace_symbol_body: not yet implemented"))
    }
}

#[async_trait::async_trait]
impl Tool for InsertBeforeSymbol {
    fn name(&self) -> &str { "insert_before_symbol" }
    fn description(&self) -> &str { "Insert code immediately before a named symbol." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "code"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "code": { "type": "string" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("insert_before_symbol: not yet implemented"))
    }
}

#[async_trait::async_trait]
impl Tool for InsertAfterSymbol {
    fn name(&self) -> &str { "insert_after_symbol" }
    fn description(&self) -> &str { "Insert code immediately after a named symbol." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "code"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "code": { "type": "string" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("insert_after_symbol: not yet implemented"))
    }
}

#[async_trait::async_trait]
impl Tool for RenameSymbol {
    fn name(&self) -> &str { "rename_symbol" }
    fn description(&self) -> &str { "Rename a symbol across the entire codebase." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name_path", "relative_path", "new_name"],
            "properties": {
                "name_path": { "type": "string" },
                "relative_path": { "type": "string" },
                "new_name": { "type": "string" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("rename_symbol: not yet implemented (requires LSP)"))
    }
}
