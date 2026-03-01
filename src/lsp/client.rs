//! Async LSP client: spawns a language server subprocess and communicates
//! via JSON-RPC over stdio.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::BufReader;
use tokio::process::{ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use super::transport;

/// Convert an LSP `file://` URI back to a filesystem path.
///
/// Uses `url::Url` for correct handling of Windows drive letters,
/// UNC paths, and percent-encoding. Falls back to raw path extraction
/// if the URI cannot be parsed.
fn uri_to_path(uri: &lsp_types::Uri) -> PathBuf {
    // Parse the lsp_types URI string with url::Url which handles
    // Windows drive letters and percent-encoding correctly.
    url::Url::parse(uri.as_str())
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .unwrap_or_else(|| PathBuf::from(uri.path().as_str()))
}

/// Convert a filesystem path to an LSP `file://` URI.
///
/// Uses `url::Url` for correct encoding of special characters and
/// proper `file:///` formatting on all platforms.
fn path_to_uri(path: &Path) -> Result<lsp_types::Uri> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let u = url::Url::from_file_path(&abs)
        .map_err(|_| anyhow::anyhow!("cannot convert path to URI: {}", abs.display()))?;
    u.as_str()
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid URI: {}", e))
}

/// Convert hierarchical `DocumentSymbol[]` into our `SymbolInfo` tree.
fn convert_document_symbols(
    symbols: &[lsp_types::DocumentSymbol],
    file: &PathBuf,
    parent_path: &str,
) -> Vec<super::SymbolInfo> {
    symbols
        .iter()
        .map(|ds| {
            let name_path = if parent_path.is_empty() {
                ds.name.clone()
            } else {
                format!("{}/{}", parent_path, ds.name)
            };
            let children = ds
                .children
                .as_ref()
                .map(|c| convert_document_symbols(c, file, &name_path))
                .unwrap_or_default();
            super::SymbolInfo {
                name: ds.name.clone(),
                name_path: name_path.clone(),
                kind: ds.kind.into(),
                file: file.clone(),
                start_line: ds.selection_range.start.line,
                end_line: ds.range.end.line,
                start_col: ds.selection_range.start.character,
                children,
                detail: ds.detail.clone().filter(|s| !s.is_empty()),
            }
        })
        .collect()
}

/// Configuration for launching a language server.
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    pub command: String,
    #[allow(dead_code)]
    pub args: Vec<String>,
    pub workspace_root: std::path::PathBuf,
    /// Timeout for the LSP `initialize` handshake. JVM-based servers need longer.
    /// Defaults to 30s if not set.
    pub init_timeout: Option<std::time::Duration>,
}

/// A running LSP client session connected to a language server process.
pub struct LspClient {
    writer: Mutex<ChildStdin>,
    #[allow(dead_code)]
    next_id: AtomicI64,
    #[allow(dead_code)]
    pending: Arc<StdMutex<HashMap<i64, oneshot::Sender<Result<Value>>>>>,
    #[allow(dead_code)]
    alive: Arc<AtomicBool>,
    #[allow(dead_code)]
    reader_handle: StdMutex<Option<JoinHandle<()>>>,
    pub workspace_root: std::path::PathBuf,
    #[allow(dead_code)]
    pub(crate) capabilities: StdMutex<lsp_types::ServerCapabilities>,
    #[allow(dead_code)]
    child_pid: Option<u32>,
    /// Timeout for the LSP initialize handshake.
    init_timeout: std::time::Duration,
    /// Tracks files opened via textDocument/didOpen to prevent duplicate notifications.
    /// The LSP spec prohibits sending didOpen for an already-open file without an
    /// intervening didClose; some servers (e.g. kotlin-lsp) error on duplicates.
    open_files: StdMutex<HashSet<PathBuf>>,
}

