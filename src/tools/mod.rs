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
