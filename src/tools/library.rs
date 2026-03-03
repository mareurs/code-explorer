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
        "List registered libraries and their index status. \
         Use scope='lib:<name>' in semantic_search, find_symbol, or index_project to target a library."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _input: Value, ctx: &ToolContext) -> Result<Value> {
        let inner = ctx.agent.inner.read().await;
        let project = inner.active_project.as_ref().ok_or_else(|| {
            super::RecoverableError::with_hint(
                "No active project. Use activate_project first.",
                "Call activate_project(\"/path/to/project\") to set the active project.",
            )
        })?;

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

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_list_libraries(result))
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
        let name = super::require_str_param(&input, "name")?;
        let force = input["force"].as_bool().unwrap_or(false);

        let (root, lib_path) = {
            let inner = ctx.agent.inner.read().await;
            let project = inner.active_project.as_ref().ok_or_else(|| {
                super::RecoverableError::with_hint(
                    "No active project. Use activate_project first.",
                    "Call activate_project(\"/path/to/project\") to set the active project.",
                )
            })?;
            let entry = project.library_registry.lookup(name).ok_or_else(|| {
                super::RecoverableError::with_hint(
                    format!("Library '{}' not found in registry.", name),
                    "Use list_libraries to see registered libraries.",
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

        // Return stats — sync SQLite off async runtime
        let source2 = source.clone();
        let (file_count, chunk_count) = tokio::task::spawn_blocking(move || {
            let conn = crate::embed::index::open_db(&root)?;
            let by_source = crate::embed::index::index_stats_by_source(&conn)?;
            let lib_stats = by_source.get(&source2);
            anyhow::Ok((
                lib_stats.map_or(0, |s| s.file_count),
                lib_stats.map_or(0, |s| s.chunk_count),
            ))
        })
        .await??;

        Ok(json!({
            "status": "ok",
            "library": name,
            "source": source,
            "files_indexed": file_count,
            "chunks": chunk_count,
        }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_index_library(result))
    }
}

fn format_list_libraries(result: &Value) -> String {
    let libs = match result["libraries"].as_array() {
        Some(l) if !l.is_empty() => l,
        _ => return "0 libraries".to_string(),
    };
    let name_width = libs
        .iter()
        .filter_map(|l| l["name"].as_str())
        .map(|n| n.len())
        .max()
        .unwrap_or(0);
    let mut out = format!("{} libraries", libs.len());
    for lib in libs.iter() {
        let name = lib["name"].as_str().unwrap_or("?");
        let status = if lib["indexed"].as_bool().unwrap_or(false) {
            "indexed"
        } else {
            "not indexed"
        };
        out.push_str(&format!("\n  {name:<name_width$}  {status}"));
    }
    out
}

fn format_index_library(result: &Value) -> String {
    let name = result["library"].as_str().unwrap_or("?");
    let chunks = result["chunks"].as_u64().unwrap_or(0);
    format!("{name} · {chunks} chunks")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::LspManager;
    use std::path::PathBuf;

    async fn project_ctx() -> ToolContext {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(root)).await.unwrap();
        // Leak the tempdir so it stays alive
        std::mem::forget(dir);
        ToolContext {
            agent,
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
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
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
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

    // --- format_list_libraries tests ---

    #[test]
    fn format_list_libraries_shows_names_and_status() {
        let result = serde_json::json!({
            "libraries": [
                {"name": "serde", "indexed": true},
                {"name": "tokio", "indexed": false}
            ]
        });
        let out = format_list_libraries(&result);
        assert!(
            out.contains("serde"),
            "should show library name, got: {out}"
        );
        assert!(
            out.contains("tokio"),
            "should show library name, got: {out}"
        );
        assert!(
            out.contains("indexed"),
            "should show index status, got: {out}"
        );
    }

    #[tokio::test]
    async fn index_project_scope_lib_errors_for_unknown() {
        let ctx = project_ctx().await;
        // Register nothing — querying an unknown lib name should return RecoverableError
        let tool = crate::tools::semantic::IndexProject;
        let result = tool.call(json!({ "scope": "lib:nonexistent" }), &ctx).await;
        assert!(result.is_err(), "expected error for unknown library");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nonexistent") || msg.contains("not found"),
            "error should mention the library name: {msg}"
        );
    }
}
