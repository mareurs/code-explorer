use anyhow::Result;
use serde_json::{json, Value};

use super::{Tool, ToolContext};

pub struct ListLibraries;

#[async_trait::async_trait]
impl Tool for ListLibraries {
    fn name(&self) -> &str {
        "list_libraries"
    }

    fn description(&self) -> &str {
        "Show all registered libraries and their status (indexed, path, language)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _input: Value, ctx: &ToolContext) -> Result<Value> {
        let inner = ctx.agent.inner.read().await;
        let project = inner
            .active_project
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active project. Use activate_project first."))?;

        let libs: Vec<Value> = project
            .library_registry
            .all()
            .iter()
            .map(|entry| {
                json!({
                    "name": entry.name,
                    "version": entry.version,
                    "path": entry.path.display().to_string(),
                    "language": entry.language,
                    "discovered_via": entry.discovered_via,
                    "indexed": entry.indexed,
                })
            })
            .collect();

        Ok(json!({ "libraries": libs }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use std::path::PathBuf;
    use std::sync::Arc;

    async fn project_ctx() -> ToolContext {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(root)).await.unwrap();
        // Leak the tempdir so it stays alive
        std::mem::forget(dir);
        ToolContext {
            agent,
            lsp: Arc::new(LspManager::new()),
        }
    }

    #[tokio::test]
    async fn list_libraries_empty() {
        let ctx = project_ctx().await;
        let tool = ListLibraries;
        let result = tool.call(json!({}), &ctx).await.unwrap();
        let libs = result["libraries"].as_array().unwrap();
        assert!(libs.is_empty());
    }

    #[tokio::test]
    async fn list_libraries_shows_registered() {
        let ctx = project_ctx().await;
        {
            let mut inner = ctx.agent.inner.write().await;
            let project = inner.active_project.as_mut().unwrap();
            project.library_registry.register(
                "serde".into(),
                PathBuf::from("/tmp/serde"),
                "rust".into(),
                crate::library::registry::DiscoveryMethod::Manual,
            );
        }
        let tool = ListLibraries;
        let result = tool.call(json!({}), &ctx).await.unwrap();
        let libs = result["libraries"].as_array().unwrap();
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0]["name"], "serde");
        assert_eq!(libs[0]["indexed"], false);
    }

    #[tokio::test]
    async fn list_libraries_errors_without_project() {
        let agent = Agent::new(None).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: Arc::new(LspManager::new()),
        };
        let tool = ListLibraries;
        let result = tool.call(json!({}), &ctx).await;
        assert!(result.is_err());
    }
}
