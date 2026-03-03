//! Memory tools: persistent per-project knowledge store.

use super::{RecoverableError, Tool, ToolContext};
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
                Ok(json!("ok"))
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
                    Some(content) => Ok(json!({ "content": content })),
                    None => Err(RecoverableError::with_hint(
                        format!("topic '{}' not found", topic),
                        "Use list_memories to see available topics",
                    )
                    .into()),
                }
            })
            .await
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_read_memory(result))
    }
}

#[async_trait::async_trait]
impl Tool for ListMemories {
    fn name(&self) -> &str {
        "list_memories"
    }
    fn description(&self) -> &str {
        "List all stored memory topics for the active project. \
         Pass include_private: true to also see private topics."
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
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let include_private = input["include_private"].as_bool().unwrap_or(false);
        ctx.agent
            .with_project(|p| {
                if include_private {
                    let shared = p.memory.list()?;
                    let private = p.private_memory.list()?;
                    Ok(json!({ "shared": shared, "private": private }))
                } else {
                    let topics = p.memory.list()?;
                    Ok(json!({ "topics": topics }))
                }
            })
            .await
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_list_memories(result))
    }
}

fn format_read_memory(result: &Value) -> String {
    result["content"].as_str().unwrap_or("").to_string()
}

fn format_list_memories(result: &Value) -> String {
    // include_private=true path: { shared: [...], private: [...] }
    if let (Some(shared), Some(private)) =
        (result["shared"].as_array(), result["private"].as_array())
    {
        let mut out = format!("{} shared, {} private", shared.len(), private.len());
        for t in shared {
            if let Some(name) = t.as_str() {
                out.push_str(&format!("\n  {name}"));
            }
        }
        if !private.is_empty() {
            out.push_str("\n  -- private --");
            for t in private {
                if let Some(name) = t.as_str() {
                    out.push_str(&format!("\n  {name}"));
                }
            }
        }
        return out;
    }
    // Default path: { topics: [...] }
    let topics = match result["topics"].as_array() {
        Some(t) if !t.is_empty() => t,
        _ => return "0 topics".to_string(),
    };
    let mut out = format!("{} topics", topics.len());
    for topic in topics.iter() {
        if let Some(name) = topic.as_str() {
            out.push_str(&format!("\n  {name}"));
        }
    }
    out
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
                Ok(json!("ok"))
            })
            .await
    }
}

pub struct Memory;

