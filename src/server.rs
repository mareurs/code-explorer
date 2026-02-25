//! MCP server — bridges our `Tool` registry to rmcp's `ServerHandler`.

use std::path::PathBuf;
use std::sync::Arc;

use crate::lsp::LspManager;

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
    ast::{ExtractDocstrings, ListFunctions},
    config::{ActivateProject, GetCurrentConfig},
    file::{CreateTextFile, FindFile, ListDir, ReadFile, ReplaceContent, SearchForPattern},
    git::{GitBlame, GitDiff, GitLog},
    memory::{DeleteMemory, ListMemories, ReadMemory, WriteMemory},
    semantic::{IndexProject, IndexStatus, SemanticSearch},
    symbol::{
        FindReferencingSymbols, FindSymbol, GetSymbolsOverview, InsertAfterSymbol,
        InsertBeforeSymbol, RenameSymbol, ReplaceSymbolBody,
    },
    workflow::{CheckOnboardingPerformed, ExecuteShellCommand, Onboarding},
    Tool, ToolContext,
};

/// The MCP server handler — holds shared agent state and a registry of tools.
#[derive(Clone)]
pub struct CodeExplorerServer {
    agent: Agent,
    lsp: Arc<LspManager>,
    tools: Vec<Arc<dyn Tool>>,
    instructions: String,
}

impl CodeExplorerServer {
    pub async fn new(agent: Agent) -> Self {
        Self::from_parts(agent, Arc::new(LspManager::new())).await
    }

    /// Create a server with an existing LspManager (used for HTTP multi-session).
    pub async fn from_parts(agent: Agent, lsp: Arc<LspManager>) -> Self {
        let status = agent.project_status().await;
        let instructions = crate::prompts::build_server_instructions(status.as_ref());
        let tools: Vec<Arc<dyn Tool>> = vec![
            // File tools (fully implemented)
            Arc::new(ReadFile),
            Arc::new(ListDir),
            Arc::new(SearchForPattern),
            Arc::new(CreateTextFile),
            Arc::new(FindFile),
            Arc::new(ReplaceContent),
            // Workflow tools (ExecuteShellCommand implemented; others stub)
            Arc::new(ExecuteShellCommand),
            Arc::new(Onboarding),
            Arc::new(CheckOnboardingPerformed),
            // Symbol tools (stub — require LSP)
            Arc::new(FindSymbol),
            Arc::new(FindReferencingSymbols),
            Arc::new(GetSymbolsOverview),
            Arc::new(ReplaceSymbolBody),
            Arc::new(InsertBeforeSymbol),
            Arc::new(InsertAfterSymbol),
            Arc::new(RenameSymbol),
            // AST tools (stub — require tree-sitter wiring)
            Arc::new(ListFunctions),
            Arc::new(ExtractDocstrings),
            // Git tools (stub — require Agent project root)
            Arc::new(GitBlame),
            Arc::new(GitLog),
            Arc::new(GitDiff),
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
            Arc::new(GetCurrentConfig),
        ];
        Self {
            agent,
            lsp,
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

        let input: Value = req
            .arguments
            .map(Value::Object)
            .unwrap_or(Value::Object(Default::default()));

        let ctx = ToolContext {
            agent: self.agent.clone(),
            lsp: self.lsp.clone(),
        };
        match tool.call(input, &ctx).await {
            Ok(output) => {
                let text =
                    serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => {
                // Surface tool errors to the LLM as error content (not an MCP protocol error)
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            }
        }
    }
}

/// Entry point: start the MCP server with the chosen transport.
pub async fn run(project: Option<PathBuf>, transport: &str, host: &str, port: u16) -> Result<()> {
    // If no --project given, auto-detect from CWD (Claude Code launches servers from the project dir)
    let project = project.or_else(|| std::env::current_dir().ok());
    let agent = Agent::new(project).await?;
    let lsp = Arc::new(LspManager::new());

    match transport {
        "stdio" => {
            tracing::info!("code-explorer MCP server ready (stdio)");
            let server = CodeExplorerServer::from_parts(agent, lsp).await;
            let service = server
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| anyhow::anyhow!("MCP server error: {}", e))?;
            service
                .waiting()
                .await
                .map_err(|e| anyhow::anyhow!("MCP server exited: {}", e))?;
            Ok(())
        }
        "http" => {
            let addr: std::net::SocketAddr = format!("{}:{}", host, port).parse()?;
            tracing::info!("code-explorer MCP server ready (HTTP/SSE at {})", addr);
            tracing::info!("  SSE endpoint: http://{}/sse", addr);
            tracing::info!("  Message endpoint: http://{}/message", addr);

            let mut sse_server = rmcp::transport::sse_server::SseServer::serve(addr)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to start SSE server: {}", e))?;

            // Accept connections — each SSE session gets its own handler
            while let Some(transport) = sse_server.next_transport().await {
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

            Ok(())
        }
        other => anyhow::bail!("Unknown transport '{}'. Use 'stdio' or 'http'.", other),
    }
}
