//! Memory tools: persistent per-project knowledge store.

use anyhow::anyhow;
use serde_json::{json, Value};
use super::{Tool, ToolContext};

pub struct WriteMemory;
pub struct ReadMemory;
pub struct ListMemories;
pub struct DeleteMemory;

#[async_trait::async_trait]
impl Tool for WriteMemory {
    fn name(&self) -> &str { "write_memory" }
    fn description(&self) -> &str {
        "Persist a piece of knowledge about the project. \
         Topic is a path-like string, e.g. 'debugging/async-patterns'."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["topic", "content"],
            "properties": {
                "topic": { "type": "string" },
                "content": { "type": "string" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("write_memory: not yet wired to MemoryStore"))
    }
}

#[async_trait::async_trait]
impl Tool for ReadMemory {
    fn name(&self) -> &str { "read_memory" }
    fn description(&self) -> &str { "Read a stored memory entry by topic." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["topic"],
            "properties": { "topic": { "type": "string" } }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("read_memory: not yet wired to MemoryStore"))
    }
}

#[async_trait::async_trait]
impl Tool for ListMemories {
    fn name(&self) -> &str { "list_memories" }
    fn description(&self) -> &str { "List all stored memory topics for the active project." }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("list_memories: not yet wired to MemoryStore"))
    }
}

#[async_trait::async_trait]
impl Tool for DeleteMemory {
    fn name(&self) -> &str { "delete_memory" }
    fn description(&self) -> &str { "Delete a memory entry by topic." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["topic"],
            "properties": { "topic": { "type": "string" } }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("delete_memory: not yet wired to MemoryStore"))
    }
}
