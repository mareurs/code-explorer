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

pub struct IndexLibrary;

#[async_trait::async_trait]
impl Tool for IndexLibrary {
    fn name(&self) -> &str {
        "index_library"
    }

    fn description(&self) -> &str {
        "Build embedding index for a registered library. \
         Library must be in the registry (discovered via goto_definition or manually registered)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Library name (as shown in list_libraries)"
                },
                "force": {
                    "type": "boolean",
                    "description": "Re-index even if already done",
                    "default": false
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name' parameter"))?;
        let force = input["force"].as_bool().unwrap_or(false);

        let (root, lib_path) = {
            let inner = ctx.agent.inner.read().await;
            let project = inner
                .active_project
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No active project. Use activate_project first."))?;
            let entry = project
                .library_registry
                .lookup(name)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Library '{}' not found in registry. Use list_libraries to see registered libraries.",
                        name
                    )
                })?;
            (project.root.clone(), entry.path.clone())
        };

        let source = format!("lib:{}", name);
        crate::embed::index::build_library_index(&root, &lib_path, &source, force).await?;

        // Mark as indexed in registry and save
        {
            let mut inner = ctx.agent.inner.write().await;
            let project = inner.active_project.as_mut().unwrap();
            if let Some(entry) = project.library_registry.lookup_mut(name) {
                entry.indexed = true;
            }
            let registry_path = project.root.join(".code-explorer").join("libraries.json");
            project.library_registry.save(&registry_path)?;
        }

        // Return stats
        let conn = crate::embed::index::open_db(&root)?;
        let by_source = crate::embed::index::index_stats_by_source(&conn)?;
        let lib_stats = by_source.get(&source);

        Ok(json!({
            "status": "ok",
            "library": name,
            "source": source,
            "files_indexed": lib_stats.map_or(0, |s| s.file_count),
            "chunks": lib_stats.map_or(0, |s| s.chunk_count),
        }))
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

    #[tokio::test]
    async fn index_library_errors_for_unknown() {
        let ctx = project_ctx().await;
        let tool = IndexLibrary;
        let result = tool.call(json!({ "name": "nonexistent" }), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention not found: {}",
            err
        );
    }

    #[test]
    fn index_library_schema_is_valid() {
        let tool = IndexLibrary;
        let schema = tool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("name"),
            "schema must have 'name' property"
        );
        assert!(
            props.contains_key("force"),
            "schema must have 'force' property"
        );
        let required = schema["required"].as_array().unwrap();
        assert!(
            required.iter().any(|v| v.as_str() == Some("name")),
            "'name' must be required"
        );
    }
}
