//! code-explorer: high-performance coding agent MCP server.
//!
//! Provides IDE-grade code intelligence to LLMs via the Model Context Protocol.

pub mod agent;
pub mod ast;
pub mod config;
pub mod embed;
pub mod git;
pub mod library;
pub mod lsp;
pub mod memory;
pub mod prompts;
pub mod server;
pub mod tools;
pub mod util;
