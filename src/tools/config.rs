//! Configuration and project management tools.

use super::{Tool, ToolContext};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ActivateProject;
pub struct ProjectStatus;

#[async_trait::async_trait]
impl Tool for ActivateProject {
    fn name(&self) -> &str {
        "activate_project"
    }
    fn description(&self) -> &str {
        "Switch the active project to the given path. All subsequent tool calls \
         operate relative to this project root."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the project root" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let path = super::require_str_param(&input, "path")?;
        let root = PathBuf::from(path);
        if !root.is_dir() {
            return Err(super::RecoverableError::with_hint(
                format!("path '{}' is not a directory", path),
                "Provide an absolute path to an existing directory.",
            )
            .into());
        }
        // Capture before activate() — activate sets home_root on first call
        let had_home = ctx.agent.home_root().await.is_some();

        ctx.agent.activate(root).await?;

        let config = ctx
            .agent
            .with_project(|p| {
                Ok(json!({
                    "project_root": p.root.display().to_string(),
                    "config": serde_json::to_value(&p.config)?,
                }))
            })
            .await?;

        // Build CWD hint
        let project_root_str = config["project_root"].as_str().unwrap_or("?");
        let is_home = ctx.agent.is_home().await;
        let home = ctx.agent.home_root().await;

        let hint = if !had_home {
            format!("CWD: {}", project_root_str)
        } else if is_home {
            format!("Returned to original project. CWD: {}", project_root_str)
        } else {
            format!(
                "Switched project. CWD: {} (home: {})",
                project_root_str,
                home.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            )
        };

        Ok(json!({ "status": "ok", "activated": config, "hint": hint }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_activate_project(result))
    }
}

#[async_trait::async_trait]
impl Tool for ProjectStatus {
    fn name(&self) -> &str {
        "project_status"
    }

    fn description(&self) -> &str {
        "Active project state: languages, embedding model, index health summary, and memory staleness. \
         Call index_status() for detailed index info and live progress."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, _input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        use crate::agent::IndexingState;

        // --- Essential config + library section ---
        let (root, languages, embeddings_model, lib_count, lib_indexed) = ctx
            .agent
            .with_project(|p| {
                let lib_count = p.library_registry.all().len();
                let lib_indexed = p
                    .library_registry
                    .all()
                    .iter()
                    .filter(|e| e.indexed)
                    .count();
                Ok((
                    p.root.clone(),
                    p.config.project.languages.clone(),
                    p.config.embeddings.model.clone(),
                    lib_count,
                    lib_indexed,
                ))
            })
            .await?;

        let mut result = json!({
            "project_root": root.display().to_string(),
            "languages": languages,
            "embeddings_model": embeddings_model,
            "libraries": { "count": lib_count, "indexed": lib_indexed },
        });

        // --- Index section ---
        // Running state takes priority over DB stats.
        let indexing_state = ctx
            .agent
            .indexing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        if let IndexingState::Running {
            done,
            total,
            eta_secs,
        } = indexing_state
        {
            result["index"] = json!({
                "status": "running",
                "done": done,
                "total": total,
                "eta_secs": eta_secs,
                "hint": "Call index_status() for detailed breakdown.",
            });
        } else {
            let db_path = crate::embed::index::project_db_path(&root);
            if !db_path.exists() {
                result["index"] = json!({
                    "status": "not_indexed",
                    "hint": "Run index_project() to build the index.",
                });
            } else {
                let root2 = root.clone();
                let index_result = tokio::task::spawn_blocking(move || {
                    let conn = crate::embed::index::open_db(&root2)?;
                    let stats = crate::embed::index::index_stats(&conn)?;
                    let staleness = crate::embed::index::check_index_staleness(&conn, &root2).ok();
                    anyhow::Ok((stats, staleness))
                })
                .await;

                match index_result {
                    Ok(Ok((stats, staleness))) => {
                        let status = match staleness.as_ref() {
                            Some(s) if s.stale => "behind",
                            _ => "up_to_date",
                        };
                        result["index"] = json!({
                            "status": status,
                            "files": stats.file_count,
                            "chunks": stats.chunk_count,
                            "last_updated": stats.indexed_at,
                            "hint": "Call index_status() for model info, by_source breakdown, drift, and progress details.",
                        });
                    }
                    _ => {
                        result["index"] = json!({
                            "status": "not_indexed",
                            "hint": "Run index_project() to build the index.",
                        });
                    }
                }
            }
        }

        // --- Memory staleness section ---
        let staleness_result = ctx
            .agent
            .with_project(|p| {
                let memories_dir = p.root.join(".codescout").join("memories");
                crate::memory::anchors::check_all_memories(&p.root, &memories_dir)
            })
            .await;
        match staleness_result {
            Ok(staleness) => {
                result["memory_staleness"] = staleness;
            }
            Err(e) => {
                tracing::debug!("memory staleness check failed: {e}");
            }
        }

        // --- Workspace section ---
        let workspace_toml_path = root.join(".codescout").join("workspace.toml");
        let workspace_info = if workspace_toml_path.exists() {
            std::fs::read_to_string(&workspace_toml_path)
                .ok()
                .and_then(|s| toml::from_str::<crate::config::workspace::WorkspaceConfig>(&s).ok())
                .map(|ws| {
                    json!({
                        "name": ws.workspace.name,
                        "projects": ws.projects.iter().map(|p| json!({
                            "id": p.id,
                            "root": p.root,
                            "languages": p.languages,
                            "depends_on": p.depends_on,
                        })).collect::<Vec<_>>(),
                        "resources": {
                            "max_lsp_clients": ws.resources.max_lsp_clients,
                            "idle_timeout_secs": ws.resources.idle_timeout_secs,
                        },
                    })
                })
        } else {
            None
        };
        result["workspace"] = json!(workspace_info);

        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_project_status(result))
    }
}

