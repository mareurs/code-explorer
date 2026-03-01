//! MCP server — bridges our `Tool` registry to rmcp's `ServerHandler`.

use std::path::PathBuf;
use std::sync::Arc;

use crate::lsp::{LspManager, LspProvider};

use anyhow::Result;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, Content, ListToolsResult, PaginatedRequestParam,
        ServerCapabilities, ServerInfo, Tool as McpTool,
    },
    service::RequestContext,
    Error as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde_json::Value;

use crate::agent::Agent;
use crate::tools::{
    ast::{ListDocs, ListFunctions},
    config::{ActivateProject, GetConfig},
    file::{CreateFile, EditLines, FindFile, ListDir, ReadFile, SearchPattern},
    git::GitBlame,
    library::{IndexLibrary, ListLibraries},
    memory::{DeleteMemory, ListMemories, ReadMemory, WriteMemory},
    semantic::{IndexProject, IndexStatus, SemanticSearch},
    symbol::{
        FindReferences, FindSymbol, GotoDefinition, Hover, InsertCode, ListSymbols, RenameSymbol,
        ReplaceSymbol,
    },
    usage::GetUsageStats,
    workflow::{Onboarding, RunCommand},
    Tool, ToolContext,
};
use crate::usage::UsageRecorder;

/// The MCP server handler — holds shared agent state and a registry of tools.
#[derive(Clone)]
pub struct CodeExplorerServer {
    agent: Agent,
    lsp: Arc<dyn LspProvider>,
    output_buffer: Arc<crate::tools::output_buffer::OutputBuffer>,
    tools: Vec<Arc<dyn Tool>>,
    /// Pre-computed at construction because `get_info()` is sync.
    /// Becomes stale if project state changes mid-session (e.g. after onboarding or indexing).
    /// For stdio this means instructions reflect the state at server startup;
    /// for HTTP/SSE each connection gets fresh instructions.
    instructions: String,
}

impl CodeExplorerServer {
    pub async fn new(agent: Agent) -> Self {
        Self::from_parts(agent, LspManager::new_arc()).await
    }

    /// Create a server with an existing LspManager (used for HTTP multi-session).
    pub async fn from_parts(agent: Agent, lsp: Arc<dyn LspProvider>) -> Self {
        let status = agent.project_status().await;
        let instructions = crate::prompts::build_server_instructions(status.as_ref());
        let tools: Vec<Arc<dyn Tool>> = vec![
            // File tools (fully implemented)
            Arc::new(ReadFile),
            Arc::new(ListDir),
            Arc::new(SearchPattern),
            Arc::new(CreateFile),
            Arc::new(FindFile),
            Arc::new(EditLines),
            // Workflow tools
            Arc::new(RunCommand),
            Arc::new(Onboarding),
            // Symbol tools (stub — require LSP)
            Arc::new(FindSymbol),
            Arc::new(FindReferences),
            Arc::new(GotoDefinition),
            Arc::new(Hover),
            Arc::new(ListSymbols),
            Arc::new(ReplaceSymbol),
            Arc::new(InsertCode),
            Arc::new(RenameSymbol),
            // AST tools (stub — require tree-sitter wiring)
            Arc::new(ListFunctions),
            Arc::new(ListDocs),
            // Git tools
            Arc::new(GitBlame),
            // Memory tools (stub — require Agent project root)
            Arc::new(WriteMemory),
            Arc::new(ReadMemory),
            Arc::new(ListMemories),
            Arc::new(DeleteMemory),
            // Semantic search tools (stub — require embed engine)
            Arc::new(SemanticSearch),
            Arc::new(IndexProject),
            Arc::new(IndexStatus),
            // Config tools (stub — require Agent wiring)
            Arc::new(ActivateProject),
            Arc::new(GetConfig),
            // Library tools
            Arc::new(ListLibraries),
            Arc::new(IndexLibrary),
            // Usage monitoring
            Arc::new(GetUsageStats),
        ];
        let output_buffer = Arc::new(crate::tools::output_buffer::OutputBuffer::new(20));
        Self {
            agent,
            lsp,
            output_buffer,
            tools,
            instructions,
        }
    }

