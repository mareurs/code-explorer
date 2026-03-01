//! Memory tools: persistent per-project knowledge store.

use super::{Tool, ToolContext};
use serde_json::{json, Value};

pub struct WriteMemory;
pub struct ReadMemory;
pub struct ListMemories;
pub struct DeleteMemory;

#[async_trait::async_trait]
impl Tool for WriteMemory {
    fn name(&self) -> &str {
        "write_memory"
    }
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
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let topic = super::require_str_param(&input, "topic")?;
        let content = super::require_str_param(&input, "content")?;
        ctx.agent
            .with_project(|p| {
                p.memory.write(topic, content)?;
                Ok(json!({ "status": "ok", "topic": topic }))
            })
            .await
    }
}

#[async_trait::async_trait]
impl Tool for ReadMemory {
    fn name(&self) -> &str {
        "read_memory"
    }
    fn description(&self) -> &str {
        "Read a stored memory entry by topic."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["topic"],
            "properties": { "topic": { "type": "string" } }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let topic = super::require_str_param(&input, "topic")?;
        ctx.agent
            .with_project(|p| match p.memory.read(topic)? {
                Some(content) => Ok(json!({ "topic": topic, "content": content })),
                None => Ok(json!({ "topic": topic, "content": null, "message": "not found" })),
            })
            .await
    }
}

#[async_trait::async_trait]
impl Tool for ListMemories {
    fn name(&self) -> &str {
        "list_memories"
    }
    fn description(&self) -> &str {
        "List all stored memory topics for the active project."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        ctx.agent
            .with_project(|p| {
                let topics = p.memory.list()?;
                Ok(json!({ "topics": topics }))
            })
            .await
    }
}

#[async_trait::async_trait]
impl Tool for DeleteMemory {
    fn name(&self) -> &str {
        "delete_memory"
    }
    fn description(&self) -> &str {
        "Delete a memory entry by topic."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["topic"],
            "properties": { "topic": { "type": "string" } }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let topic = super::require_str_param(&input, "topic")?;
        ctx.agent
            .with_project(|p| {
                p.memory.delete(topic)?;
                Ok(json!({ "status": "ok", "topic": topic }))
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn lsp() -> Arc<dyn crate::lsp::LspProvider> {
        crate::lsp::LspManager::new_arc()
    }

    async fn test_ctx_with_project() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        // Create .code-explorer dir so MemoryStore::open works
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: lsp(),
                output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(
                    20,
                )),
            },
        )
    }

    async fn test_ctx_no_project() -> ToolContext {
        ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
        }
    }

    #[tokio::test]
    async fn write_and_read_roundtrip() {
        let (_dir, ctx) = test_ctx_with_project().await;
        let result = WriteMemory
            .call(
                json!({
                    "topic": "test-topic",
                    "content": "hello memory"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(result["status"], "ok");

        let result = ReadMemory
            .call(json!({ "topic": "test-topic" }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["content"], "hello memory");
    }

    #[tokio::test]
    async fn read_missing_returns_null() {
        let (_dir, ctx) = test_ctx_with_project().await;
        let result = ReadMemory
            .call(json!({ "topic": "nonexistent" }), &ctx)
            .await
            .unwrap();
        assert!(result["content"].is_null());
    }

    #[tokio::test]
    async fn list_after_writes() {
        let (_dir, ctx) = test_ctx_with_project().await;
        WriteMemory
            .call(json!({ "topic": "b-topic", "content": "b" }), &ctx)
            .await
            .unwrap();
        WriteMemory
            .call(json!({ "topic": "a-topic", "content": "a" }), &ctx)
            .await
            .unwrap();

        let result = ListMemories.call(json!({}), &ctx).await.unwrap();
        let topics: Vec<&str> = result["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(topics, vec!["a-topic", "b-topic"]);
    }

    #[tokio::test]
    async fn delete_removes_entry() {
        let (_dir, ctx) = test_ctx_with_project().await;
        WriteMemory
            .call(json!({ "topic": "to-delete", "content": "bye" }), &ctx)
            .await
            .unwrap();
        DeleteMemory
            .call(json!({ "topic": "to-delete" }), &ctx)
            .await
            .unwrap();

        let result = ReadMemory
            .call(json!({ "topic": "to-delete" }), &ctx)
            .await
            .unwrap();
        assert!(result["content"].is_null());
    }

    #[tokio::test]
    async fn tools_error_without_active_project() {
        let ctx = test_ctx_no_project().await;
        assert!(WriteMemory
            .call(json!({ "topic": "x", "content": "y" }), &ctx)
            .await
            .is_err());
        assert!(ReadMemory
            .call(json!({ "topic": "x" }), &ctx)
            .await
            .is_err());
        assert!(ListMemories.call(json!({}), &ctx).await.is_err());
        assert!(DeleteMemory
            .call(json!({ "topic": "x" }), &ctx)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn nested_topic_works() {
        let (_dir, ctx) = test_ctx_with_project().await;
        WriteMemory
            .call(
                json!({
                    "topic": "debugging/async-patterns",
                    "content": "avoid blocking the runtime"
                }),
                &ctx,
            )
            .await
            .unwrap();

        let result = ReadMemory
            .call(json!({ "topic": "debugging/async-patterns" }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["content"], "avoid blocking the runtime");
    }
}
