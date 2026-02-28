//! LSP client layer: manages language server processes and exposes a
//! unified async interface for symbol operations.

pub mod client;
pub mod manager;
pub mod mock;
pub mod servers;
pub mod symbols;
pub use mock::{MockLspClient, MockLspProvider};
pub mod ops;
pub use ops::{LspClientOps, LspProvider};
pub mod transport;

pub use client::{LspClient, LspServerConfig};
pub use manager::LspManager;
pub use symbols::{SymbolInfo, SymbolKind};