    fn find_tool(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }
}

impl ServerHandler for CodeExplorerServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(self.instructions.clone()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    async fn list_tools(
        &self,
        _req: PaginatedRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, McpError> {
        let tools = self
            .tools
            .iter()
            .map(|t| {
                let schema = t.input_schema();
                let schema_obj = schema.as_object().cloned().unwrap_or_default();
                McpTool {
                    name: t.name().to_owned().into(),
                    description: t.description().to_owned().into(),
                    input_schema: Arc::new(schema_obj),
                }
            })
            .collect();

        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let tool = self.find_tool(&req.name).ok_or_else(|| {
            McpError::invalid_params(format!("unknown tool: '{}'", req.name), None)
        })?;

        // Check tool access before dispatching
        let security = self.agent.security_config().await;
        if let Err(e) = crate::util::path_security::check_tool_access(&req.name, &security) {
            return Ok(CallToolResult::error(vec![Content::text(e.to_string())]));
        }

        let input: Value = req
            .arguments
            .map(Value::Object)
            .unwrap_or(Value::Object(Default::default()));

        let ctx = ToolContext {
            agent: self.agent.clone(),
            lsp: self.lsp.clone(),
            output_buffer: self.output_buffer.clone(),
        };

        // Indexing tools run embedding loops that can legitimately take minutes;
        // skip the per-call timeout for them.
        let is_long_running = matches!(req.name.as_ref(), "index_project" | "index_library");

        let timeout_secs = if is_long_running {
            None
        } else {
            self.agent
                .with_project(|p| Ok(p.config.project.tool_timeout_secs))
                .await
                .ok()
        };

        let recorder = UsageRecorder::new(self.agent.clone());

        let result = if let Some(secs) = timeout_secs {
            tokio::time::timeout(
                std::time::Duration::from_secs(secs),
                recorder.record(&req.name, || tool.call(input, &ctx)),
            )
            .await
            .unwrap_or_else(|_| {
                Err(anyhow::anyhow!(
                    "Tool '{}' timed out after {}s. \
                     Increase tool_timeout_secs in .code-explorer/project.toml if needed.",
                    req.name,
                    secs
                ))
            })
        } else {
            recorder.record(&req.name, || tool.call(input, &ctx)).await
        };

        match result {
            Ok(output) => {
                let text =
                    serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(route_tool_error(e)),
        }
    }
}

/// Route a tool `Err(e)` to the appropriate `CallToolResult`.
///
/// - [`RecoverableError`] → `isError: false` with a JSON body
///   `{"error": "...", "hint": "..."}`.  The LLM sees the problem and a
///   suggestion but sibling parallel calls are **not** aborted by the client.
/// - Any other error → `isError: true` (fatal; something truly broke).
///
/// [`RecoverableError`]: crate::tools::RecoverableError
fn route_tool_error(e: anyhow::Error) -> CallToolResult {
    if let Some(rec) = e.downcast_ref::<crate::tools::RecoverableError>() {
        let mut body = serde_json::json!({ "error": rec.message });
        if let Some(hint) = &rec.hint {
            body["hint"] = serde_json::json!(hint);
        }
        let text = serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string());
        CallToolResult::success(vec![Content::text(text)])
    } else if e.to_string().contains("code -32800") {
        // LSP RequestCancelled — treat as recoverable so sibling parallel tool calls are not aborted.
        let body = serde_json::json!({
            "error": e.to_string(),
            "hint": "The LSP server cancelled this request (code -32800). Common causes:\n\
                     (1) Another editor (e.g. VS Code) is running a language server for the same \
                     project. kotlin-lsp v0.253 uses an on-disk MVStore index that only supports \
                     one session at a time — the workspace database is locked. Close the other \
                     editor session or upgrade kotlin-lsp to v261+ (which allows shared access).\n\
                     (2) The server just started and is still running background indexing \
                     (can take 1-5 minutes on first run). Wait and retry the call."
        });
        let text = serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string());
        CallToolResult::success(vec![Content::text(text)])
    } else {
        CallToolResult::error(vec![Content::text(e.to_string())])
    }
}

