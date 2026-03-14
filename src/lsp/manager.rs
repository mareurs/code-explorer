//! Manages per-language LSP client instances with lazy initialization.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::Weak;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::Mutex;

use super::client::{LspClient, LspServerConfig};
use super::servers;

/// Composite key for the LSP client pool: one client per (language, project_root).
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct LspKey {
    pub language: String,
    pub project_root: PathBuf,
}

impl LspKey {
    pub fn new(language: &str, project_root: &Path) -> Self {
        Self {
            language: language.to_string(),
            project_root: project_root.to_path_buf(),
        }
    }
}

/// Manages LSP client instances, one per (language, project_root) pair.
///
/// Clients are lazily started on first use and cached. When the pool
/// reaches `max_clients`, the least-recently-used client is evicted.
/// Clients idle for longer than `idle_ttl` are also reaped by a background
/// task spawned in `new_arc_with_ttl`.
pub struct LspManager {
    clients: Mutex<HashMap<LspKey, Arc<LspClient>>>,
    /// Tracks last access time for LRU eviction.
    last_used: Mutex<HashMap<LspKey, Instant>>,
    /// Per-key startup barrier: concurrent callers for the same key
    /// wait on a `watch` channel. The first caller sends `true` on success or
    /// `false` on failure; late arrivals always see the final value.
    ///
    /// Uses `std::sync::Mutex` (not tokio) so it can be locked in `Drop`
    /// guards, which are synchronous. The lock is never held across `await`
    /// points — only for brief HashMap insert/remove operations.
    starting: StdMutex<HashMap<LspKey, tokio::sync::watch::Receiver<Option<bool>>>>,
    /// Maximum number of concurrent LSP clients before LRU eviction kicks in.
    max_clients: usize,
    /// How long a client may sit idle before the background task evicts it.
    idle_ttl: Duration,
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for LspKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.language, self.project_root.display())
    }
}

/// RAII guard that removes a language entry from the `starting` barrier map
/// when dropped, regardless of how the enclosing scope exits (success, error,
/// or async cancellation).
///
/// This prevents a stale closed-channel entry from accumulating in the map
/// when `do_start` is cancelled mid-flight by a tool timeout.
struct StartingCleanup<'a> {
    starting: &'a StdMutex<HashMap<LspKey, tokio::sync::watch::Receiver<Option<bool>>>>,
    key: LspKey,
}

