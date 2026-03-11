//! codescout: high-performance coding agent MCP server.
//!
//! Provides IDE-grade code intelligence to LLMs via the Model Context Protocol.

pub mod agent;
pub mod ast;
pub mod config;
#[cfg(feature = "dashboard")]
pub mod dashboard;
pub mod embed;
pub mod git;
pub mod library;
pub mod logging;
pub mod lsp;
pub mod memory;
pub mod prompts;
pub mod server;
pub mod tools;
pub mod usage;
pub mod util;
