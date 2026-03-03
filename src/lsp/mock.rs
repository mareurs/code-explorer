//! Test-only mock implementations of LspClientOps and LspProvider.
//! Returned symbol lists are configured at construction time; all
//! other LSP methods return Ok(Default::default()) silently.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::lsp::ops::{LspClientOps, LspProvider};
use crate::lsp::SymbolInfo;

pub struct MockLspClient {
    symbols: HashMap<PathBuf, Vec<SymbolInfo>>,
    definitions: HashMap<(u32, u32), Vec<lsp_types::Location>>,
    workspace_results: Vec<SymbolInfo>,
}

impl MockLspClient {
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
            definitions: HashMap::new(),
            workspace_results: vec![],
        }
    }

    /// Pre-load symbol results for a given file path.
    /// The path must match exactly what the tool passes to `document_symbols`.
    pub fn with_symbols(mut self, path: impl Into<PathBuf>, syms: Vec<SymbolInfo>) -> Self {
        self.symbols.insert(path.into(), syms);
        self
    }

    /// Pre-load definition results for a specific (line, col) position (0-indexed).
    /// `goto_definition` returns these locations only when called with an exact match.
    pub fn with_definitions(
        mut self,
        line: u32,
        col: u32,
        locations: Vec<lsp_types::Location>,
    ) -> Self {
        self.definitions.insert((line, col), locations);
        self
    }

    /// Pre-load workspace/symbol results returned for any query.
    pub fn with_workspace_symbols(mut self, syms: Vec<SymbolInfo>) -> Self {
        self.workspace_results = syms;
        self
    }
}

impl Default for MockLspClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LspClientOps for MockLspClient {
    async fn document_symbols(
        &self,
        path: &Path,
        _language_id: &str,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        Ok(self.symbols.get(path).cloned().unwrap_or_default())
    }

    async fn workspace_symbols(&self, _query: &str) -> anyhow::Result<Vec<SymbolInfo>> {
        Ok(self.workspace_results.clone())
    }

    async fn references(
        &self,
        _path: &Path,
        _line: u32,
        _col: u32,
        _language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        Ok(vec![])
    }

    async fn goto_definition(
        &self,
        _path: &Path,
        line: u32,
        col: u32,
        _language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        Ok(self
            .definitions
            .get(&(line, col))
            .cloned()
            .unwrap_or_default())
    }

    async fn hover(
        &self,
        _path: &Path,
        _line: u32,
        _col: u32,
        _language_id: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn rename(
        &self,
        _path: &Path,
        _line: u32,
        _col: u32,
        _new_name: &str,
        _language_id: &str,
    ) -> anyhow::Result<lsp_types::WorkspaceEdit> {
        Ok(lsp_types::WorkspaceEdit::default())
    }

    async fn did_change(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }
}

pub struct MockLspProvider {
    client: Arc<MockLspClient>,
}

impl MockLspProvider {
    pub fn with_client(client: MockLspClient) -> Arc<dyn LspProvider> {
        Arc::new(Self {
            client: Arc::new(client),
        })
    }
}

#[async_trait::async_trait]
impl LspProvider for MockLspProvider {
    async fn get_or_start(
        &self,
        _language: &str,
        _workspace_root: &Path,
    ) -> anyhow::Result<Arc<dyn LspClientOps>> {
        Ok(Arc::clone(&self.client) as Arc<dyn LspClientOps>)
    }

    async fn notify_file_changed(&self, _path: &Path) {}

    async fn shutdown_all(&self) {}
}
