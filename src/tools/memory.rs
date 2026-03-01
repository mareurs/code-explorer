//! Memory tools: persistent per-project knowledge store.

use super::{user_format, Tool, ToolContext};
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
                "content": { "type": "string" },
                "private": {
                    "type": "boolean",
                    "description": "If true, write to the gitignored private store (personal/machine-specific notes, not shared with teammates)."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let topic = super::require_str_param(&input, "topic")?;
        let content = super::require_str_param(&input, "content")?;
        let private = input["private"].as_bool().unwrap_or(false);
        ctx.agent
            .with_project(|p| {
                if private {
                    p.private_memory.write(topic, content)?;
                } else {
                    p.memory.write(topic, content)?;
                }
                Ok(json!({ "status": "ok", "topic": topic }))
            })
            .await
    }

    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_write_memory(result))
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
            "properties": {
                "topic": { "type": "string" },
                "private": {
                    "type": "boolean",
                    "description": "If true, read from the private memory store."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let topic = super::require_str_param(&input, "topic")?;
        let private = input["private"].as_bool().unwrap_or(false);
        ctx.agent
            .with_project(|p| {
                let store = if private {
                    &p.private_memory
                } else {
                    &p.memory
                };
                match store.read(topic)? {
                    Some(content) => Ok(json!({ "topic": topic, "content": content })),
                    None => Ok(json!({ "topic": topic, "content": null, "message": "not found" })),
                }
            })
            .await
    }

    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_read_memory(result))
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
        json!({
            "type": "object",
            "properties": {
                "include_private": {
                    "type": "boolean",
                    "description": "If true, also list private memory topics. Returns { shared, private } instead of { topics }."
                }
            }
        })
    }
    async fn call(&self, _input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        ctx.agent
            .with_project(|p| {
                let topics = p.memory.list()?;
                Ok(json!({ "topics": topics }))
            })
            .await
    }

    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_list_memories(result))
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
            "properties": {
                "topic": { "type": "string" },
                "private": {
                    "type": "boolean",
                    "description": "If true, delete from the private memory store."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let topic = super::require_str_param(&input, "topic")?;
        let private = input["private"].as_bool().unwrap_or(false);
        ctx.agent
            .with_project(|p| {
                if private {
                    p.private_memory.delete(topic)?;
                } else {
                    p.memory.delete(topic)?;
                }
                Ok(json!({ "status": "ok", "topic": topic }))
            })
            .await
    }

    fn format_for_user(&self, result: &Value) -> Option<String> {
        Some(user_format::format_delete_memory(result))
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
                progress: None,
            },
        )
    }

    async fn test_ctx_no_project() -> ToolContext {
        ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
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

    #[test]
    fn write_memory_format_for_user() {
        use serde_json::json;
        let tool = WriteMemory;
        let r = json!({ "status": "ok", "topic": "arch" });
        let t = tool.format_for_user(&r).unwrap();
        assert!(t.contains("arch"), "got: {t}");
    }

    #[test]
    fn list_memories_format_for_user() {
        use serde_json::json;
        let tool = ListMemories;
        let r = json!({ "topics": ["a", "b", "c"] });
        let t = tool.format_for_user(&r).unwrap();
        assert!(t.contains("3"), "got: {t}");
    }

    #[test]
    fn write_memory_schema_has_private_field() {
        let schema = WriteMemory.input_schema();
        assert!(schema["properties"]["private"].is_object());
        assert_eq!(schema["properties"]["private"]["type"], "boolean");
    }

    #[test]
    fn read_memory_schema_has_private_field() {
        let schema = ReadMemory.input_schema();
        assert!(schema["properties"]["private"].is_object());
        assert_eq!(schema["properties"]["private"]["type"], "boolean");
    }

    #[test]
    fn delete_memory_schema_has_private_field() {
        let schema = DeleteMemory.input_schema();
        assert!(schema["properties"]["private"].is_object());
        assert_eq!(schema["properties"]["private"]["type"], "boolean");
    }

    #[test]
    fn list_memories_schema_has_include_private_field() {
        let schema = ListMemories.input_schema();
        assert!(schema["properties"]["include_private"].is_object());
        assert_eq!(schema["properties"]["include_private"]["type"], "boolean");
    }

    #[tokio::test]
    async fn write_private_goes_to_private_store() {
        let (_dir, ctx) = test_ctx_with_project().await;
        WriteMemory
            .call(
                json!({"topic": "prefs", "content": "verbose", "private": true}),
                &ctx,
            )
            .await
            .unwrap();
        // not in shared store
        let shared = ctx
            .agent
            .with_project(|p| p.memory.read("prefs"))
            .await
            .unwrap();
        assert_eq!(shared, None);
        // is in private store
        let private = ctx
            .agent
            .with_project(|p| p.private_memory.read("prefs"))
            .await
            .unwrap();
        assert_eq!(private, Some("verbose".to_string()));
    }

    #[tokio::test]
    async fn read_private_reads_from_private_store() {
        let (_dir, ctx) = test_ctx_with_project().await;
        ctx.agent
            .with_project(|p| p.private_memory.write("wip", "issue-42"))
            .await
            .unwrap();
        let result = ReadMemory
            .call(json!({"topic": "wip", "private": true}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["content"], "issue-42");
    }

    #[tokio::test]
    async fn read_private_does_not_see_shared() {
        let (_dir, ctx) = test_ctx_with_project().await;
        ctx.agent
            .with_project(|p| p.memory.write("shared-topic", "data"))
            .await
            .unwrap();
        let result = ReadMemory
            .call(json!({"topic": "shared-topic", "private": true}), &ctx)
            .await
            .unwrap();
        assert!(result["content"].is_null());
    }

    #[tokio::test]
    async fn delete_private_removes_from_private_store() {
        let (_dir, ctx) = test_ctx_with_project().await;
        ctx.agent
            .with_project(|p| p.private_memory.write("tmp", "gone"))
            .await
            .unwrap();
        DeleteMemory
            .call(json!({"topic": "tmp", "private": true}), &ctx)
            .await
            .unwrap();
        let result = ctx
            .agent
            .with_project(|p| p.private_memory.read("tmp"))
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn delete_private_does_not_affect_shared_store() {
        let (_dir, ctx) = test_ctx_with_project().await;
        ctx.agent
            .with_project(|p| p.memory.write("tmp", "keep"))
            .await
            .unwrap();
        DeleteMemory
            .call(json!({"topic": "tmp", "private": true}), &ctx)
            .await
            .unwrap();
        let result = ctx
            .agent
            .with_project(|p| p.memory.read("tmp"))
            .await
            .unwrap();
        assert_eq!(result, Some("keep".to_string()));
    }
}
