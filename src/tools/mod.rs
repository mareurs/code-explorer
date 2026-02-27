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
pub mod semantic;
pub mod symbol;
pub mod usage;
pub use usage::GetUsageStats;
pub mod workflow;

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use crate::agent::Agent;
use crate::lsp::LspManager;

/// Shared context passed to every tool invocation.
///
/// Holds references to all shared resources (agent state, LSP manager,
/// and eventually parser pool, etc.). Extend this struct as new shared
/// resources are added — all tools get access automatically.
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<LspManager>,
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
    fn recoverable_error_downcasts_from_anyhow() {
        let e: anyhow::Error = RecoverableError::new("test error").into();
        assert!(
            e.downcast_ref::<RecoverableError>().is_some(),
            "must be recoverable via downcast"
        );
    }
}
