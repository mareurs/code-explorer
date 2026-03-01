//! Tool trait and registry.
//!
//! Each tool is a struct that implements the `Tool` trait. Tools are
//! registered in the MCP server at startup.

pub mod ast;
pub mod command_summary;
pub mod config;
pub mod file;
pub mod file_summary;
pub mod git;
pub mod library;
pub mod memory;
pub mod output;
pub mod output_buffer;
pub mod progress;
pub mod semantic;
pub mod symbol;
pub mod usage;
pub use usage::GetUsageStats;
pub mod user_format;
pub mod workflow;

use std::sync::Arc;

use anyhow::Result;
use rmcp::model::{Content, Role};
use serde_json::Value;

use crate::agent::Agent;
use crate::lsp::LspProvider;

/// Compact JSON size above which tool output is routed through OutputBuffer.
const TOOL_OUTPUT_BUFFER_THRESHOLD: usize = 10_000;

/// Shared context passed to every tool invocation.
///
/// Holds references to all shared resources (agent state, LSP manager,
/// and eventually parser pool, etc.). Extend this struct as new shared
/// resources are added — all tools get access automatically.
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,
    pub output_buffer: Arc<output_buffer::OutputBuffer>,
    pub progress: Option<Arc<progress::ProgressReporter>>,
}

/// A recoverable tool error: the LLM gave bad input and can self-correct.
///
/// When a tool returns this error type, the MCP server serialises it as
/// `isError: false` with a JSON body containing `"error"` and optional
/// `"hint"` fields.  This prevents Claude Code from aborting sibling
/// parallel tool calls (which it does when it sees `isError: true`).
///
/// Use this for **expected, input-driven failures**: path not found,
/// unsupported file type, empty glob match, no index built yet, etc.
///
/// Keep returning plain `anyhow` errors (→ `isError: true`) for genuine
/// failures: panics, security violations, LSP crashes.
#[derive(Debug)]
pub struct RecoverableError {
    /// Human-readable description of what went wrong.
    pub message: String,
    /// Optional LLM-facing suggestion for how to correct the call.
    pub hint: Option<String>,
}

impl RecoverableError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: Some(hint.into()),
        }
    }
}

impl std::fmt::Display for RecoverableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RecoverableError {}

/// Convenience: extract a required parameter from a JSON `Value`, returning
/// `RecoverableError` (not a fatal error) if it is missing.
pub fn require_param<'a>(
    input: &'a serde_json::Value,
    name: &str,
) -> anyhow::Result<&'a serde_json::Value> {
    input.get(name).ok_or_else(|| {
        RecoverableError::with_hint(
            format!("missing '{}' parameter", name),
            format!("Add the required '{}' parameter to the tool call.", name),
        )
        .into()
    })
}

/// Convenience: extract a required string parameter from a JSON `Value`.
pub fn require_str_param<'a>(input: &'a serde_json::Value, name: &str) -> anyhow::Result<&'a str> {
    require_param(input, name)?.as_str().ok_or_else(|| {
        RecoverableError::with_hint(
            format!("'{}' must be a string", name),
            format!("Provide '{}' as a string value.", name),
        )
        .into()
    })
}

/// Convenience: extract a required u64 parameter from a JSON `Value`.
pub fn require_u64_param(input: &serde_json::Value, name: &str) -> anyhow::Result<u64> {
    let val = require_param(input, name)?;
    // Accept both JSON numbers and string-encoded integers (LLMs sometimes quote them).
    if let Some(n) = val.as_u64() {
        return Ok(n);
    }
    if let Some(s) = val.as_str() {
        if let Ok(n) = s.trim().parse::<u64>() {
            return Ok(n);
        }
    }
    Err(RecoverableError::with_hint(
        format!("'{}' must be a non-negative integer", name),
        format!("Provide '{}' as a non-negative integer.", name),
    )
    .into())
}

