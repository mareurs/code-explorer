//! Manages per-language LSP client instances with lazy initialization.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use super::client::LspClient;
use super::servers;

/// Manages LSP client instances, one per language.
///
/// Clients are lazily started on first use and cached. If a client's
/// workspace root changes (e.g. project switch), the old client is
/// shut down and a new one started.
pub struct LspManager {
    clients: Mutex<HashMap<String, Arc<LspClient>>>,
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// Get an existing client for the language, or start one.
    ///
    /// If the existing client has a different workspace root or has crashed,
    /// it is replaced with a new instance.
    ///
    /// The mutex is held only for the fast cache check, not during the slow
    /// LSP process startup.  This allows concurrent cold-starts for different
    /// languages to proceed in parallel.
    pub async fn get_or_start(
        &self,
        language: &str,
        workspace_root: &Path,
    ) -> Result<Arc<LspClient>> {
        // Phase 1: quick cache check under a short lock.
        // If alive and matching workspace, return immediately.
        // If dead or wrong workspace, evict it now so we restart below.
        {
            let mut clients = self.clients.lock().await;
            if let Some(client) = clients.get(language) {
                if client.is_alive() && client.workspace_root == workspace_root {
                    return Ok(client.clone());
                }
                // Dead or wrong workspace — evict.
                let old = clients.remove(language).unwrap();
                let _ = old.shutdown().await;
            }
        }
        // Lock released: concurrent callers for *different* languages now
        // start their LSP processes in parallel.

        let config = servers::default_config(language, workspace_root).ok_or_else(|| {
            anyhow::anyhow!("No LSP server configured for language: {}", language)
        })?;

        let new_client = Arc::new(LspClient::start(config).await?);

        // Phase 2: re-acquire to insert.  A concurrent caller for the *same*
        // language may have already stored a client while we were starting
        // ours — if so, prefer theirs and discard ours.
        let mut clients = self.clients.lock().await;
        if let Some(existing) = clients.get(language) {
            if existing.is_alive() && existing.workspace_root == workspace_root {
                let _ = new_client.shutdown().await;
                return Ok(existing.clone());
            }
        }
        clients.insert(language.to_string(), new_client.clone());
        Ok(new_client)
    }

    /// Get an existing alive client without starting one.
    pub async fn get(&self, language: &str) -> Option<Arc<LspClient>> {
        let clients = self.clients.lock().await;
        clients.get(language).filter(|c| c.is_alive()).cloned()
    }

    /// Shut down all active LSP servers.
    pub async fn shutdown_all(&self) {
        let mut clients = self.clients.lock().await;
        for (lang, client) in clients.drain() {
            tracing::info!("Shutting down LSP for: {}", lang);
            if let Err(e) = client.shutdown().await {
                tracing::warn!("Error shutting down LSP for {}: {}", lang, e);
            }
        }
    }

    /// List currently active languages.
    pub async fn active_languages(&self) -> Vec<String> {
        let clients = self.clients.lock().await;
        clients
            .iter()
            .filter(|(_, c)| c.is_alive())
            .map(|(lang, _)| lang.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manager_starts_empty() {
        let mgr = LspManager::new();
        assert!(mgr.active_languages().await.is_empty());
        assert!(mgr.get("rust").await.is_none());
    }

    #[tokio::test]
    async fn manager_errors_for_unknown_language() {
        let mgr = LspManager::new();
        let dir = tempfile::tempdir().unwrap();
        let result = mgr.get_or_start("brainfuck", dir.path()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn manager_shutdown_all_empty() {
        let mgr = LspManager::new();
        mgr.shutdown_all().await; // Should not panic
    }
}
