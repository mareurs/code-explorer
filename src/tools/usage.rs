use crate::tools::{RecoverableError, Tool, ToolContext};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct GetUsageStats;

#[async_trait]
impl Tool for GetUsageStats {
    fn name(&self) -> &str {
        "get_usage_stats"
    }

    fn description(&self) -> &str {
        "Get tool call statistics for the current project. Returns per-tool call counts, \
         error rates, overflow rates, and latency percentiles (p50/p99) for a time window. \
         Use this to diagnose agent behavior: high overflow_rate_pct means queries are too \
         broad; high error_rate_pct on a tool means it is failing repeatedly. \
         Prefer this over manual log inspection."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "enum": ["1h", "24h", "7d", "30d"],
                    "description": "Time window for aggregation. Default: 30d."
                }
            }
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value> {
        let window = input["window"].as_str().unwrap_or("30d");

        let project_root = ctx
            .agent
            .with_project(|p| Ok(p.root.clone()))
            .await
            .map_err(|_| {
                RecoverableError::with_hint("no active project", "run activate_project first")
            })?;

        let conn = crate::usage::db::open_db(&project_root)?;
        let stats = crate::usage::db::query_stats(&conn, window)?;
        Ok(serde_json::to_value(stats)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::manager::LspManager;
    use crate::tools::ToolContext;
    use tempfile::TempDir;

    async fn ctx_with_project(root: &std::path::Path) -> ToolContext {
        std::fs::create_dir_all(root.join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(root.to_path_buf())).await.unwrap();
        ToolContext {
            agent,
            lsp: LspManager::new_arc(),
        }
    }

    #[tokio::test]
    async fn returns_empty_stats_on_fresh_project() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_project(dir.path()).await;
        let tool = GetUsageStats;
        let result = tool.call(serde_json::json!({}), &ctx).await.unwrap();
        assert_eq!(result["total_calls"], 0);
        assert_eq!(result["window"], "30d");
        assert!(result["by_tool"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn returns_error_without_active_project() {
        let agent = Agent::new(None).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: LspManager::new_arc(),
        };
        let tool = GetUsageStats;
        let result = tool.call(serde_json::json!({}), &ctx).await;
        // RecoverableError — isError:false at MCP level; Err at call() level
        let err = result.unwrap_err();
        assert!(
            err.downcast_ref::<RecoverableError>().is_some(),
            "no-project error must be RecoverableError so sibling calls are not aborted"
        );
    }

    #[tokio::test]
    async fn respects_window_parameter() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_project(dir.path()).await;
        let tool = GetUsageStats;
        let result = tool
            .call(serde_json::json!({"window": "1h"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["window"], "1h");
    }
}