/// Block write operations when git worktrees exist but the agent hasn't
/// explicitly called `activate_project` to confirm which project to write to.
///
/// Returns `Ok(())` when writes are allowed:
/// - Agent explicitly activated a project via `activate_project`
/// - No git worktrees exist (no ambiguity)
///
/// Returns `RecoverableError` when writes should be blocked:
/// - Worktrees exist AND the project was only implicitly set at startup
pub async fn guard_worktree_write(ctx: &ToolContext) -> anyhow::Result<()> {
    if ctx.agent.is_project_explicitly_activated().await {
        return Ok(());
    }
    let root = ctx.agent.require_project_root().await?;
    let worktrees = crate::util::path_security::list_git_worktrees(&root);
    if worktrees.is_empty() {
        return Ok(());
    }
    let wt_list: Vec<String> = worktrees.iter().map(|p| p.display().to_string()).collect();
    let hint = format!(
        "Call activate_project(\"{}\") to select the write target (or use \"{}\") for the main repo.",
        wt_list[0],
        root.display()
    );
    Err(RecoverableError::with_hint(
        format!(
            "Write blocked: git worktrees detected but activate_project has not been called. \
             Worktrees: [{}]",
            wt_list.join(", ")
        ),
        hint,
    )
    .into())
}

/// A single MCP tool exposed to the LLM.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as exposed over MCP (e.g. "find_symbol")
    fn name(&self) -> &str;

    /// Short description shown to the LLM
    fn description(&self) -> &str;

    /// JSON Schema for the input parameters
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given input (already parsed from JSON)
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;

    /// Optional human-readable formatting for the tool result.
    /// When Some, call_content() emits dual-audience blocks:
    ///   1. Compact JSON (audience: assistant)
    ///   2. Formatted plain text (audience: user)
    fn format_for_user(&self, _result: &Value) -> Option<String> {
        None
    }

    /// Returns MCP content blocks for this tool call.
    ///
    /// If format_for_user() returns Some, emits dual-audience blocks:
    /// compact JSON for the assistant, formatted text for the user.
    /// Otherwise, falls back to pretty-printed JSON for both.
    /// Override directly for full control over content blocks.
    async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
        let val = self.call(input, ctx).await?;
        let json = serde_json::to_string(&val).unwrap_or_else(|_| val.to_string());

        if json.len() > TOOL_OUTPUT_BUFFER_THRESHOLD {
            let ref_id = ctx.output_buffer.store_tool(self.name(), json.clone());
            let summary = self
                .format_for_user(&val)
                .unwrap_or_else(|| format!("Result stored in {} ({} bytes)", ref_id, json.len()));
            return Ok(vec![Content::text(format!(
                "{}\nFull result: {}",
                summary, ref_id
            ))]);
        }

        match self.format_for_user(&val) {
            Some(user_text) => Ok(vec![
                Content::text(json).with_audience(vec![Role::Assistant]),
                Content::text(user_text).with_audience(vec![Role::User]),
            ]),
            None => Ok(vec![Content::text(
                serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string()),
            )]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_context_has_progress_field() {
        // Compile-only test: ensures the progress field exists and has the right type.
        fn _check_progress_field_type(_ctx: &ToolContext) {
            let _p: &Option<std::sync::Arc<progress::ProgressReporter>> = &_ctx.progress;
        }
    }

    #[test]
    fn recoverable_error_stores_message() {
        let e = RecoverableError::new("path not found");
        assert_eq!(e.message, "path not found");
        assert!(e.hint.is_none());
    }

    #[test]
    fn recoverable_error_stores_hint() {
        let e = RecoverableError::with_hint("path not found", "use list_dir to explore");
        assert_eq!(e.message, "path not found");
        assert_eq!(e.hint.as_deref(), Some("use list_dir to explore"));
    }

    #[test]
    fn recoverable_error_display_shows_message() {
        let e = RecoverableError::with_hint("file missing", "check the path");
        assert_eq!(e.to_string(), "file missing");
    }

    #[test]
    fn require_u64_param_accepts_integer() {
        let input = serde_json::json!({ "n": 42 });
        assert_eq!(require_u64_param(&input, "n").unwrap(), 42);
    }

    #[test]
    fn require_u64_param_accepts_string_encoded_integer() {
        // LLMs sometimes quote integers — we must tolerate this.
        let input = serde_json::json!({ "n": "11" });
        assert_eq!(require_u64_param(&input, "n").unwrap(), 11);
    }

    #[test]
    fn require_u64_param_rejects_non_numeric_string() {
        let input = serde_json::json!({ "n": "abc" });
        assert!(require_u64_param(&input, "n").is_err());
    }

    #[test]
    fn require_u64_param_rejects_negative_string() {
        let input = serde_json::json!({ "n": "-5" });
        assert!(require_u64_param(&input, "n").is_err());
    }
    #[test]
    fn recoverable_error_downcasts_from_anyhow() {
        let e: anyhow::Error = RecoverableError::new("test error").into();
        assert!(
            e.downcast_ref::<RecoverableError>().is_some(),
            "must be recoverable via downcast"
        );
    }

    // ---- call_content auto-buffering tests ----

    async fn bare_ctx() -> ToolContext {
        ToolContext {
            agent: crate::agent::Agent::new(None).await.unwrap(),
            lsp: crate::lsp::LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(output_buffer::OutputBuffer::new(20)),
            progress: None,
        }
    }

    struct EchoTool {
        result: Value,
        user_summary: Option<String>,
    }

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo_tool"
        }
        fn description(&self) -> &str {
            "test"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({})
        }
        async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
            Ok(self.result.clone())
        }
        fn format_for_user(&self, _result: &Value) -> Option<String> {
            self.user_summary.clone()
        }
    }

    #[tokio::test]
    async fn call_content_passthrough_small_output() {
        let ctx = bare_ctx().await;
        let result = serde_json::json!({"key": "value"});
        let tool = EchoTool {
            result: result.clone(),
            user_summary: None,
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        // Small output: no buffering — content should contain the JSON
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        assert!(text.contains("key"));
    }

    #[tokio::test]
    async fn call_content_buffers_large_output() {
        let ctx = bare_ctx().await;
        // Build a Value that serializes to > 10_000 bytes
        let big_array: Vec<Value> = (0..500)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "name": format!("symbol_{}", i),
                    "file": "src/tools/symbol.rs"
                })
            })
            .collect();
        let result = serde_json::json!({ "symbols": big_array });
        let tool = EchoTool {
            result,
            user_summary: None,
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        // Must return exactly 1 Content item
        assert_eq!(content.len(), 1);
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        // Contains a @tool_ ref handle
        assert!(text.contains("@tool_"), "expected @tool_ ref in: {}", text);
    }

    #[tokio::test]
    async fn call_content_uses_format_for_user_in_compact_text() {
        let ctx = bare_ctx().await;
        let big_array: Vec<Value> = (0..500)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "name": format!("symbol_{}", i)
                })
            })
            .collect();
        let result = serde_json::json!({ "symbols": big_array });
        let tool = EchoTool {
            result,
            user_summary: Some("Found 500 symbols".to_string()),
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        assert!(
            text.contains("Found 500 symbols"),
            "expected summary in: {}",
            text
        );
        assert!(text.contains("@tool_"), "expected ref handle in: {}", text);
    }

    #[tokio::test]
    async fn call_content_generic_fallback_without_format_for_user() {
        let ctx = bare_ctx().await;
        let big_array: Vec<Value> = (0..500)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "name": format!("symbol_{}", i)
                })
            })
            .collect();
        let result = serde_json::json!({ "symbols": big_array });
        let tool = EchoTool {
            result,
            user_summary: None,
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        // No format_for_user → generic fallback message with byte count and ref
        assert!(
            text.contains("bytes") || text.contains("stored"),
            "expected fallback in: {}",
            text
        );
        assert!(text.contains("@tool_"), "expected ref handle in: {}", text);
    }
}