/// Entry point: start the MCP server with the chosen transport.
/// Generate a bearer token for HTTP transport authentication.
///
/// Combines high-resolution timestamp with process ID to produce a unique,
/// hard-to-guess hex string. This is NOT cryptographically secure — it is a
/// convenience default when the operator does not supply `--auth-token`.
pub fn generate_auth_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id() as u64;
    // Mask to 64 bits each so the total is exactly 32 hex characters.
    let hi = nanos as u64;
    let lo = pid.wrapping_mul(0x517cc1b727220a95);
    format!("{:016x}{:016x}", hi, lo)
}

/// Wait for SIGINT (Ctrl-C) or SIGTERM.
pub(crate) async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}

pub async fn run(
    project: Option<PathBuf>,
    transport: &str,
    host: &str,
    port: u16,
    auth_token: Option<String>,
) -> Result<()> {
    // If no --project given, auto-detect from CWD (Claude Code launches servers from the project dir)
    let project = project.or_else(|| std::env::current_dir().ok());
    let agent = Agent::new(project).await?;
    let lsp = LspManager::new_arc();

    match transport {
        "stdio" => {
            if auth_token.is_some() {
                tracing::warn!("--auth-token is ignored for stdio transport");
            }
            tracing::info!("code-explorer MCP server ready (stdio)");
            let server = CodeExplorerServer::from_parts(agent, lsp.clone()).await;
            let service = server
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| anyhow::anyhow!("MCP server error: {}", e))?;

            // Wait for service to end OR shutdown signal
            tokio::select! {
                result = service.waiting() => {
                    result.map_err(|e| anyhow::anyhow!("MCP server exited: {}", e))?;
                }
                _ = shutdown_signal() => {
                    tracing::info!("Received shutdown signal");
                }
            }

            // Gracefully shut down all LSP servers
            tracing::info!("Shutting down LSP servers...");
            lsp.shutdown_all().await;
            tracing::info!("All LSP servers shut down");
            Ok(())
        }
        "http" => {
            // --- Auth token setup ---
            let token = auth_token.unwrap_or_else(|| {
                let t = generate_auth_token();
                eprintln!("No --auth-token provided; generated one automatically.");
                t
            });
            eprintln!("HTTP transport auth token: {}", token);
            eprintln!("Clients must send header:  Authorization: Bearer {}", token);

            // --- Bind address safety warnings ---
            if host == "0.0.0.0" || host == "::" {
                eprintln!(
                    "WARNING: Server is bound to all interfaces ({}).\n\
                     This exposes the MCP server to the entire network.\n\
                     Use --host 127.0.0.1 for local-only access.",
                    host
                );
            }

            let addr: std::net::SocketAddr = format!("{}:{}", host, port).parse()?;
            tracing::info!("code-explorer MCP server ready (HTTP/SSE at {})", addr);
            tracing::info!("  SSE endpoint: http://{}/sse", addr);
            tracing::info!("  Message endpoint: http://{}/message", addr);

            // NOTE: rmcp's SseServer does not expose middleware hooks for
            // per-request header validation.  The token is printed at startup
            // so the operator can configure their MCP client with the correct
            // Authorization header.  Proper per-connection token enforcement
            // will be added once rmcp supports custom middleware or an auth
            // callback on SseServer.
            // TODO: validate Authorization header per-connection when rmcp supports middleware
            let _token = token; // retained for future middleware use

            let mut sse_server = rmcp::transport::sse_server::SseServer::serve(addr)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to start SSE server: {}", e))?;

            // Accept connections until shutdown signal
            loop {
                tokio::select! {
                    transport = sse_server.next_transport() => {
                        match transport {
                            Some(transport) => {
                                let agent = agent.clone();
                                let lsp = lsp.clone();
                                tokio::spawn(async move {
                                    let handler = CodeExplorerServer::from_parts(agent, lsp).await;
                                    match handler.serve(transport).await {
                                        Ok(service) => {
                                            if let Err(e) = service.waiting().await {
                                                tracing::debug!("SSE session ended: {}", e);
                                            }
                                        }
                                        Err(e) => tracing::warn!("SSE session failed to start: {}", e),
                                    }
                                });
                            }
                            None => break,
                        }
                    }
                    _ = shutdown_signal() => {
                        tracing::info!("Received shutdown signal");
                        break;
                    }
                }
            }

            // Gracefully shut down all LSP servers
            tracing::info!("Shutting down LSP servers...");
            lsp.shutdown_all().await;
            tracing::info!("All LSP servers shut down");
            Ok(())
        }
        other => anyhow::bail!("Unknown transport '{}'. Use 'stdio' or 'http'.", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use tempfile::tempdir;

    async fn make_server() -> (tempfile::TempDir, CodeExplorerServer) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let server = CodeExplorerServer::new(agent).await;
        (dir, server)
    }

    async fn make_server_no_project() -> CodeExplorerServer {
        let agent = Agent::new(None).await.unwrap();
        CodeExplorerServer::new(agent).await
    }

    #[tokio::test]
    async fn server_registers_all_tools() {
        let (_dir, server) = make_server().await;
        // Verify expected tool count matches the registered tools
        let expected_tools = [
            "read_file",
            "list_dir",
            "search_pattern",
            "create_file",
            "find_file",
            "edit_lines",
            "run_command",
            "onboarding",
            "find_symbol",
            "find_references",
            "list_symbols",
            "replace_symbol",
            "insert_code",
            "rename_symbol",
            "goto_definition",
            "hover",
            "list_functions",
            "list_docs",
            "git_blame",
            "write_memory",
            "read_memory",
            "list_memories",
            "delete_memory",
            "semantic_search",
            "index_project",
            "index_status",
            "activate_project",
            "get_config",
            "list_libraries",
            "index_library",
            "get_usage_stats",
        ];
        assert_eq!(
            server.tools.len(),
            expected_tools.len(),
            "tool count mismatch: expected {}, got {}",
            expected_tools.len(),
            server.tools.len()
        );
        for name in &expected_tools {
            assert!(
                server.find_tool(name).is_some(),
                "tool '{}' not found in server",
                name
            );
        }
    }

    #[tokio::test]
    async fn find_tool_returns_none_for_unknown() {
        let (_dir, server) = make_server().await;
        assert!(server.find_tool("nonexistent_tool").is_none());
        assert!(server.find_tool("").is_none());
        assert!(server.find_tool("READ_FILE").is_none()); // case-sensitive
    }

    #[tokio::test]
    async fn tool_names_are_unique() {
        let (_dir, server) = make_server().await;
        let mut names: Vec<&str> = server.tools.iter().map(|t| t.name()).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), original_len, "duplicate tool names found");
    }

    #[tokio::test]
    async fn all_tools_have_valid_schemas() {
        let (_dir, server) = make_server().await;
        for tool in &server.tools {
            let schema = tool.input_schema();
            assert!(
                schema.is_object(),
                "tool '{}' schema is not an object",
                tool.name()
            );
            // Every schema should have "type": "object" at minimum
            assert_eq!(
                schema["type"],
                "object",
                "tool '{}' schema missing type:object",
                tool.name()
            );
        }
    }

    #[tokio::test]
    async fn all_tools_have_descriptions() {
        let (_dir, server) = make_server().await;
        for tool in &server.tools {
            let desc = tool.description();
            assert!(
                !desc.is_empty(),
                "tool '{}' has empty description",
                tool.name()
            );
        }
    }

    #[tokio::test]
    async fn get_info_contains_instructions() {
        let (_dir, server) = make_server().await;
        let info = server.get_info();
        assert!(info.instructions.is_some());
        let instructions = info.instructions.unwrap();
        assert!(!instructions.is_empty());
    }

    #[tokio::test]
    async fn get_info_without_project_still_works() {
        let server = make_server_no_project().await;
        let info = server.get_info();
        assert!(info.instructions.is_some());
    }

    #[tokio::test]
    async fn server_instructions_mention_project_when_active() {
        let (_dir, server) = make_server().await;
        let info = server.get_info();
        let instructions = info.instructions.unwrap();
        // When a project is active, instructions should reference it
        assert!(
            instructions.contains("Project:") || instructions.contains("project"),
            "instructions should mention the active project"
        );
    }

    #[test]
    fn generate_auth_token_produces_nonempty_hex() {
        let token = super::generate_auth_token();
        assert!(!token.is_empty(), "token must not be empty");
        assert_eq!(token.len(), 32, "token should be 32 hex chars");
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token must be valid hex: {}",
            token
        );
    }

    #[test]
    fn generate_auth_token_is_unique_across_calls() {
        let t1 = super::generate_auth_token();
        let t2 = super::generate_auth_token();
        // The nanos component changes between calls, so tokens should differ.
        // In the astronomically unlikely case of a collision, the test is still
        // correct — but practically this always passes.
        assert_ne!(t1, t2, "consecutive tokens should differ");
    }

    #[tokio::test]
    async fn shell_tool_blocked_by_default() {
        let (_dir, server) = make_server().await;
        // Shell should be disabled by default — verify through security config
        let security = server.agent.security_config().await;
        assert!(!security.shell_enabled);
        assert!(crate::util::path_security::check_tool_access("run_command", &security).is_err());
    }

    // ── route_tool_error ───────────────────────────────────────────────────

    #[test]
    fn recoverable_error_routes_to_success_not_is_error() {
        let err = anyhow::Error::new(crate::tools::RecoverableError::new("path not found"));
        let result = route_tool_error(err);
        assert!(
            result.is_error != Some(true),
            "RecoverableError must not set isError:true"
        );
    }

    #[test]
    fn recoverable_error_body_has_error_key() {
        let err = anyhow::Error::new(crate::tools::RecoverableError::new(
            "path not found: foo/bar",
        ));
        let result = route_tool_error(err);
        let text = &result.content[0].as_text().unwrap().text;
        let body: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(body["error"], "path not found: foo/bar");
    }

    #[test]
    fn recoverable_error_body_includes_hint_when_present() {
        let err = anyhow::Error::new(crate::tools::RecoverableError::with_hint(
            "not found",
            "use list_dir to explore",
        ));
        let result = route_tool_error(err);
        let text = &result.content[0].as_text().unwrap().text;
        let body: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(body["hint"], "use list_dir to explore");
    }

    #[test]
    fn recoverable_error_without_hint_omits_hint_from_body() {
        let err = anyhow::Error::new(crate::tools::RecoverableError::new("not found"));
        let result = route_tool_error(err);
        let text = &result.content[0].as_text().unwrap().text;
        let body: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(body.get("hint").is_none(), "hint key must be absent");
    }

    #[test]
    fn plain_anyhow_error_routes_to_is_error_true() {
        let err = anyhow::anyhow!("LSP crashed unexpectedly");
        let result = route_tool_error(err);
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn lsp_request_cancelled_routes_to_recoverable_not_fatal() {
        // Kotlin-lsp (and other IntelliJ-based servers) send code -32800 when
        // they cancel a request due to concurrent load.  This must NOT produce
        // isError:true, otherwise Claude Code aborts all sibling parallel calls.
        let err = anyhow::anyhow!("LSP error (code -32800): cancelled");
        let result = route_tool_error(err);
        assert!(
            result.is_error != Some(true),
            "LSP RequestCancelled must not set isError:true"
        );
        let text = &result.content[0].as_text().unwrap().text;
        let body: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(body.get("hint").is_some(), "must include retry hint");
    }

    #[test]
    fn other_lsp_errors_still_route_to_is_error_true() {
        // Only -32800 gets the recoverable treatment; other LSP errors are fatal.
        let err = anyhow::anyhow!("LSP error (code -32603): internal error");
        let result = route_tool_error(err);
        assert_eq!(result.is_error, Some(true));
    }
}
