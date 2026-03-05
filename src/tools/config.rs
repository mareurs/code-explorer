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
        Ok(json!({ "status": "ok", "activated": config }))
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
        "Active project state: config, semantic index health, usage telemetry, and library summary. \
         Pass threshold (float) to include drift scores. Pass window ('1h','24h','7d','30d') for usage window."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "threshold": {
                    "type": "number",
                    "description": "Min avg_drift to include (0.0-1.0). When provided, adds drift data to index section."
                },
                "path": {
                    "type": "string",
                    "description": "Glob pattern to filter drift files (SQL LIKE syntax, e.g. 'src/tools/%')."
                },
                "detail_level": {
                    "type": "string",
                    "enum": ["exploring", "full"],
                    "description": "Drift output detail. 'full' includes most-drifted chunk content."
                },
                "window": {
                    "type": "string",
                    "enum": ["1h", "24h", "7d", "30d"],
                    "description": "Time window for usage stats. Default: 30d."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        // --- Config + library section ---
        let (root, config_val, lib_count, lib_indexed) = ctx
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
                    serde_json::to_value(&p.config)?,
                    lib_count,
                    lib_indexed,
                ))
            })
            .await?;

        let mut result = json!({
            "project_root": root.display().to_string(),
            "config": config_val,
            "libraries": { "count": lib_count, "indexed": lib_indexed },
        });

        // --- Index section ---
        let db_path = crate::embed::index::db_path(&root);
        if !db_path.exists() {
            result["index"] = json!({ "indexed": false });
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
                    let mut index_section = json!({
                        "indexed": true,
                        "files": stats.file_count,
                        "chunks": stats.chunk_count,
                        "last_updated": stats.indexed_at,
                        "model": stats.model,
                    });
                    if let Some(s) = staleness {
                        if s.stale {
                            index_section["stale"] = json!(true);
                            index_section["behind_commits"] = json!(s.behind_commits);
                        }
                    }

                    // Drift — only if threshold or path param provided
                    let wants_drift =
                        input.get("threshold").is_some() || input.get("path").is_some();
                    if wants_drift {
                        use crate::tools::output::OutputGuard;
                        let (root3, drift_enabled) = ctx
                            .agent
                            .with_project(|p| {
                                Ok((p.root.clone(), p.config.embeddings.drift_detection_enabled))
                            })
                            .await?;
                        if !drift_enabled {
                            index_section["drift"] = json!({
                                "status": "disabled",
                                "hint": "Set embeddings.drift_detection_enabled = true in .code-explorer/project.toml"
                            });
                        } else {
                            let threshold =
                                input["threshold"].as_f64().map(|v| v as f32).unwrap_or(0.1);
                            let path_filter = input["path"].as_str().map(|s| s.to_string());
                            let guard = OutputGuard::from_input(&input);
                            let rows = tokio::task::spawn_blocking(move || {
                                let conn = crate::embed::index::open_db(&root3)?;
                                crate::embed::index::query_drift_report(
                                    &conn,
                                    Some(threshold),
                                    path_filter.as_deref(),
                                )
                            })
                            .await??;
                            let items: Vec<Value> = rows
                                .iter()
                                .map(|r| {
                                    let mut obj = serde_json::Map::new();
                                    obj.insert("file_path".into(), json!(r.file_path));
                                    obj.insert("avg_drift".into(), json!(r.avg_drift));
                                    obj.insert("max_drift".into(), json!(r.max_drift));
                                    if guard.should_include_body() {
                                        if let Some(chunk) = &r.max_drift_chunk {
                                            obj.insert("max_drift_chunk".into(), json!(chunk));
                                        }
                                    }
                                    Value::Object(obj)
                                })
                                .collect();
                            let (items, overflow) = guard.cap_items(
                                items,
                                "Use detail_level='full' with offset for pagination",
                            );
                            let total = overflow.as_ref().map_or(items.len(), |o| o.total);
                            let mut drift_result = json!({ "results": items, "total": total });
                            if let Some(ov) = overflow {
                                drift_result["overflow"] = OutputGuard::overflow_json(&ov);
                            }
                            index_section["drift"] = drift_result;
                        }
                    }

                    result["index"] = index_section;
                }
                _ => {
                    result["index"] = json!({ "indexed": false, "error": "failed to read index" });
                }
            }
        }

        // --- Usage section ---
        let window = input["window"].as_str().unwrap_or("30d");
        match crate::usage::db::open_db(&root)
            .and_then(|conn| crate::usage::db::query_stats(&conn, window))
        {
            Ok(stats) => {
                result["usage"] = serde_json::to_value(stats).unwrap_or(json!(null));
            }
            Err(_) => {
                result["usage"] = json!({ "window": window, "by_tool": [] });
            }
        }

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
    format!("activated · {name}")
}

fn format_project_status(result: &Value) -> String {
    let root = result["project_root"].as_str().unwrap_or("?");
    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(root);
    let indexed = result["index"]["indexed"].as_bool().unwrap_or(false);
    let index_str = if indexed {
        let files = result["index"]["files"].as_u64().unwrap_or(0);
        let chunks = result["index"]["chunks"].as_u64().unwrap_or(0);
        format!("index:{files}f/{chunks}c")
    } else {
        "index:none".to_string()
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
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
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
        assert!(status["config"]["project"]["name"].is_string());
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
        std::fs::create_dir_all(dir1.path().join(".code-explorer")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".code-explorer")).unwrap();

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
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
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
        assert!(result["config"].is_object(), "missing config section");
        assert!(result.get("index").is_some(), "missing index section");
        assert!(result.get("usage").is_some(), "missing usage section");
        assert!(
            result.get("libraries").is_some(),
            "missing libraries section"
        );
    }
}