impl Drop for StartingCleanup<'_> {
    fn drop(&mut self) {
        // best-effort: if another task won the race and re-inserted a live
        // entry while this guard was cancellation-dropped, we leave it alone.
        // In tokio's cooperative scheduling, the cancellation drop runs
        // synchronously inside the current poll — no other task can interleave
        // between the timeout firing and this Drop executing, so in practice
        // this always removes the stale entry and never removes a live one.
        if let Ok(mut map) = self.starting.lock() {
            map.remove(&self.key);
        }
    }
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            last_used: Mutex::new(HashMap::new()),
            starting: StdMutex::new(HashMap::new()),
            max_clients: 5,
            idle_ttl: Duration::from_secs(20 * 60),
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
        let key = LspKey::new(language, workspace_root);

        // Fast path: cache hit.
        {
            let clients = self.clients.lock().await;
            if let Some(client) = clients.get(&key) {
                if client.is_alive() {
                    // Update last_used outside clients lock to avoid deadlock.
                    drop(clients);
                    self.last_used
                        .lock()
                        .await
                        .insert(key.clone(), Instant::now());
                    // Re-fetch since we dropped the lock (another task could have
                    // evicted it, but that's extremely unlikely and we'd just
                    // fall through to the slow path).
                    let clients = self.clients.lock().await;
                    if let Some(client) = clients.get(&key) {
                        return Ok(client.clone());
                    }
                }
            }
        }

        // Resolve the server config early — fail fast for unknown languages
        // before touching the barrier map at all.
        let config = servers::default_config(language, workspace_root).ok_or_else(|| {
            anyhow::anyhow!("No LSP server configured for language: {}", language)
        })?;

        // LRU eviction: if at capacity, shut down the least-recently-used client.
        {
            let clients = self.clients.lock().await;
            if clients.len() >= self.max_clients {
                let last_used = self.last_used.lock().await;
                if let Some(oldest_key) = last_used
                    .iter()
                    .min_by_key(|(_, t)| *t)
                    .map(|(k, _)| k.clone())
                {
                    drop(last_used);
                    let evict_client = clients.get(&oldest_key).cloned();
                    drop(clients);
                    // Remove and shut down outside the lock.
                    {
                        let mut clients = self.clients.lock().await;
                        clients.remove(&oldest_key);
                    }
                    self.last_used.lock().await.remove(&oldest_key);
                    if let Some(old) = evict_client {
                        tracing::info!("LRU evicting LSP client: {}", oldest_key);
                        let _ = old.shutdown().await;
                    }
                }
            }
        }

        // Slow path: need to start (or wait for someone else starting).
        // Use a per-key watch channel: the first caller creates a sender,
        // concurrent callers clone the receiver and wait. Unlike Notify, watch
        // channels never lose signals — late subscribers always see the value.
        let mut rx_opt = None;
        let tx_opt;
        {
            let mut starting = self.starting.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing_rx) = starting.get(&key) {
                // Someone else is already starting this key — grab a receiver.
                rx_opt = Some(existing_rx.clone());
                tx_opt = None;
            } else {
                // We're the first — create the channel and register.
                let (tx, rx) = tokio::sync::watch::channel(None);
                starting.insert(key.clone(), rx);
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
                if let Some(client) = clients.get(&key) {
                    if client.is_alive() {
                        return Ok(client.clone());
                    }
                }
            }
            // Starter failed or client doesn't match — fall through to try ourselves.
            // Clean up the old barrier and register as a new starter.
            let (tx, rx) = tokio::sync::watch::channel(None);
            {
                let mut starting = self.starting.lock().unwrap_or_else(|e| e.into_inner());
                starting.insert(key.clone(), rx);
            }
            return self.do_start(&key, config, tx).await;
        }

        // We're the starter.
        self.do_start(&key, config, tx_opt.expect("tx_opt is always Some when rx_opt is None — set in the same exclusive branch above"))
            .await
    }

    /// Internal: actually start the LSP, update cache, and signal waiters.
    ///
    /// The `StartingCleanup` guard ensures the barrier entry is removed from
    /// `self.starting` on every exit path: success, error, **and async
    /// cancellation** (tool timeout dropping the future mid-flight).
    async fn do_start(
        &self,
        key: &LspKey,
        config: LspServerConfig,
        tx: tokio::sync::watch::Sender<Option<bool>>,
    ) -> Result<Arc<LspClient>> {
        // Register the cleanup guard first. It removes the `starting` entry
        // when this function returns or is cancelled.
        let _cleanup = StartingCleanup {
            starting: &self.starting,
            key: key.clone(),
        };

        // Evict dead client if present.
        // Remove from map first and release the lock, THEN shut down.
        // Calling shutdown().await while holding the clients lock would block
        // all other get_or_start callers for up to 35 seconds.
        let stale_client = {
            let mut clients = self.clients.lock().await;
            if let Some(client) = clients.get(key) {
                if !client.is_alive() {
                    clients.remove(key)
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

        let result = LspClient::start(config).await.map(Arc::new);

        match result {
            Ok(new_client) => {
                // Insert into cache BEFORE signalling waiters.
                {
                    let mut clients = self.clients.lock().await;
                    clients.insert(key.clone(), new_client.clone());
                }
                // Update last_used.
                self.last_used
                    .lock()
                    .await
                    .insert(key.clone(), Instant::now());
                // Signal success. The `starting` entry is removed by _cleanup
                // when this function returns.
                let _ = tx.send(Some(true));
                Ok(new_client)
            }
            Err(e) => {
                // Signal failure. The `starting` entry is removed by _cleanup
                // when this function returns.
                let _ = tx.send(Some(false));
                Err(e)
            }
        }
    }

    pub async fn get(&self, language: &str, project_root: &Path) -> Option<Arc<LspClient>> {
        let key = LspKey::new(language, project_root);
        let clients = self.clients.lock().await;
        clients.get(&key).filter(|c| c.is_alive()).cloned()
    }

    /// Shut down all active LSP servers.
    pub async fn shutdown_all(&self) {
        let mut clients = self.clients.lock().await;
        for (key, client) in clients.drain() {
            tracing::info!("Shutting down LSP for: {}", key);
            match client.shutdown().await {
                Ok(()) => tracing::debug!("LSP server shut down cleanly: {}", key),
                Err(e) => tracing::warn!("Error shutting down LSP for {}: {}", key, e),
            }
        }
        self.last_used.lock().await.clear();
    }

    /// List currently active languages (deduplicated).
    pub async fn active_languages(&self) -> Vec<String> {
        let clients = self.clients.lock().await;
        let mut langs: Vec<String> = clients
            .iter()
            .filter(|(_, c)| c.is_alive())
            .map(|(key, _)| key.language.clone())
            .collect();
        langs.sort();
        langs.dedup();
        langs
    }

    /// Notify LSP clients whose project_root is an ancestor of the changed file.
    /// Each client silently skips the file if it doesn't have it open.
    pub async fn notify_file_changed(&self, path: &std::path::Path) {
        let clients: Vec<_> = self
            .clients
            .lock()
            .await
            .iter()
            .filter(|(key, _)| path.starts_with(&key.project_root))
            .map(|(_, client)| client.clone())
            .collect();
        for client in clients {
            if client.is_alive() {
                let _ = client.did_change(path).await;
            }
        }
    }

    /// Return the number of in-progress language starts. Should be 0 after any
    /// `get_or_start` call completes (success, failure, or cancellation).
    #[cfg(test)]
    pub fn starting_count_sync(&self) -> usize {
        self.starting.lock().unwrap().len()
    }

    /// Like `get_or_start` but accepts a custom `LspServerConfig`, bypassing
    /// `servers::default_config`. Used in tests to inject fake (e.g. `sleep`)
    /// servers so the startup can be cancelled or timed out on demand.
    #[cfg(test)]
    pub async fn get_or_start_for_test(
        &self,
        language: &str,
        config: LspServerConfig,
    ) -> Result<Arc<LspClient>> {
        let workspace_root = config.workspace_root.clone();
        let key = LspKey::new(language, &workspace_root);

        // Fast path
        {
            let clients = self.clients.lock().await;
            if let Some(client) = clients.get(&key) {
                if client.is_alive() {
                    return Ok(client.clone());
                }
            }
        }

        // Barrier
        let mut rx_opt = None;
        let tx_opt;
        {
            let mut starting = self.starting.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing_rx) = starting.get(&key) {
                rx_opt = Some(existing_rx.clone());
                tx_opt = None;
            } else {
                let (tx, rx) = tokio::sync::watch::channel(None);
                starting.insert(key.clone(), rx);
                tx_opt = Some(tx);
            }
        }

        if let Some(mut rx) = rx_opt {
            let _ = rx.wait_for(|v| v.is_some()).await;
            {
                let clients = self.clients.lock().await;
                if let Some(client) = clients.get(&key) {
                    if client.is_alive() {
                        return Ok(client.clone());
                    }
                }
            }
            let (tx, rx) = tokio::sync::watch::channel(None);
            {
                let mut starting = self.starting.lock().unwrap_or_else(|e| e.into_inner());
                starting.insert(key.clone(), rx);
            }
            return self.do_start(&key, config, tx).await;
        }

        self.do_start(&key, config, tx_opt.expect("tx_opt is always Some when rx_opt is None — set in the same exclusive branch above"))
            .await
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
    /// Create a new `Arc<LspManager>` using the default idle TTL (20 minutes)
    /// and spawn a background eviction task.
    pub fn new_arc() -> Arc<Self> {
        Self::new_arc_with_ttl(Duration::from_secs(20 * 60))
    }

    /// Create a new `Arc<LspManager>` with a custom idle TTL and spawn the
    /// background eviction task.  The task holds a `Weak` reference so it
    /// exits automatically when the last `Arc` is dropped.
    pub fn new_arc_with_ttl(ttl: Duration) -> Arc<Self> {
        let mut mgr = Self::new();
        mgr.idle_ttl = ttl;
        let arc = Arc::new(mgr);
        let weak = Arc::downgrade(&arc);
        tokio::spawn(async move {
            Self::idle_eviction_loop(weak, ttl).await;
        });
        arc
    }

    /// Evict all clients that have not been accessed for longer than `ttl`.
    /// Called periodically by the background task; also `pub(crate)` for
    /// direct testing without the background task.
    pub(crate) async fn evict_idle(&self, ttl: Duration) {
        let now = Instant::now();
        let idle_keys: Vec<LspKey> = {
            let last_used = self.last_used.lock().await;
            last_used
                .iter()
                .filter(|(_, t)| now.duration_since(**t) > ttl)
                .map(|(k, _)| k.clone())
                .collect()
        };
        for key in idle_keys {
            let client = {
                let mut clients = self.clients.lock().await;
                clients.remove(&key)
            };
            self.last_used.lock().await.remove(&key);
            if let Some(c) = client {
                tracing::info!("Idle TTL evicting LSP client: {}", key);
                let _ = c.shutdown().await;
            }
        }
    }

    /// Background loop: wakes every `ttl / 4` and calls `evict_idle`.
    /// Exits when the `Weak` can no longer be upgraded (manager dropped).
    async fn idle_eviction_loop(weak: Weak<Self>, ttl: Duration) {
        let interval = ttl / 4;
        loop {
            tokio::time::sleep(interval).await;
            match weak.upgrade() {
                Some(mgr) => mgr.evict_idle(ttl).await,
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manager_starts_empty() {
        let mgr = LspManager::new();
        assert!(mgr.active_languages().await.is_empty());
        assert!(mgr.get("rust", Path::new("/tmp")).await.is_none());
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

    /// After a failed start (unknown language), the barrier map must be empty.
    /// This is the three-query sandwich for the StartingCleanup guard:
    ///   1. starting_count == 0 (baseline)
    ///   2. get_or_start for unknown language fails quickly
    ///   3. starting_count == 0 (guard cleaned up on normal failure exit)
    #[tokio::test]
    async fn failed_start_cleans_up_starting_map() {
        let mgr = LspManager::new();
        let dir = tempfile::tempdir().unwrap();

        // Step 1 — baseline
        assert_eq!(mgr.starting_count_sync(), 0, "map should start empty");

        // Step 2 — unknown language fails immediately (no config exists)
        let result = mgr.get_or_start("brainfuck", dir.path()).await;
        assert!(result.is_err());

        // Step 3 — cleanup guard fired on failure exit
        assert_eq!(
            mgr.starting_count_sync(),
            0,
            "map should be clean after failed start"
        );
    }

    /// After a cancelled start (tool timeout mid-initialize), the barrier map
    /// must be empty. Without the StartingCleanup guard the stale closed-channel
    /// entry would remain in `starting` until the next caller overwrote it.
    ///
    /// Uses `sleep 99999` as a fake LSP: it starts immediately but never writes
    /// to stdout, so `initialize()` blocks until the external timeout fires.
    #[tokio::test]
    async fn cancelled_get_or_start_cleans_up_starting_map() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new();

        // Step 1 — baseline
        assert_eq!(mgr.starting_count_sync(), 0, "map should start empty");

        let config = LspServerConfig {
            command: "sleep".into(),
            args: vec!["99999".into()],
            workspace_root: dir.path().to_path_buf(),
            // Short init timeout so the LSP-level request also fails fast,
            // but the outer tokio::time::timeout fires first.
            init_timeout: Some(std::time::Duration::from_secs(30)),
        };

        // Step 2 — cancel the future after 100 ms (before initialize responds)
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            mgr.get_or_start_for_test("fake-slow-lsp", config),
        )
        .await;
        assert!(cancelled.is_err(), "expected outer timeout");

        // Step 3 — cleanup guard must have fired during the cancellation drop
        assert_eq!(
            mgr.starting_count_sync(),
            0,
            "stale starting entry leaked after cancellation"
        );
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

    #[tokio::test]
    async fn same_language_different_roots_get_separate_clients() {
        let key1 = LspKey::new("rust", Path::new("/project-a"));
        let key2 = LspKey::new("rust", Path::new("/project-b"));
        assert_ne!(key1, key2);

        // HashMap correctly distinguishes them
        let mut map: HashMap<LspKey, &str> = HashMap::new();
        map.insert(key1.clone(), "client-a");
        map.insert(key2.clone(), "client-b");
        assert_eq!(map.get(&key1), Some(&"client-a"));
        assert_eq!(map.get(&key2), Some(&"client-b"));
    }

    #[test]
    fn lsp_key_same_language_same_root_is_equal() {
        let k1 = LspKey::new("typescript", Path::new("/workspace/mcp-server"));
        let k2 = LspKey::new("typescript", Path::new("/workspace/mcp-server"));
        assert_eq!(k1, k2);
    }

    #[test]
    fn lsp_key_display() {
        let key = LspKey::new("rust", Path::new("/my/project"));
        assert_eq!(format!("{}", key), "rust@/my/project");
    }

    // --- Idle TTL eviction tests ---

    /// evict_idle must remove last_used entries whose age exceeds the TTL,
    /// even when no corresponding client exists in the pool (e.g. already
    /// LRU-evicted but last_used not yet cleaned up).
    ///
    /// Three-query sandwich:
    ///   1. Insert stale entry → baseline count = 1
    ///   2. evict_idle with 1 ms TTL → should remove it
    ///   3. Count = 0 → entry cleaned up
    #[tokio::test]
    async fn evict_idle_clears_stale_last_used_entries() {
        let mgr = LspManager::new();
        let key = LspKey::new("rust", Path::new("/stale-project"));

        // Step 1 — baseline: insert a stale entry (1 hour in the past)
        mgr.last_used.lock().await.insert(
            key.clone(),
            Instant::now() - std::time::Duration::from_secs(3600),
        );
        assert_eq!(mgr.last_used.lock().await.len(), 1);

        // Step 2 — evict with a 1 ms TTL; the 1-hour-old entry qualifies
        mgr.evict_idle(std::time::Duration::from_millis(1)).await;

        // Step 3 — stale entry removed
        assert_eq!(mgr.last_used.lock().await.len(), 0);
    }

    /// evict_idle must leave entries whose age is below the TTL untouched.
    #[tokio::test]
    async fn evict_idle_preserves_recent_entries() {
        let mgr = LspManager::new();
        let key = LspKey::new("typescript", Path::new("/fresh-project"));

        // Insert a just-accessed entry
        mgr.last_used
            .lock()
            .await
            .insert(key.clone(), Instant::now());

        // Evict with a 1-hour TTL — the fresh entry should survive
        mgr.evict_idle(std::time::Duration::from_secs(3600)).await;

        assert_eq!(mgr.last_used.lock().await.len(), 1);
    }

    /// The background task spawned by new_arc_with_ttl must automatically
    /// evict a client that has not been accessed since longer than the TTL.
    ///
    /// Uses rust-analyzer as the real LSP; skipped if not installed.
    #[tokio::test]
    async fn idle_background_task_evicts_after_ttl() {
        use std::process::Command as StdCommand;
        if StdCommand::new("rust-analyzer")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}").unwrap();

        let ttl = std::time::Duration::from_millis(300);
        let mgr = LspManager::new_arc_with_ttl(ttl);

        // Start a real LSP client
        mgr.get_or_start("rust", dir.path()).await.unwrap();
        assert!(
            !mgr.active_languages().await.is_empty(),
            "client should be alive"
        );

        // Wait 4× the TTL so the background check interval fires at least once
        tokio::time::sleep(ttl * 4).await;

        // Client must have been evicted
        assert!(
            mgr.active_languages().await.is_empty(),
            "idle client should have been evicted after TTL"
        );
    }
}
