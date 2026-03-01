//! Configuration and project management tools.

use super::{Tool, ToolContext};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ActivateProject;
pub struct GetConfig;

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
}

#[async_trait::async_trait]
impl Tool for GetConfig {
    fn name(&self) -> &str {
        "get_config"
    }
    fn description(&self) -> &str {
        "Display the active project config and server settings."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        ctx.agent
            .with_project(|p| {
                Ok(json!({
                    "project_root": p.root.display().to_string(),
                    "config": serde_json::to_value(&p.config)?,
                }))
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

    #[tokio::test]
    async fn activate_and_get_config() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
        };

        // No project initially
        assert!(GetConfig.call(json!({}), &ctx).await.is_err());

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

        // Now config works
        let config = GetConfig.call(json!({}), &ctx).await.unwrap();
        assert!(config["project_root"].as_str().unwrap().len() > 0);
        assert!(config["config"]["project"]["name"].is_string());
    }

    #[tokio::test]
    async fn activate_nonexistent_path_errors() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: lsp(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
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

        let config = GetConfig.call(json!({}), &ctx).await.unwrap();
        let root = config["project_root"].as_str().unwrap();
        assert!(root.contains(dir2.path().file_name().unwrap().to_str().unwrap()));
    }
}
