//! Async LSP client: spawns a language server subprocess and communicates
//! via JSON-RPC over stdio.

use std::collections::HashMap;
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
fn uri_to_path(uri: &lsp_types::Uri) -> PathBuf {
    PathBuf::from(uri.path().as_str())
}

/// Convert a filesystem path to an LSP `file://` URI.
fn path_to_uri(path: &Path) -> Result<lsp_types::Uri> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let uri_str = format!("file://{}", abs.display());
    uri_str
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid URI '{}': {}", uri_str, e))
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
                start_line: ds.range.start.line,
                end_line: ds.range.end.line,
                start_col: ds.range.start.character,
                children,
            }
        })
        .collect()
}

/// Configuration for launching a language server.
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    /// Executable to launch (e.g. "rust-analyzer", "pyright-langserver")
    pub command: String,
    /// Arguments passed to the executable
    pub args: Vec<String>,
    /// Working directory (usually the project root)
    pub workspace_root: PathBuf,
}

/// A running LSP client session connected to a language server process.
pub struct LspClient {
    /// Writer to the server's stdin
    writer: Mutex<ChildStdin>,
    /// Monotonically increasing request ID
    next_id: AtomicI64,
    /// Pending request senders, keyed by request ID
    pending: Arc<StdMutex<HashMap<i64, oneshot::Sender<Result<Value>>>>>,
    /// Whether the server process is still alive
    alive: Arc<AtomicBool>,
    /// Background reader task
    reader_handle: StdMutex<Option<JoinHandle<()>>>,
    /// The workspace root for this server instance
    pub workspace_root: PathBuf,
    /// Server capabilities from initialization
    pub capabilities: StdMutex<lsp_types::ServerCapabilities>,
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

        let client = Self {
            writer: Mutex::new(stdin),
            next_id: AtomicI64::new(1),
            pending,
            alive,
            reader_handle: StdMutex::new(Some(reader_handle)),
            workspace_root: config.workspace_root.clone(),
            capabilities: StdMutex::new(lsp_types::ServerCapabilities::default()),
        };

        // Perform the LSP initialize handshake
        client.initialize().await?;

        Ok(client)
    }

    /// Send a JSON-RPC request and await the response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
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
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => bail!("LSP response channel closed"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                bail!("LSP request timed out after 30s: {}", method)
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

        let result = self
            .request("initialize", serde_json::to_value(params)?)
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

        let infos: Vec<lsp_types::SymbolInformation> = serde_json::from_value(result)
            .context("failed to parse workspace/symbol response")?;

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
                }
            })
            .collect())
    }

    /// Send textDocument/didOpen notification for a file.
    pub async fn did_open(&self, path: &Path, language_id: &str) -> Result<()> {
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
                .map(|si| super::SymbolInfo {
                    name: si.name.clone(),
                    name_path: si.name.clone(),
                    kind: si.kind.into(),
                    file: file_path.clone(),
                    start_line: si.location.range.start.line,
                    end_line: si.location.range.end.line,
                    start_col: si.location.range.start.character,
                    children: vec![],
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
        let uri = path_to_uri(path)?;

        self.notify(
            "textDocument/didClose",
            serde_json::to_value(lsp_types::DidCloseTextDocumentParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
            })?,
        )
        .await
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Abort the reader task — kill_on_drop on the child process
        // ensures the server process is cleaned up too.
        if let Ok(mut guard) = self.reader_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
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
        };

        let client = LspClient::start(config).await.unwrap();
        // Open the main file
        client
            .did_open(&dir.path().join("src/main.rs"), "rust")
            .await
            .unwrap();

        client.shutdown().await.unwrap();
    }
}
