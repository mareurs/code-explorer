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
    /// Per-language startup barrier: concurrent callers for the same language
    /// wait on a `watch` channel. The first caller sends `true` on success or
    /// `false` on failure; late arrivals always see the final value.
    starting: Mutex<HashMap<String, tokio::sync::watch::Receiver<Option<bool>>>>,
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
            starting: Mutex::new(HashMap::new()),
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
        // Fast path: cache hit.
        {
            let clients = self.clients.lock().await;
            if let Some(client) = clients.get(language) {
                if client.is_alive() && client.workspace_root == workspace_root {
                    return Ok(client.clone());
                }
            }
        }

        // Slow path: need to start (or wait for someone else starting).
        // Use a per-language watch channel: the first caller creates a sender,
        // concurrent callers clone the receiver and wait. Unlike Notify, watch
        // channels never lose signals — late subscribers always see the value.
        let mut rx_opt = None;
        let tx_opt;
        {
            let mut starting = self.starting.lock().await;
            if let Some(existing_rx) = starting.get(language) {
                // Someone else is already starting this language — grab a receiver.
                rx_opt = Some(existing_rx.clone());
                tx_opt = None;
            } else {
                // We're the first — create the channel and register.
                let (tx, rx) = tokio::sync::watch::channel(None);
                starting.insert(language.to_string(), rx);
                tx_opt = Some(tx);
            }
        }

        // If we're a waiter, wait for the starter to finish.
        if let Some(mut rx) = rx_opt {
            // Wait until the value changes from None to Some(bool).
            let _ = rx.wait_for(|v| v.is_some()).await;
            // Check the cache — starter should have inserted on success.
            // IMPORTANT: scope the lock so it drops before any call to do_start,
            // which also locks `self.clients`. Tokio Mutex is not reentrant —
            // holding it while calling do_start would deadlock.
            {
                let clients = self.clients.lock().await;
                if let Some(client) = clients.get(language) {
                    if client.is_alive() && client.workspace_root == workspace_root {
                        return Ok(client.clone());
                    }
                }
            }
            // Starter failed or client doesn't match — fall through to try ourselves.
            // Clean up the old barrier and register as a new starter.
            let (tx, rx) = tokio::sync::watch::channel(None);
            {
                let mut starting = self.starting.lock().await;
                starting.insert(language.to_string(), rx);
            }
            return self.do_start(language, workspace_root, tx).await;
        }

        // We're the starter.
        self.do_start(language, workspace_root, tx_opt.unwrap())
            .await
    }

    /// Internal: actually start the LSP, update cache, and signal waiters.
    async fn do_start(
        &self,
        language: &str,
        workspace_root: &Path,
        tx: tokio::sync::watch::Sender<Option<bool>>,
    ) -> Result<Arc<LspClient>> {
        // Evict dead/stale client if present.
        // Remove from map first and release the lock, THEN shut down.
        // Calling shutdown().await while holding the clients lock would block
        // all other get_or_start callers (any language) for up to 35 seconds.
        let stale_client = {
            let mut clients = self.clients.lock().await;
            if let Some(client) = clients.get(language) {
                if !client.is_alive() || client.workspace_root != workspace_root {
                    clients.remove(language)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(old) = stale_client {
            let _ = old.shutdown().await;
        }

        let config = servers::default_config(language, workspace_root)
            .ok_or_else(|| anyhow::anyhow!("No LSP server configured for language: {}", language));

        let result = match config {
            Ok(config) => LspClient::start(config).await.map(Arc::new),
            Err(e) => Err(e),
        };

        match result {
            Ok(new_client) => {
                // Insert into cache BEFORE signalling waiters.
                {
                    let mut clients = self.clients.lock().await;
                    clients.insert(language.to_string(), new_client.clone());
                }
                // Signal success and clean up barrier.
                let _ = tx.send(Some(true));
                {
                    let mut starting = self.starting.lock().await;
                    starting.remove(language);
                }
                Ok(new_client)
            }
            Err(e) => {
                // Signal failure and clean up barrier.
                let _ = tx.send(Some(false));
                {
                    let mut starting = self.starting.lock().await;
                    starting.remove(language);
                }
                Err(e)
            }
        }
    }

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

    /// Notify all active LSP clients that a file was modified on disk by a write tool.
    /// Each client silently skips the file if it doesn't have it open.
    pub async fn notify_file_changed(&self, path: &std::path::Path) {
        let clients: Vec<_> = self.clients.lock().await.values().cloned().collect();
        for client in clients {
            if client.is_alive() {
                let _ = client.did_change(path).await;
            }
        }
    }
}

#[async_trait::async_trait]
impl crate::lsp::ops::LspProvider for LspManager {
    async fn get_or_start(
        &self,
        language: &str,
        workspace_root: &std::path::Path,
    ) -> anyhow::Result<Arc<dyn crate::lsp::ops::LspClientOps>> {
        let client = LspManager::get_or_start(self, language, workspace_root).await?;
        Ok(client as Arc<dyn crate::lsp::ops::LspClientOps>)
    }

    async fn notify_file_changed(&self, path: &std::path::Path) {
        LspManager::notify_file_changed(self, path).await
    }

    async fn shutdown_all(&self) {
        LspManager::shutdown_all(self).await
    }
}

impl LspManager {
    /// Convenience constructor returning `Arc<dyn LspProvider>`.
    /// Use this everywhere `ToolContext` is constructed instead of `Arc::new(LspManager::new())`.
    pub fn new_arc() -> Arc<dyn crate::lsp::ops::LspProvider> {
        Arc::new(Self::new())
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

    #[tokio::test]
    async fn shutdown_all_stops_running_servers() {
        use std::process::Command as StdCommand;

        // Check if rust-analyzer is available
        if StdCommand::new("rust-analyzer")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        // Create minimal Cargo project
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}").unwrap();

        let mgr = LspManager::new();
        let client = mgr.get_or_start("rust", dir.path()).await.unwrap();
        assert!(client.is_alive());

        mgr.shutdown_all().await;

        // After shutdown, the client should be dead
        assert!(!client.is_alive());
        assert!(mgr.active_languages().await.is_empty());
    }
}