impl LspClient {
    /// Start a language server process and perform the LSP initialize handshake.
    pub async fn start(config: LspServerConfig) -> Result<Self> {
        tracing::info!("Starting LSP server: {} {:?}", config.command, config.args);

        let mut child = Command::new(&config.command)
            .args(&config.args)
            .current_dir(&config.workspace_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to start LSP server: {}", config.command))?;

        let stdin = child.stdin.take().expect("stdin must be piped");
        let stdout = child.stdout.take().expect("stdout must be piped");
        let stderr = child.stderr.take().expect("stderr must be piped");
        let child_pid = child.id();

        let pending: Arc<StdMutex<HashMap<i64, oneshot::Sender<Result<Value>>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));

        // Spawn stderr reader (logs server stderr, doesn't affect protocol)
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line).await {
                    Ok(0) => break,
                    Ok(_) => tracing::debug!(target: "lsp_stderr", "{}", line.trim_end()),
                    Err(_) => break,
                }
            }
        });

        // Spawn stdout reader task — dispatches responses to pending senders
        let pending_clone = pending.clone();
        let alive_clone = alive.clone();
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match transport::read_message(&mut reader).await {
                    Ok(msg) => {
                        if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
                            if msg.get("method").is_some() {
                                // Server-to-client request — log and ignore for now
                                tracing::debug!(
                                    "LSP server request (id={}): {}",
                                    id,
                                    msg["method"]
                                );
                            } else {
                                // Response to our request
                                if let Some(sender) = pending_clone.lock().unwrap().remove(&id) {
                                    if let Some(error) = msg.get("error") {
                                        let err_msg = error["message"]
                                            .as_str()
                                            .unwrap_or("unknown LSP error");
                                        let _ = sender.send(Err(anyhow::anyhow!(
                                            "LSP error (code {}): {}",
                                            error["code"],
                                            err_msg
                                        )));
                                    } else {
                                        let result =
                                            msg.get("result").cloned().unwrap_or(Value::Null);
                                        let _ = sender.send(Ok(result));
                                    }
                                }
                            }
                        } else if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
                            tracing::debug!("LSP notification: {}", method);
                        }
                    }
                    Err(e) => {
                        // EOF or read error — server crashed or exited
                        if alive_clone.load(Ordering::SeqCst) {
                            tracing::warn!("LSP reader error: {}", e);
                        }
                        alive_clone.store(false, Ordering::SeqCst);
                        // Drain pending requests with errors
                        let mut map = pending_clone.lock().unwrap();
                        for (_, sender) in map.drain() {
                            let _ = sender.send(Err(anyhow::anyhow!("LSP server disconnected")));
                        }
                        break;
                    }
                }
            }
            // Wait for child to exit (kill_on_drop will handle cleanup)
            let _ = child.wait().await;
        });
        let init_timeout = config
            .init_timeout
            .unwrap_or(std::time::Duration::from_secs(30));

        let client = Self {
            writer: Mutex::new(stdin),
            next_id: AtomicI64::new(1),
            pending,
            alive,
            reader_handle: StdMutex::new(Some(reader_handle)),
            workspace_root: config.workspace_root.clone(),
            capabilities: StdMutex::new(lsp_types::ServerCapabilities::default()),
            child_pid,
            init_timeout,
            open_files: StdMutex::new(HashSet::new()),
        };

        // Perform the LSP initialize handshake
        client.initialize().await?;

        Ok(client)
    }

    /// Send a JSON-RPC request and await the response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        // Retry on -32800 (RequestCancelled) — disabled for now; re-enable if
        // empirical evidence shows retries help more than they delay failure reporting.
        const RETRY_ON_CANCELLED: bool = false;
        const MAX_RETRIES: usize = 3;
        const RETRY_DELAY_MS: u64 = 300;

        let mut last_err = None;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(RETRY_DELAY_MS * attempt as u64);
                tokio::time::sleep(delay).await;
                tracing::debug!(
                    "LSP request cancelled, retrying {}/{}: {}",
                    attempt,
                    MAX_RETRIES,
                    method
                );
            }
            match self
                .request_with_timeout(method, params.clone(), std::time::Duration::from_secs(30))
                .await
            {
                Ok(result) => return Ok(result),
                Err(e) if RETRY_ON_CANCELLED && e.to_string().contains("code -32800") => {
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap())
    }

    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        if !self.alive.load(Ordering::SeqCst) {
            bail!("LSP server is not running");
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        self.pending.lock().unwrap().insert(id, tx);

        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        {
            let mut writer = self.writer.lock().await;
            if let Err(e) = transport::write_message(&mut *writer, &msg).await {
                self.pending.lock().unwrap().remove(&id);
                return Err(e);
            }
        }

        // Await response with timeout
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => bail!("LSP response channel closed"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                bail!(
                    "LSP request timed out after {}s: {}",
                    timeout.as_secs(),
                    method
                )
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        if !self.alive.load(Ordering::SeqCst) {
            bail!("LSP server is not running");
        }

        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let mut writer = self.writer.lock().await;
        transport::write_message(&mut *writer, &msg).await
    }

    /// Perform the LSP initialize/initialized handshake.
    async fn initialize(&self) -> Result<()> {
        let root_uri = path_to_uri(&self.workspace_root)?;

        let params = lsp_types::InitializeParams {
            process_id: Some(std::process::id()),
            capabilities: lsp_types::ClientCapabilities {
                text_document: Some(lsp_types::TextDocumentClientCapabilities {
                    document_symbol: Some(lsp_types::DocumentSymbolClientCapabilities {
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    references: Some(lsp_types::DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    definition: Some(lsp_types::GotoCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(false),
                    }),
                    rename: Some(lsp_types::RenameClientCapabilities {
                        prepare_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
                uri: root_uri,
                name: self
                    .workspace_root
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            }]),
            ..Default::default()
        };
        // Use the per-server init_timeout. JVM-based servers (Kotlin, Java) need 300s;
        // lightweight servers (rust-analyzer, pyright) use the default 30s.
        let result = self
            .request_with_timeout(
                "initialize",
                serde_json::to_value(params)?,
                self.init_timeout,
            )
            .await?;

        // Parse and store server capabilities
        let init_result: lsp_types::InitializeResult = serde_json::from_value(result)?;
        *self.capabilities.lock().unwrap() = init_result.capabilities;

        // Send initialized notification
        self.notify("initialized", json!({})).await?;

        tracing::info!("LSP server initialized successfully");
        Ok(())
    }

    /// Check if the server process is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Gracefully shut down the LSP server.
    pub async fn shutdown(&self) -> Result<()> {
        if !self.alive.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Send shutdown request
        let _ = self.request("shutdown", Value::Null).await;

        // Send exit notification
        let _ = self.notify("exit", Value::Null).await;

        // Mark as dead
        self.alive.store(false, Ordering::SeqCst);

        // Wait for reader task to finish (with timeout)
        // Extract handle before awaiting to avoid holding MutexGuard across await
        let handle = self.reader_handle.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        Ok(())
    }

    /// Request all symbols in the workspace matching a query string.
    ///
    /// Uses `workspace/symbol` — one round-trip for the whole project, vs
    /// `textDocument/documentSymbol` which requires one request per file.
    /// Returns a flat list (no hierarchy); `container_name` is preserved in
    /// `name_path` when available.
    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<super::SymbolInfo>> {
        let params = lsp_types::WorkspaceSymbolParams {
            query: query.to_string(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let result = self
            .request("workspace/symbol", serde_json::to_value(params)?)
            .await?;

        if result.is_null() {
            return Ok(vec![]);
        }

        let infos: Vec<lsp_types::SymbolInformation> =
            serde_json::from_value(result).context("failed to parse workspace/symbol response")?;

        Ok(infos
            .into_iter()
            .map(|si| {
                let file = uri_to_path(&si.location.uri);
                let name_path = match &si.container_name {
                    Some(container) if !container.is_empty() => {
                        format!("{}/{}", container, si.name)
                    }
                    _ => si.name.clone(),
                };
                super::SymbolInfo {
                    name: si.name,
                    name_path,
                    kind: si.kind.into(),
                    file,
                    start_line: si.location.range.start.line,
                    end_line: si.location.range.end.line,
                    start_col: si.location.range.start.character,
                    children: vec![],
                    detail: None,
                }
            })
            .collect())
    }

    /// Send textDocument/didOpen notification for a file.
    pub async fn did_open(&self, path: &Path, language_id: &str) -> Result<()> {
        // Guard against duplicate didOpen notifications. The LSP spec prohibits sending
        // textDocument/didOpen for a document that is already open (without a prior didClose).
        {
            let mut open_files = self.open_files.lock().unwrap();
            if !open_files.insert(path.to_path_buf()) {
                return Ok(());
            }
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file for didOpen: {:?}", path))?;
        let uri = path_to_uri(path)?;

        self.notify(
            "textDocument/didOpen",
            serde_json::to_value(lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri,
                    language_id: language_id.to_string(),
                    version: 0,
                    text: content,
                },
            })?,
        )
        .await
    }

    /// Request document symbols for a file.
    ///
    /// Returns the hierarchical `DocumentSymbol[]` response parsed into our
    /// `SymbolInfo` tree. Sends `didOpen` first if the file hasn't been opened.
    pub async fn document_symbols(
        &self,
        path: &Path,
        language_id: &str,
    ) -> Result<Vec<super::SymbolInfo>> {
        // Ensure the file is open in the server
        self.did_open(path, language_id).await?;

        let uri = path_to_uri(path)?;
        let params = lsp_types::DocumentSymbolParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let result = self
            .request("textDocument/documentSymbol", serde_json::to_value(params)?)
            .await?;

        // LSP returns either DocumentSymbol[] (hierarchical) or SymbolInformation[] (flat)
        // We prefer hierarchical and convert both to SymbolInfo
        if result.is_null() {
            return Ok(vec![]);
        }

        let file_path = path.to_path_buf();

        // Try hierarchical first
        if let Ok(symbols) =
            serde_json::from_value::<Vec<lsp_types::DocumentSymbol>>(result.clone())
        {
            return Ok(convert_document_symbols(&symbols, &file_path, ""));
        }

        // Fall back to flat SymbolInformation[]
        if let Ok(infos) = serde_json::from_value::<Vec<lsp_types::SymbolInformation>>(result) {
            return Ok(infos
                .iter()
                .map(|si| {
                    let name_path = match &si.container_name {
                        Some(container) if !container.is_empty() => {
                            format!("{}/{}", container, si.name)
                        }
                        _ => si.name.clone(),
                    };
                    super::SymbolInfo {
                        name: si.name.clone(),
                        name_path,
                        kind: si.kind.into(),
                        file: file_path.clone(),
                        start_line: si.location.range.start.line,
                        end_line: si.location.range.end.line,
                        start_col: si.location.range.start.character,
                        children: vec![],
                        detail: None,
                    }
                })
                .collect());
        }

        Ok(vec![])
    }

    /// Request references for a symbol at a given position.
    pub async fn references(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> Result<Vec<lsp_types::Location>> {
        self.did_open(path, language_id).await?;
        let uri = path_to_uri(path)?;
        let params = lsp_types::ReferenceParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position: lsp_types::Position {
                    line,
                    character: col,
                },
            },
            context: lsp_types::ReferenceContext {
                include_declaration: true,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let result = self
            .request("textDocument/references", serde_json::to_value(params)?)
            .await?;

        if result.is_null() {
            return Ok(vec![]);
        }

        Ok(serde_json::from_value(result)?)
    }

    /// Request definition location for a symbol at a given position.
    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> Result<Vec<lsp_types::Location>> {
        self.did_open(path, language_id).await?;
        let uri = path_to_uri(path)?;
        let params = lsp_types::GotoDefinitionParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position: lsp_types::Position {
                    line,
                    character: col,
                },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let result = self
            .request("textDocument/definition", serde_json::to_value(params)?)
            .await?;

        if result.is_null() {
            return Ok(vec![]);
        }

        // Can return Location | Location[] | LocationLink[]
        if let Ok(loc) = serde_json::from_value::<lsp_types::Location>(result.clone()) {
            return Ok(vec![loc]);
        }
        if let Ok(locs) = serde_json::from_value::<Vec<lsp_types::Location>>(result.clone()) {
            return Ok(locs);
        }
        if let Ok(links) = serde_json::from_value::<Vec<lsp_types::LocationLink>>(result) {
            return Ok(links
                .into_iter()
                .map(|l| lsp_types::Location {
                    uri: l.target_uri,
                    range: l.target_selection_range,
                })
                .collect());
        }

        Ok(vec![])
    }

    /// Send textDocument/hover and return the hover contents as a string.
    pub async fn hover(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> Result<Option<String>> {
        self.did_open(path, language_id).await?;
        let uri = path_to_uri(path)?;
        let params = lsp_types::HoverParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position: lsp_types::Position {
                    line,
                    character: col,
                },
            },
            work_done_progress_params: Default::default(),
        };

        let result = self
            .request("textDocument/hover", serde_json::to_value(params)?)
            .await?;

        if result.is_null() {
            return Ok(None);
        }

        let hover: lsp_types::Hover = serde_json::from_value(result)?;

        let text = match hover.contents {
            lsp_types::HoverContents::Scalar(ms) => match ms {
                lsp_types::MarkedString::String(s) => s,
                lsp_types::MarkedString::LanguageString(ls) => ls.value,
            },
            lsp_types::HoverContents::Array(arr) => arr
                .into_iter()
                .map(|ms| match ms {
                    lsp_types::MarkedString::String(s) => s,
                    lsp_types::MarkedString::LanguageString(ls) => ls.value,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            lsp_types::HoverContents::Markup(mc) => mc.value,
        };

        Ok(Some(text))
    }

    /// Request a rename across the workspace.
    pub async fn rename(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
        language_id: &str,
    ) -> Result<lsp_types::WorkspaceEdit> {
        self.did_open(path, language_id).await?;
        let uri = path_to_uri(path)?;
        let params = lsp_types::RenameParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position: lsp_types::Position {
                    line,
                    character: col,
                },
            },
            new_name: new_name.to_string(),
            work_done_progress_params: Default::default(),
        };

        let result = self
            .request("textDocument/rename", serde_json::to_value(params)?)
            .await?;

        Ok(serde_json::from_value(result)?)
    }

    /// Send textDocument/didClose notification for a file.
    pub async fn did_close(&self, path: &Path) -> Result<()> {
        self.open_files.lock().unwrap().remove(path);

        let uri = path_to_uri(path)?;

        self.notify(
            "textDocument/didClose",
            serde_json::to_value(lsp_types::DidCloseTextDocumentParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
            })?,
        )
        .await
    }

    /// Notify the LSP server that a file it has open was modified on disk by an external tool.
    /// No-op if the file is not currently open in this server.
    pub async fn did_change(&self, path: &Path) -> Result<()> {
        if !self.open_files.lock().unwrap().contains(path) {
            return Ok(());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file for didChange: {:?}", path))?;
        let uri = path_to_uri(path)?;
        self.notify(
            "textDocument/didChange",
            serde_json::to_value(lsp_types::DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier { uri, version: 1 },
                content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: content,
                }],
            })?,
        )
        .await
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Abort the reader task
        if let Ok(mut guard) = self.reader_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
        // Kill the child process as a safety net.
        // The graceful shutdown path (shutdown_all -> shutdown) sends LSP
        // shutdown/exit first.  This ensures the process dies even if the
        // graceful path was skipped (e.g., panic, abrupt exit).
        if let Some(pid) = self.child_pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }
}

#[async_trait::async_trait]
impl crate::lsp::ops::LspClientOps for LspClient {
    async fn document_symbols(
        &self,
        path: &std::path::Path,
        language_id: &str,
    ) -> anyhow::Result<Vec<crate::lsp::SymbolInfo>> {
        LspClient::document_symbols(self, path, language_id).await
    }

    async fn workspace_symbols(&self, query: &str) -> anyhow::Result<Vec<crate::lsp::SymbolInfo>> {
        LspClient::workspace_symbols(self, query).await
    }

    async fn references(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        LspClient::references(self, path, line, col, language_id).await
    }

    async fn goto_definition(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        LspClient::goto_definition(self, path, line, col, language_id).await
    }

    async fn hover(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        language_id: &str,
    ) -> anyhow::Result<Option<String>> {
        LspClient::hover(self, path, line, col, language_id).await
    }

    async fn rename(
        &self,
        path: &std::path::Path,
        line: u32,
        col: u32,
        new_name: &str,
        language_id: &str,
    ) -> anyhow::Result<lsp_types::WorkspaceEdit> {
        LspClient::rename(self, path, line, col, new_name, language_id).await
    }

    async fn did_change(&self, path: &std::path::Path) -> anyhow::Result<()> {
        LspClient::did_change(self, path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Create a minimal Cargo project for testing with rust-analyzer.
    fn create_test_cargo_project(dir: &Path) {
        std::fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/main.rs"),
            r#"fn main() {
    println!("hello");
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}

struct Point {
    x: f64,
    y: f64,
}
"#,
        )
        .unwrap();
    }

    fn rust_analyzer_available() -> bool {
        std::process::Command::new("rust-analyzer")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn client_initializes_with_rust_analyzer() {
        if !rust_analyzer_available() {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempdir().unwrap();
        create_test_cargo_project(dir.path());

        let config = LspServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            workspace_root: dir.path().to_path_buf(),
            init_timeout: None,
        };

        let client = LspClient::start(config).await.unwrap();
        assert!(client.is_alive());

        // Verify we got capabilities
        let caps = client.capabilities.lock().unwrap();
        // rust-analyzer should support document symbols
        assert!(caps.document_symbol_provider.is_some());
        drop(caps);

        client.shutdown().await.unwrap();
        assert!(!client.is_alive());
    }

    #[tokio::test]
    async fn client_detects_missing_server() {
        let dir = tempdir().unwrap();
        let config = LspServerConfig {
            command: "nonexistent-lsp-server-xyz".into(),
            args: vec![],
            workspace_root: dir.path().to_path_buf(),
            init_timeout: None,
        };

        let result = LspClient::start(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn workspace_symbols_returns_project_symbols() {
        if !rust_analyzer_available() {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempdir().unwrap();
        create_test_cargo_project(dir.path());

        let config = LspServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            workspace_root: dir.path().to_path_buf(),
            init_timeout: None,
        };

        let client = LspClient::start(config).await.unwrap();

        // Open a file to trigger rust-analyzer background indexing.
        client
            .did_open(&dir.path().join("src/main.rs"), "rust")
            .await
            .unwrap();

        // rust-analyzer indexes in the background after initialize; retry until
        // workspace/symbol returns results (typically < 2s for a minimal project).
        let mut symbols = vec![];
        for _ in 0..10 {
            symbols = client.workspace_symbols("add").await.unwrap();
            if !symbols.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        assert!(
            !symbols.is_empty(),
            "workspace/symbol 'add' should return results within 5s of indexing"
        );
        assert!(
            symbols.iter().any(|s| s.name == "add"),
            "should find the 'add' function, got: {:?}",
            symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );

        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn client_did_open_with_rust_analyzer() {
        if !rust_analyzer_available() {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempdir().unwrap();
        create_test_cargo_project(dir.path());

        let config = LspServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            workspace_root: dir.path().to_path_buf(),
            init_timeout: None,
        };

        let client = LspClient::start(config).await.unwrap();
        // Open the main file
        client
            .did_open(&dir.path().join("src/main.rs"), "rust")
            .await
            .unwrap();

        client.shutdown().await.unwrap();
    }

    #[test]
    fn convert_document_symbols_uses_selection_range() {
        use lsp_types::{DocumentSymbol, Position, Range, SymbolKind as LspSymbolKind};

        let symbols = vec![DocumentSymbol {
            name: "my_func".to_string(),
            detail: None,
            kind: LspSymbolKind::FUNCTION,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: Range {
                start: Position {
                    line: 5,
                    character: 0,
                },
                end: Position {
                    line: 10,
                    character: 1,
                },
            },
            selection_range: Range {
                start: Position {
                    line: 8,
                    character: 4,
                },
                end: Position {
                    line: 8,
                    character: 11,
                },
            },
            children: None,
        }];

        let path = std::env::temp_dir().join("test.rs");
        let result = convert_document_symbols(&symbols, &path, "");

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].start_line, 8,
            "start_line should use selection_range"
        );
        assert_eq!(
            result[0].start_col, 4,
            "start_col should use selection_range"
        );
        assert_eq!(
            result[0].end_line, 10,
            "end_line should use range for body extent"
        );
    }

    #[test]
    fn convert_document_symbols_captures_detail() {
        use lsp_types::{DocumentSymbol, Position, Range, SymbolKind as LspSymbolKind};

        let symbols = vec![DocumentSymbol {
            name: "my_func".to_string(),
            detail: Some("(x: i32) -> bool".to_string()),
            kind: LspSymbolKind::FUNCTION,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: 5, character: 1 },
            },
            selection_range: Range {
                start: Position { line: 0, character: 3 },
                end: Position { line: 0, character: 10 },
            },
            children: None,
        }];

        let path = std::env::temp_dir().join("test_detail_capture.rs");
        let result = convert_document_symbols(&symbols, &path, "");

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].detail,
            Some("(x: i32) -> bool".to_string()),
            "detail should be captured from DocumentSymbol"
        );
    }

    #[test]
    fn convert_document_symbols_collapses_empty_detail() {
        use lsp_types::{DocumentSymbol, Position, Range, SymbolKind as LspSymbolKind};

        let symbols = vec![DocumentSymbol {
            name: "my_func".to_string(),
            detail: Some("".to_string()),
            kind: LspSymbolKind::FUNCTION,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: 5, character: 1 },
            },
            selection_range: Range {
                start: Position { line: 0, character: 3 },
                end: Position { line: 0, character: 10 },
            },
            children: None,
        }];

        let path = std::env::temp_dir().join("test_detail_empty.rs");
        let result = convert_document_symbols(&symbols, &path, "");

        assert_eq!(
            result[0].detail,
            None,
            "empty string detail should collapse to None"
        );
    }

    #[test]
    fn flat_symbol_information_builds_name_path_from_container() {
        use crate::lsp::SymbolInfo;
        use lsp_types::{
            Location, Position, Range, SymbolInformation, SymbolKind as LspSymbolKind, Uri,
        };

        let uri: Uri = if cfg!(windows) {
            "file:///C:/temp/test.rb".parse().unwrap()
        } else {
            "file:///tmp/test.rb".parse().unwrap()
        };
        let infos = vec![
            SymbolInformation {
                name: "MyClass".to_string(),
                kind: LspSymbolKind::CLASS,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                location: Location {
                    uri: uri.clone(),
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: 20,
                            character: 3,
                        },
                    },
                },
                container_name: None,
            },
            SymbolInformation {
                name: "my_method".to_string(),
                kind: LspSymbolKind::METHOD,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                location: Location {
                    uri: uri.clone(),
                    range: Range {
                        start: Position {
                            line: 5,
                            character: 2,
                        },
                        end: Position {
                            line: 10,
                            character: 5,
                        },
                    },
                },
                container_name: Some("MyClass".to_string()),
            },
        ];

        // Simulate what document_symbols does with flat format
        let file_path = std::env::temp_dir().join("test.rb");
        // Current code just does name_path: si.name.clone() — this test verifies the fix
        let result: Vec<SymbolInfo> = infos
            .iter()
            .map(|si| {
                let name_path = match &si.container_name {
                    Some(container) if !container.is_empty() => {
                        format!("{}/{}", container, si.name)
                    }
                    _ => si.name.clone(),
                };
                SymbolInfo {
                    name: si.name.clone(),
                    name_path,
                    kind: si.kind.into(),
                    file: file_path.clone(),
                    start_line: si.location.range.start.line,
                    end_line: si.location.range.end.line,
                    start_col: si.location.range.start.character,
                    children: vec![],
                    detail: None,
                }
            })
            .collect();

        assert_eq!(result[0].name_path, "MyClass");
        assert_eq!(result[1].name_path, "MyClass/my_method");
    }

    #[test]
    fn path_to_uri_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "").unwrap();

        let uri = path_to_uri(&file).unwrap();
        let uri_str = uri.as_str();
        assert!(
            uri_str.starts_with("file:///"),
            "URI should start with file:///: {}",
            uri_str
        );

        let back = uri_to_path(&uri);
        assert_eq!(back, file, "roundtrip should preserve the path");
    }

    #[tokio::test]
    async fn drop_kills_child_process() {
        if !rust_analyzer_available() {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }
        let dir = tempdir().unwrap();
        create_test_cargo_project(dir.path());
        let config = LspServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            workspace_root: dir.path().to_path_buf(),
            init_timeout: None,
        };
        let client = LspClient::start(config).await.unwrap();
        let pid = client.child_pid.unwrap();

        // Verify child is alive
        let alive = unsafe { libc::kill(pid as i32, 0) };
        assert_eq!(alive, 0, "child should be alive before drop");

        // Drop the client
        drop(client);

        // Give the process a moment to die
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Verify child is dead
        let dead = unsafe { libc::kill(pid as i32, 0) };
        assert_ne!(dead, 0, "child should be dead after drop");
    }

    /// Reproduce the stale-position bug: after editing a file on disk without sending
    /// didChange, the LSP returns positions from the old content. did_change fixes it.
    #[tokio::test]
    async fn did_change_refreshes_stale_symbol_positions() {
        if !rust_analyzer_available() {
            eprintln!("Skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempdir().unwrap();
        create_test_cargo_project(dir.path());
        let main_rs = dir.path().join("src/main.rs");

        let config = LspServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            workspace_root: dir.path().to_path_buf(),
            init_timeout: None,
        };
        let client = LspClient::start(config).await.unwrap();

        // Step 1: query symbols — fn add is at line 4 (0-indexed) in the original file.
        let syms = client.document_symbols(&main_rs, "rust").await.unwrap();
        let add_before = syms
            .iter()
            .find(|s| s.name == "add")
            .expect("fn add not found");
        let original_line = add_before.start_line;

        // Step 2: prepend 3 blank lines on disk — shifts fn add to line 7.
        let original = std::fs::read_to_string(&main_rs).unwrap();
        std::fs::write(&main_rs, format!("\n\n\n{}", original)).unwrap();

        // Step 3: query again WITHOUT did_change — LSP returns stale positions.
        let syms_stale = client.document_symbols(&main_rs, "rust").await.unwrap();
        let add_stale = syms_stale
            .iter()
            .find(|s| s.name == "add")
            .expect("fn add not found");
        assert_eq!(
            add_stale.start_line, original_line,
            "without did_change, LSP should still return the old (stale) line number"
        );

        // Step 4: notify the LSP about the disk change.
        client.did_change(&main_rs).await.unwrap();

        // Step 5: query again — LSP should now return the shifted position.
        let syms_fresh = client.document_symbols(&main_rs, "rust").await.unwrap();
        let add_fresh = syms_fresh
            .iter()
            .find(|s| s.name == "add")
            .expect("fn add not found");
        assert_eq!(
            add_fresh.start_line,
            original_line + 3,
            "after did_change, LSP should return the updated line number (shifted by 3)"
        );

        client.shutdown().await.unwrap();
    }
}