#[async_trait::async_trait]
impl Tool for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Persistent project memory — action: \"read\", \"write\", \"list\", \"delete\". \
         topic is a path-like key (e.g. 'debugging/async-patterns'). \
         Pass private=true to use the gitignored private store."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "list", "delete"],
                    "description": "Operation to perform"
                },
                "topic": {
                    "type": "string",
                    "description": "Required for read/write/delete. Path-like key, e.g. 'debugging/async-patterns'."
                },
                "content": {
                    "type": "string",
                    "description": "Required for write. The content to persist."
                },
                "private": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, use the gitignored private store."
                },
                "include_private": {
                    "type": "boolean",
                    "default": false,
                    "description": "For list: also return private topics. Returns { shared, private } instead of { topics }."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let action = super::require_str_param(&input, "action")?;
        match action {
            "write" => {
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
                        Ok(json!("ok"))
                    })
                    .await
            }
            "read" => {
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
                            Some(content) => Ok(json!({ "content": content })),
                            None => Err(RecoverableError::with_hint(
                                format!("topic '{}' not found", topic),
                                "Use memory(action='list') to see available topics",
                            )
                            .into()),
                        }
                    })
                    .await
            }
            "list" => {
                let include_private = input["include_private"].as_bool().unwrap_or(false);
                ctx.agent
                    .with_project(|p| {
                        if include_private {
                            Ok(json!({ "shared": p.memory.list()?, "private": p.private_memory.list()? }))
                        } else {
                            Ok(json!({ "topics": p.memory.list()? }))
                        }
                    })
                    .await
            }
            "delete" => {
                let topic = super::require_str_param(&input, "topic")?;
                let private = input["private"].as_bool().unwrap_or(false);
                ctx.agent
                    .with_project(|p| {
                        if private {
                            p.private_memory.delete(topic)?;
                        } else {
                            p.memory.delete(topic)?;
                        }
                        Ok(json!("ok"))
                    })
                    .await
            }
            _ => Err(RecoverableError::with_hint(
                format!(
                    "unknown action '{}'. Must be one of: read, write, list, delete",
                    action
                ),
                "Pass action: 'read', 'write', 'list', or 'delete'",
            )
            .into()),
        }
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        if result["topics"].is_array() || result["shared"].is_array() {
            Some(format_list_memories(result))
        } else if result["content"].is_string() {
            Some(format_read_memory(result))
        } else {
            None
        }
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
        assert_eq!(result, "ok");

        let result = ReadMemory
            .call(json!({ "topic": "test-topic" }), &ctx)
            .await
            .unwrap();
        assert_eq!(result["content"], "hello memory");
    }

    #[tokio::test]
    async fn read_missing_returns_null() {
        let (_dir, ctx) = test_ctx_with_project().await;
        let err = ReadMemory
            .call(json!({ "topic": "nonexistent" }), &ctx)
            .await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"), "got: {msg}");
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

        let err = ReadMemory.call(json!({ "topic": "to-delete" }), &ctx).await;
        assert!(err.is_err());
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
    fn list_memories_format_compact() {
        use serde_json::json;
        let tool = ListMemories;
        let r = json!({ "topics": ["a", "b", "c"] });
        let t = tool.format_compact(&r).unwrap();
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
        // private store doesn't have the topic → should error, not return shared data
        let err = ReadMemory
            .call(json!({"topic": "shared-topic", "private": true}), &ctx)
            .await;
        assert!(err.is_err());
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

    #[tokio::test]
    async fn list_memories_default_returns_topics_key() {
        let (_dir, ctx) = test_ctx_with_project().await;
        ctx.agent
            .with_project(|p| p.memory.write("arch", "..."))
            .await
            .unwrap();
        let result = ListMemories.call(json!({}), &ctx).await.unwrap();
        assert!(result["topics"].is_array());
        assert!(result["shared"].is_null()); // old shape preserved by default
    }

    #[tokio::test]
    async fn list_memories_include_private_returns_shared_and_private_keys() {
        let (_dir, ctx) = test_ctx_with_project().await;
        ctx.agent
            .with_project(|p| {
                p.memory.write("arch", "...")?;
                p.private_memory.write("prefs", "...")?;
                Ok(())
            })
            .await
            .unwrap();
        let result = ListMemories
            .call(json!({"include_private": true}), &ctx)
            .await
            .unwrap();
        assert!(result["shared"].is_array());
        assert!(result["private"].is_array());
        assert!(result["topics"].is_null()); // new shape, no "topics" key
        let shared: Vec<_> = result["shared"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(shared.contains(&"arch"));
        let private: Vec<_> = result["private"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(private.contains(&"prefs"));
    }

    #[tokio::test]
    async fn list_memories_include_private_empty_private_store() {
        let (_dir, ctx) = test_ctx_with_project().await;
        ctx.agent
            .with_project(|p| p.memory.write("arch", "..."))
            .await
            .unwrap();
        let result = ListMemories
            .call(json!({"include_private": true}), &ctx)
            .await
            .unwrap();
        let private = result["private"].as_array().unwrap();
        assert!(private.is_empty());
    }

    // --- format_list_memories / format_read_memory tests ---

    #[test]
    fn format_list_memories_shows_topic_names() {
        let result = serde_json::json!({
            "topics": ["architecture", "conventions", "gotchas"]
        });
        let out = format_list_memories(&result);
        assert!(out.contains("architecture"), "should list topic names");
        assert!(out.contains("conventions"), "should list topic names");
        assert!(out.contains("gotchas"), "should list topic names");
        assert!(out.contains('3'), "should include count");
    }

    #[test]
    fn format_list_memories_empty() {
        let result = serde_json::json!({ "topics": [] });
        let out = format_list_memories(&result);
        assert!(out.contains('0'), "should say 0 topics");
    }

    #[test]
    fn format_list_memories_include_private_shows_both() {
        let result = serde_json::json!({ "shared": ["arch", "conventions"], "private": ["prefs"] });
        let out = format_list_memories(&result);
        assert!(out.contains("2 shared"));
        assert!(out.contains("1 private"));
        assert!(out.contains("arch"));
        assert!(out.contains("prefs"));
    }

    #[test]
    fn format_list_memories_include_private_empty_private() {
        let result = serde_json::json!({ "shared": ["arch"], "private": [] });
        let out = format_list_memories(&result);
        assert!(out.contains("1 shared"));
        assert!(out.contains("0 private"));
    }

    #[test]
    fn format_read_memory_shows_content() {
        let result = serde_json::json!({
            "content": "## Layers\n\nAgent → Server → Tools"
        });
        let out = format_read_memory(&result);
        assert!(out.contains("Layers"), "should show content");
        assert!(
            out.contains("Agent → Server → Tools"),
            "should show full content"
        );
    }

    #[tokio::test]
    async fn memory_write_and_read_via_dispatch() {
        let (dir, ctx) = test_ctx_with_project().await;
        let tool = Memory;

        // write
        let w = tool
            .call(
                json!({ "action": "write", "topic": "test/key", "content": "hello" }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(w, json!("ok"));

        // read
        let r = tool
            .call(json!({ "action": "read", "topic": "test/key" }), &ctx)
            .await
            .unwrap();
        assert_eq!(r["content"], json!("hello"));

        drop(dir);
    }

    #[tokio::test]
    async fn memory_list_via_dispatch() {
        let (dir, ctx) = test_ctx_with_project().await;
        let tool = Memory;
        tool.call(
            json!({ "action": "write", "topic": "a", "content": "x" }),
            &ctx,
        )
        .await
        .unwrap();
        let result = tool.call(json!({ "action": "list" }), &ctx).await.unwrap();
        let topics = result["topics"].as_array().expect("expected topics array");
        assert!(topics.iter().any(|t| t.as_str() == Some("a")));
        drop(dir);
    }

    #[tokio::test]
    async fn memory_delete_via_dispatch() {
        let (dir, ctx) = test_ctx_with_project().await;
        let tool = Memory;
        tool.call(
            json!({ "action": "write", "topic": "to_delete", "content": "x" }),
            &ctx,
        )
        .await
        .unwrap();
        tool.call(json!({ "action": "delete", "topic": "to_delete" }), &ctx)
            .await
            .unwrap();
        let result = tool
            .call(json!({ "action": "read", "topic": "to_delete" }), &ctx)
            .await;
        assert!(result.is_err(), "expected error reading deleted topic");
        drop(dir);
    }

    #[tokio::test]
    async fn memory_unknown_action_returns_recoverable_error() {
        let (dir, ctx) = test_ctx_with_project().await;
        let tool = Memory;
        let result = tool.call(json!({ "action": "explode" }), &ctx).await;
        assert!(result.is_err());
        drop(dir);
    }
}
