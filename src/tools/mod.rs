//! Tool trait and registry.
//!
//! Each tool is a struct that implements the `Tool` trait. Tools are
//! registered in the MCP server at startup.

pub mod ast;
pub mod config;
pub mod file;
pub mod git;
pub mod library;
pub mod memory;
pub mod output;
pub mod output_buffer;
pub mod semantic;
pub mod symbol;
pub mod usage;
pub use usage::GetUsageStats;
pub mod workflow;

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use crate::agent::Agent;
use crate::lsp::LspProvider;

/// Shared context passed to every tool invocation.
///
/// Holds references to all shared resources (agent state, LSP manager,
/// and eventually parser pool, etc.). Extend this struct as new shared
/// resources are added — all tools get access automatically.
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,
    pub output_buffer: Arc<output_buffer::OutputBuffer>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