fn format_activate_project(result: &Value) -> String {
    let root = result["activated"]["project_root"]
        .as_str()
        .or_else(|| result["path"].as_str())
        .unwrap_or("?");
    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(root);
    if let Some(hint) = result["hint"].as_str() {
        format!("activated · {name} · {hint}")
    } else {
        format!("activated · {name}")
    }
}

fn format_project_status(result: &Value) -> String {
    let root = result["project_root"].as_str().unwrap_or("?");
    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(root);
    let status = result["index"]["status"].as_str().unwrap_or("unknown");
    let index_str = match status {
        "up_to_date" | "behind" => {
            let files = result["index"]["files"].as_u64().unwrap_or(0);
            let chunks = result["index"]["chunks"].as_u64().unwrap_or(0);
            format!("index:{files}f/{chunks}c ({status})")
        }
        "running" => {
            let done = result["index"]["done"].as_u64().unwrap_or(0);
            let total = result["index"]["total"].as_u64().unwrap_or(0);
            format!("index:running {done}/{total}")
        }
        _ => "index:none".to_string(),
    };
    format!("status · {name} · {index_str}")
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

    #[tokio::test]
    async fn activate_and_get_config() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };

        // No project initially
        assert!(ProjectStatus.call(json!({}), &ctx).await.is_err());

        // Activate
        let result = ActivateProject
            .call(
                json!({
                    "path": dir.path().to_str().unwrap()
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(result["status"], "ok");

        // Now project_status works
        let status = ProjectStatus.call(json!({}), &ctx).await.unwrap();
        assert!(status["project_root"].as_str().unwrap().len() > 0);
        assert!(status["languages"].is_array());
        assert!(status["embeddings_model"].is_string());
    }

    #[tokio::test]
    async fn activate_nonexistent_path_errors() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        let result = ActivateProject
            .call(
                json!({
                    "path": "/nonexistent/path/xyz"
                }),
                &ctx,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn activate_replaces_previous_project() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".codescout")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".codescout")).unwrap();

        let ctx = ToolContext {
            agent: Agent::new(Some(dir1.path().to_path_buf())).await.unwrap(),
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };

        // Activate dir2
        ActivateProject
            .call(
                json!({
                    "path": dir2.path().to_str().unwrap()
                }),
                &ctx,
            )
            .await
            .unwrap();

        let status = ProjectStatus.call(json!({}), &ctx).await.unwrap();
        let root = status["project_root"].as_str().unwrap();
        assert!(root.contains(dir2.path().file_name().unwrap().to_str().unwrap()));
    }

    #[tokio::test]
    async fn project_status_returns_all_sections() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        let tool = ProjectStatus;
        let result = tool.call(json!({}), &ctx).await.unwrap();
        assert!(result["project_root"].is_string(), "missing project_root");
        assert!(result["languages"].is_array(), "missing languages field");
        assert!(
            result["embeddings_model"].is_string(),
            "missing embeddings_model field"
        );
        assert!(result.get("index").is_some(), "missing index section");
        assert!(
            result.get("libraries").is_some(),
            "missing libraries section"
        );
    }

    #[tokio::test]
    async fn project_status_compact_shape() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        let result = ProjectStatus.call(json!({}), &ctx).await.unwrap();

        // Flat config fields — no blob
        assert!(result["languages"].is_array(), "missing languages");
        assert!(
            result["embeddings_model"].is_string(),
            "missing embeddings_model"
        );
        assert!(
            result.get("config").is_none(),
            "config blob must be removed"
        );

        // Index section has status field, no drift
        assert!(
            result["index"]["status"].is_string(),
            "index.status must be present"
        );
        assert!(
            result["index"].get("drift").is_none(),
            "drift must not appear in project_status"
        );

        // Libraries section still present
        assert!(result["libraries"].is_object(), "libraries section missing");
    }

    #[tokio::test]
    async fn project_status_includes_memory_staleness() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };

        // Create memories dir and a memory file
        let memories_dir = dir.path().join(".codescout/memories");
        std::fs::create_dir_all(&memories_dir).unwrap();
        std::fs::write(memories_dir.join("architecture.md"), "# Arch").unwrap();

        // Create anchored file and sidecar
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/server.rs"), "fn main() {}").unwrap();

        let anchors =
            crate::memory::anchors::seed_anchors(dir.path(), "Uses `src/server.rs`.").unwrap();
        crate::memory::anchors::write_anchor_file(
            &memories_dir.join("architecture.anchors.toml"),
            &anchors,
        )
        .unwrap();

        // Before change — should be fresh
        let result = ProjectStatus.call(json!({}), &ctx).await.unwrap();
        let staleness = &result["memory_staleness"];
        assert!(staleness["stale"].as_array().unwrap().is_empty());
        assert!(staleness["fresh"]
            .as_array()
            .unwrap()
            .contains(&json!("architecture")));

        // Modify the anchored file
        std::fs::write(dir.path().join("src/server.rs"), "fn changed() {}").unwrap();

        let result = ProjectStatus.call(json!({}), &ctx).await.unwrap();
        let staleness = &result["memory_staleness"];
        let stale = staleness["stale"].as_array().unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0]["topic"], "architecture");
        assert!(stale[0]["changed_files"]
            .as_array()
            .unwrap()
            .contains(&json!("src/server.rs")));
    }

    #[tokio::test]
    async fn activate_includes_cwd_hint() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(None).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        let input = json!({ "path": dir.path().to_str().unwrap() });
        let result = ActivateProject.call(input, &ctx).await.unwrap();
        let hint = result["hint"].as_str().unwrap();
        assert!(
            hint.starts_with("CWD: "),
            "hint should start with CWD: but was: {hint}"
        );
        assert!(hint.contains(dir.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn activate_hint_shows_switched_when_away_from_home() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".codescout")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir1.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        let input = json!({ "path": dir2.path().to_str().unwrap() });
        let result = ActivateProject.call(input, &ctx).await.unwrap();
        let hint = result["hint"].as_str().unwrap();
        assert!(hint.starts_with("Switched project."), "hint: {hint}");
        assert!(
            hint.contains(dir2.path().to_str().unwrap()),
            "should contain new path"
        );
        assert!(
            hint.contains(dir1.path().to_str().unwrap()),
            "should contain home path"
        );
    }

    #[tokio::test]
    async fn activate_hint_shows_returned_when_back_home() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".codescout")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir1.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };
        // Switch away
        ActivateProject
            .call(json!({ "path": dir2.path().to_str().unwrap() }), &ctx)
            .await
            .unwrap();
        // Return home
        let result = ActivateProject
            .call(json!({ "path": dir1.path().to_str().unwrap() }), &ctx)
            .await
            .unwrap();
        let hint = result["hint"].as_str().unwrap();
        assert!(
            hint.starts_with("Returned to original project."),
            "hint: {hint}"
        );
        assert!(hint.contains(dir1.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn project_status_shows_workspace_projects() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Create multi-project structure
        std::fs::write(root.join("build.gradle.kts"), "").unwrap();
        let mcp = root.join("mcp-server");
        std::fs::create_dir_all(&mcp).unwrap();
        std::fs::write(mcp.join("package.json"), r#"{"scripts":{"build":"tsc"}}"#).unwrap();

        // Create workspace.toml
        let codescout = root.join(".codescout");
        std::fs::create_dir_all(&codescout).unwrap();
        std::fs::write(
            codescout.join("workspace.toml"),
            r#"
[workspace]
name = "test"

[[project]]
id = "test"
root = "."
languages = ["kotlin"]

[[project]]
id = "mcp-server"
root = "mcp-server"
languages = ["typescript"]
depends_on = ["test"]
"#,
        )
        .unwrap();
        std::fs::write(
            codescout.join("project.toml"),
            "[project]\nname = \"test\"\nlanguages = [\"kotlin\"]\n",
        )
        .unwrap();

        let agent = Agent::new(Some(root.to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: lsp(),
            output_buffer: Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
        };

        let result = ProjectStatus
            .call(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let ws = result.get("workspace");
        assert!(
            ws.is_some(),
            "project_status should include workspace section"
        );
        let projects = ws.unwrap().get("projects").unwrap().as_array().unwrap();
        assert_eq!(projects.len(), 2);
    }
}
