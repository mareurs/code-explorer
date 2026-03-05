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

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_get_usage_stats(result))
    }
}

fn format_get_usage_stats(result: &Value) -> String {
    let window = result["window"].as_str().unwrap_or("?");
    let by_tool = match result["by_tool"].as_array() {
        Some(t) => t,
        None => return format!("usage · {window}"),
    };

    let mut tools: Vec<&Value> = by_tool
        .iter()
        .filter(|t| t["calls"].as_u64().unwrap_or(0) > 0)
        .collect();
    tools.sort_by(|a, b| {
        b["calls"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["calls"].as_u64().unwrap_or(0))
    });

    if tools.is_empty() {
        return format!("usage · {window} · no calls");
    }

    let name_width = tools
        .iter()
        .filter_map(|t| t["tool"].as_str())
        .map(|n| n.len())
        .max()
        .unwrap_or(4)
        .max(4);

    const MAX_TOOLS: usize = 10;
    let mut out = format!("usage · {window}\n");
    out.push_str(&format!(
        "\n  {:<name_width$}  {:>5}  {:>6}  {:>6}",
        "tool", "calls", "errors", "p50ms"
    ));
    out.push_str(&format!("\n  {}", "─".repeat(name_width + 22)));

    for tool in tools.iter().take(MAX_TOOLS) {
        let name = tool["tool"].as_str().unwrap_or("?");
        let calls = tool["calls"].as_u64().unwrap_or(0);
        let errors = tool["errors"].as_u64().unwrap_or(0);
        let p50 = tool["p50_ms"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "\n  {name:<name_width$}  {calls:>5}  {errors:>6}  {p50:>6}"
        ));
    }

    let hidden = tools.len().saturating_sub(MAX_TOOLS);
    if hidden > 0 {
        out.push_str(&format!("\n\n  … +{hidden} more tools"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::lsp::manager::LspManager;
    use crate::tools::ToolContext;
    use tempfile::TempDir;

    async fn ctx_with_project(root: &std::path::Path) -> ToolContext {
        std::fs::create_dir_all(root.join(".codescout")).unwrap();
        let agent = Agent::new(Some(root.to_path_buf())).await.unwrap();
        ToolContext {
            agent,
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
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
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
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

    // --- format_get_usage_stats tests ---

    #[test]
    fn format_get_usage_stats_shows_per_tool_table() {
        let result = serde_json::json!({
            "window": "1h",
            "by_tool": [
                {"tool": "find_symbol", "calls": 47, "errors": 0, "overflows": 0, "p50_ms": 12, "p99_ms": 50, "error_rate_pct": 0.0, "overflow_rate_pct": 0.0},
                {"tool": "run_command", "calls": 18, "errors": 2, "overflows": 0, "p50_ms": 340, "p99_ms": 800, "error_rate_pct": 11.1, "overflow_rate_pct": 0.0},
                {"tool": "list_symbols", "calls": 0, "errors": 0, "overflows": 0, "p50_ms": 0, "p99_ms": 0, "error_rate_pct": 0.0, "overflow_rate_pct": 0.0}
            ]
        });
        let out = format_get_usage_stats(&result);
        assert!(out.contains("1h"), "should show window, got: {out}");
        assert!(
            out.contains("find_symbol"),
            "should show tool name, got: {out}"
        );
        assert!(out.contains("47"), "should show call count, got: {out}");
        assert!(
            out.contains("run_command"),
            "should show tool with errors, got: {out}"
        );
        assert!(
            !out.contains("list_symbols"),
            "should omit tools with 0 calls, got: {out}"
        );
    }

    #[test]
    fn format_get_usage_stats_no_calls() {
        let result = serde_json::json!({
            "window": "1h",
            "by_tool": []
        });
        let out = format_get_usage_stats(&result);
        assert!(
            out.contains("no calls") || out.contains('0'),
            "should handle empty, got: {out}"
        );
    }
}
