# Architecture

See `docs/ARCHITECTURE.md` for the layer diagram. This file supplements it with concrete data flow.

## Key Abstractions

| Type | File | Role |
|---|---|---|
| `CodeScoutServer` | `src/server.rs:38` | rmcp `ServerHandler` impl — owns `Vec<Arc<dyn Tool>>`, dispatches `call_tool`, routes errors |
| `Agent` | `src/agent.rs:33` | Shared state behind `Arc<RwLock<AgentInner>>` — active project, config, embedder cache, memory, LSP manager |
| `ActiveProject` | `src/agent.rs:48` | Holds `root: PathBuf`, `ProjectConfig`, `LspManager`, `LibraryRegistry` |
| `Tool` trait | `src/tools/mod.rs:217` | Interface all 28 tools implement: `name()`, `description()`, `input_schema()`, `async call(Value, &ToolContext)` |
| `ToolContext` | `src/tools/mod.rs:47` | Per-call context passed to every tool: `agent`, `lsp`, `output_buffer`, `progress` |
| `LspClient` | `src/lsp/client.rs:96` | Manages one language server process via stdin/stdout JSON-RPC |
| `LspManager` | `src/lsp/manager.rs:19` | Manages multiple `LspClient` instances, keyed by language |
| `OutputGuard` | `src/tools/output.rs` | Enforces exploring/focused mode limits; buffers large output as `@ref` handles |

## Tool Call Data Flow

```
MCP client → CodeScoutServer::call_tool()
  → find matching Tool in Vec<Arc<dyn Tool>>
  → UsageRecorder wraps the call (records timing/outcome to usage.db)
  → Tool::call(params, &ToolContext)
      → ToolContext.agent (for config, memory, embedder)
      → ToolContext.lsp (for LSP ops via LspManager)
      → OutputGuard enforces compact/focused mode
  → route_tool_error(): RecoverableError → isError:false; anyhow → isError:true
  → strip_project_root_from_result() removes absolute path prefix
  → response to MCP client
```

## LSP Flow

`LspManager::get_or_start()` lazily spawns language servers. `LspClient` tracks an incremental request ID, sends JSON-RPC over `stdin`, reads responses from `stdout`. `did_change` notifications update the server's view of open files. `LspClientOps` trait (in `lsp/ops.rs`) is the mockable interface for tests.

## Embedding Flow

`chunker::split()` → `RemoteEmbedder::embed()` → `index::insert_chunk()` into SQLite. Search via pure-Rust cosine similarity (sqlite-vec extension disabled). Change detection: git diff → mtime → SHA-256 fallback.

## Invariants

Rules that must never be broken. Each has a specific, observable failure mode.

| Rule | Why it exists |
|---|---|
| `OutputGuard` is the only output limiter (`src/tools/output.rs`) | Per-tool limits create inconsistency; the guard enforces both modes globally |
| Mutation tools return `json!("ok")`, never echo content back | Caller already has what they sent — echoing wastes tokens with zero information gain |
| `RecoverableError` for user-fixable failures; `anyhow::bail!` for real failures | Controls MCP `isError` flag — `bail!` aborts sibling parallel tool calls, `RecoverableError` does not |
| All 3 prompt surfaces updated together on tool changes | `server_instructions.md`, `onboarding_prompt.md`, `build_system_prompt_draft()` in `workflow.rs` — silent staleness corrupts agent guidance |
| New tools must be registered in `CodeScoutServer::new()` | Tools are matched by name string in a Vec — unregistered tools silently never run |

## Strong Defaults

Preferred behaviors that can be overridden with deliberate reason.

| Default | When it's okay to break it |
|---|---|
| Exploring mode (compact output) by default | Switch to `detail_level: "full"` once you know the specific symbol/file you need |
| Lazy LSP startup — servers start on first use | Only when diagnostics are needed before the first file edit |
| `RecoverableError` prefers a `hint` when a corrective action exists | Omit hint only when there is genuinely no corrective action |
| Tools live in their category file (`file.rs`, `symbol.rs`, etc.) | Only when a tool genuinely spans multiple categories |
