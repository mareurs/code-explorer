use std::path::Path;
use std::sync::Arc;

use crate::lsp::SymbolInfo;

/// Abstract interface over LSP operations used by tools.
/// `LspClient` implements this; `MockLspClient` implements it for tests.
#[async_trait::async_trait]
pub trait LspClientOps: Send + Sync {
    async fn document_symbols(
        &self,
        path: &Path,
        language_id: &str,
    ) -> anyhow::Result<Vec<SymbolInfo>>;

    async fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<SymbolInfo>>;

    async fn references(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>>;

    async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>>;

    async fn hover(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Option<String>>;

    async fn rename(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
        language_id: &str,
    ) -> anyhow::Result<lsp_types::WorkspaceEdit>;

    async fn did_change(&self, path: &Path) -> anyhow::Result<()>;
}

/// Abstract factory that starts or retrieves an LSP client for a given language.
/// `LspManager` implements this; `MockLspProvider` implements it for tests.
#[async_trait::async_trait]
pub trait LspProvider: Send + Sync {
    async fn get_or_start(
        &self,
        language: &str,
        workspace_root: &Path,
    ) -> anyhow::Result<Arc<dyn LspClientOps>>;

    async fn notify_file_changed(&self, path: &Path);

    async fn shutdown_all(&self);
}
