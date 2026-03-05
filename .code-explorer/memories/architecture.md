# Architecture

## Layer Structure
```
MCP Layer (rmcp)
  CodeScoutServer (src/server.rs) — ServerHandler impl, tool dispatch, error routing
      ↓
Agent / Orchestrator (src/agent.rs)
  Agent { inner: Arc<RwLock<AgentInner>> } — holds ActiveProject, config, cached embedder
      ↓ (via ToolContext)
┌──────────┬──────────┬──────────┬──────────┐
│ LSP      │ AST      │ Git      │ Embed    │
│ src/lsp/ │ src/ast/ │ src/git/ │ src/embed│
└──────────┴──────────┴──────────┴──────────┘
      ↓
Storage: .code-explorer/embeddings.db (sqlite-vec), .code-explorer/memories/
```

## Key Abstractions

**`Tool` trait** (`src/tools/mod.rs:209`) — Every capability is a struct implementing:
- `name()`, `description()`, `input_schema()` — MCP registration
- `async call(Value, &ToolContext) -> Result<Value>` — execution
- `format_compact()` / `format_for_user_channel()` — output shaping

**`ToolContext`** (`src/tools/mod.rs:47`) — Injected into every tool call:
- `agent: Arc<Agent>` — project root, config, LSP access
- `lsp: Arc<LspManager>` — multi-language LSP client pool
- `output_buffer: Arc<OutputBufferStore>` — @ref handle system
- `progress: Arc<ProgressReporter>` — streaming progress

**`Agent`** (`src/agent.rs:33`) — Central orchestrator, `Arc<RwLock<AgentInner>>` pattern for shared mutable state. Holds `ActiveProject` (path, config, memory store, library registry).

**`CodeScoutServer`** (`src/server.rs:38`) — `rmcp::ServerHandler` impl. Registered `Vec<Arc<dyn Tool>>` dispatched dynamically in `call_tool`. Strips project root from paths in responses (privacy).

**`RecoverableError`** (`src/tools/mod.rs:67`) — Expected, input-driven failures. Routed to `isError: false` with `{"error":"…","hint":"…"}` so sibling parallel tool calls in Claude Code aren't aborted. Use `anyhow::bail!` only for genuine crashes.

**`OutputGuard`** (`src/tools/output.rs`) — Enforces two-mode output: Exploring (compact, ≤200 items) vs Focused (full, paginated). Not per-tool logic — a shared pattern.

## Data Flow (typical tool call)
```
Claude → MCP request → CodeScoutServer::call_tool
  → route to Tool impl → Tool::call(params, &ToolContext)
    → Agent::active_project() → get root/config
    → LspManager::get_or_start(lang) → JSON-RPC to LSP process
    → format result via OutputGuard → maybe store in OutputBuffer
  → strip_project_root_from_result → MCP response → Claude
```

## Design Patterns
- **Progressive disclosure**: OutputGuard, compact default, `detail_level: "full"` on demand
- **No echo in writes**: mutation tools return `json!("ok")` — never reflect back the input
- **RecoverableError vs bail**: input errors → recoverable; tool crashes → fatal
- **Three-query sandwich**: cache-invalidation tests need baseline → stale assert → fresh assert
- **Incremental embedding index**: git diff → mtime → SHA-256 fallback chain

## Entry Points
- `src/main.rs` — CLI (start, index, dashboard subcommands)
- `src/server.rs:run()` — starts MCP server, registers all 28 tools
- `src/tools/mod.rs` — Tool trait + ToolContext + tool registration list
