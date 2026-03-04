# Architecture

This page describes how codescout works internally. It is written for users
who want to understand the system, not for contributors adding new tools or
languages (see the [Extending](extending/adding-languages.md) chapter for that).

---

## System Overview

codescout is an MCP server that gives LLMs IDE-grade code intelligence. It
sits between the AI assistant (Claude Code, Cursor, or any MCP-capable client)
and the project's source code, providing 23 tools for navigation, search,
editing, and analysis.

The server is a single Rust binary. It launches language servers, parses source
files with tree-sitter, manages a vector embedding index, and reads git history
-- all behind a uniform MCP tool interface. The AI assistant never interacts
with these backends directly; it calls tools, and codescout handles the
rest.

---

## Component Diagram

```
Claude Code ──MCP──▶ CodeScoutServer
                          │
                    ┌─────┼─────┐
                    ▼     ▼     ▼
                  Agent  Tools  Instructions
                    │     │
              ┌─────┼─────┼─────┐
              ▼     ▼     ▼     ▼
            Config  LSP   AST   Embeddings
              │     │     │     │
              ▼     ▼     ▼     ▼
          project  Language  tree-sitter  SQLite
          .toml    Servers   grammars     index
```

**CodeScoutServer** is the MCP entry point. It holds the Agent, the tool
registry, and the server instructions that get sent to the LLM.

**Agent** manages the active project: root path, configuration, memory store.

**Tools** are stateless structs dispatched by name. All state flows through
`ToolContext`, which holds references to the Agent and the LSP manager.

**Instructions** are markdown text sent to the LLM as part of the MCP server
info. They guide the LLM on how to use the tools effectively.

---

## Request Lifecycle

When the LLM calls a tool, here is what happens:

1. **MCP request arrives.** A JSON-RPC message comes in over stdio (single
   connection) or HTTP/SSE (multi-connection). The `rmcp` crate handles
   protocol framing.

2. **Tool lookup.** `CodeScoutServer::call_tool()` searches the tool
   registry -- a `Vec<Arc<dyn Tool>>` -- for a tool matching the requested
   name. If no match is found, an `invalid_params` MCP error is returned.

3. **Security check.** Before the tool runs, `check_tool_access()` verifies
   that the tool is not disabled by the project's security configuration. For
   example, if `shell_enabled` is false, `run_command` is blocked
   here. If `git_enabled` is false, git tools are blocked.

4. **ToolContext creation.** A `ToolContext` is assembled with clones of the
   `Agent` and `Arc<LspManager>`. This is the only state a tool receives.

5. **Tool execution.** The tool's `call()` method runs with the parsed JSON
   input and the context. Tools are async and can call into LSP servers, read
   files, query the embedding index, or run shell commands.

6. **Result or error.** On success, the tool returns a `serde_json::Value`
   that gets serialized to a `CallToolResult` with text content. On failure,
   the error is wrapped in `CallToolResult::error` -- it is surfaced to the
   LLM as an error message, not as an MCP protocol error. This means tool
   failures are recoverable: the LLM sees the error and can try a different
   approach.

---

## Key Components

### Agent

**Source:** `src/agent.rs`

The Agent manages the active project state. It holds:

- **Project root** -- the filesystem path to the project being explored.
- **Configuration** -- the parsed `project.toml` settings.
- **Memory store** -- the markdown-backed key-value store for persistent
  knowledge.

The Agent is thread-safe via `Arc<RwLock<AgentInner>>`. It is cloned and shared
across all tool calls and, in HTTP mode, across all connections. Calling
`activate_project` swaps the inner project state atomically.

### Tool Registry

**Source:** `src/server.rs`

All 23 tools are registered at startup in `CodeScoutServer::from_parts()` as
a `Vec<Arc<dyn Tool>>`. Dispatch is by name: `call_tool()` iterates the vector
and matches on `tool.name()`.

Each tool is a zero-size struct implementing the `Tool` trait:

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;
}
```

Tools are stateless. All project state lives in the `ToolContext` they receive
on each call. This means tools are trivially shareable across threads and
connections.

### LSP Manager

**Source:** `src/lsp/manager.rs`, `src/lsp/client.rs`, `src/lsp/servers/`

The LSP Manager starts language server processes on demand. When a symbol tool
is called for a file, the manager checks the file's language, looks up the
default server configuration, and either reuses an existing server process or
spawns a new one.

Supported language servers:

| Language            | Server                       |
|---------------------|------------------------------|
| Rust                | `rust-analyzer`              |
| Python              | `pyright-langserver`         |
| TypeScript/JS/TSX   | `typescript-language-server` |
| Go                  | `gopls`                      |
| Java                | `jdtls`                      |
| Kotlin              | `kotlin-language-server`     |
| C/C++               | `clangd`                     |
| C#                  | `OmniSharp`                  |
| Ruby                | `solargraph`                 |

Communication with language servers uses JSON-RPC over stdio. The `LspClient`
struct handles the LSP initialization handshake, sends requests
(`textDocument/documentSymbol`, `textDocument/references`, `textDocument/rename`,
etc.), and parses responses using `lsp-types`.

Server processes are long-lived: once started, they persist for the lifetime of
the MCP session. On shutdown (SIGINT, SIGTERM, or MCP connection close), the
server calls `LspManager::shutdown_all()`, which sends proper LSP
`shutdown`/`exit` messages to each language server for a clean exit. As a safety
net, the `LspClient` Drop implementation also sends SIGTERM to the child process
via `libc::kill`, ensuring cleanup even if the graceful path is bypassed.

### AST Engine

**Source:** `src/ast/`

Tree-sitter provides offline parsing that works without a language server. It
is faster than LSP for simple structural queries and has zero startup cost.

The AST engine is used internally for richer symbol extraction and semantic chunking. It is not exposed as a standalone tool — use `list_symbols` (LSP-backed) for interactive symbol navigation.

Supported languages for tree-sitter: Rust, Python, TypeScript, Go, Java,
Kotlin. See the [Language Support](language-support.md) page for the full
matrix.

### Embedding Pipeline

**Source:** `src/embed/`

The embedding pipeline enables semantic search -- finding code by meaning
rather than by name. It has four stages:

1. **Chunking** (`src/embed/chunker.rs`, `src/embed/ast_chunker.rs`) -- Source
   files are split into overlapping text chunks. For languages with tree-sitter
   support, the chunker uses AST boundaries (functions, classes, blocks) to
   create semantically coherent chunks. For other files, it falls back to
   line-based splitting with configurable `chunk_size` and `chunk_overlap`.

2. **Embedding** (`src/embed/remote.rs`, `src/embed/local.rs`) -- Each chunk
   is sent to an embedding backend that returns a vector representation. Two
   backends are available:
   - **Remote** (default) -- HTTP client that talks to Ollama, OpenAI, or any
     OpenAI-compatible endpoint.
   - **Local** -- CPU-based embeddings via fastembed-rs and ONNX Runtime. No
     external service needed.

3. **Storage** (`src/embed/index.rs`) -- Vectors and chunk metadata are stored
   in a SQLite database at `.code-explorer/embeddings.db`. Each chunk records
   its file path, line range, content hash, and embedding vector as a blob.

4. **Search** (`src/embed/index.rs`) -- Query text is embedded using the same
   model, then compared against all stored vectors using cosine similarity.
   Results are ranked by similarity score and returned with file paths, line
   ranges, and content previews.

The index tracks file content hashes. On incremental re-indexing, only files
that changed since the last index build are re-chunked and re-embedded.

### Memory Store

**Source:** `src/memory/`

A lightweight key-value store backed by markdown files in
`.code-explorer/memories/`. Topics are path-like strings (e.g.,
`debugging/async-patterns`) that map to files on disk.

The store supports four operations: write, read, list, and delete. There is no
search -- topics are listed by name and read individually. Use this for
recording decisions, conventions, and project-specific knowledge that should
persist across agent sessions.

---

## Transport Modes

codescout supports two transport modes, selected at startup via the
`--transport` flag.

### stdio (default)

```bash
codescout start --project /path/to/project
```

Single connection. Claude Code launches the server as a subprocess and
communicates over stdin/stdout. No authentication is needed because the
connection is local and exclusive.

This is the standard mode for Claude Code integration. The MCP registration
command (`claude mcp add`) sets this up automatically.

### HTTP/SSE

```bash
codescout start --project /path/to/project --transport http --port 8080
```

Multi-connection. The server binds to a port and accepts SSE (Server-Sent
Events) connections. Each connection gets its own `CodeScoutServer` instance
but shares the same Agent and LSP Manager.

An auth token is auto-generated and printed to stderr at startup. Clients must
send it as a `Bearer` token in the `Authorization` header. You can also
provide your own token via `--auth-token`.

Use HTTP mode when the MCP client runs on a different machine, or when
multiple clients need to share a single server.

---

## Storage

All persistent data lives in `.code-explorer/` within the project root:

```
<project-root>/
└── .code-explorer/
    ├── project.toml      # Configuration
    ├── embeddings.db      # SQLite vector index
    └── memories/          # Markdown knowledge files
        ├── topic-a.md
        └── debugging/
            └── async-patterns.md
```

This directory is created automatically when a project is first activated.
Add `.code-explorer/` to your `.gitignore` -- it contains machine-local state
(embedding vectors, memory notes) that should not be committed.

The `project.toml` file is an exception: you may want to commit it so that
team members share the same configuration. See
[Project Configuration](configuration/project-toml.md) for details.

---

## Tech Stack

| Crate | Purpose |
|-------|---------|
| `rmcp` | MCP protocol implementation (stdio and SSE transports) |
| `lsp-types` | LSP type definitions |
| `tree-sitter` + language grammars | Offline AST parsing |
| `git2` | Git operations (blame, log, diff) |
| `rusqlite` | SQLite for embedding storage |
| `reqwest` | HTTP client for remote embedding backends |
| `fastembed` | Local CPU embeddings (optional, `local-embed` feature) |
| `tokio` | Async runtime |
| `clap` | CLI argument parsing |
| `serde` / `serde_json` | JSON serialization |
| `tracing` | Structured logging |
| `libc` | POSIX signals for LSP process cleanup |

---

## Further Reading

- [Progressive Disclosure](concepts/progressive-disclosure.md) -- how output
  volume is controlled across all tools.
- [Project Configuration](configuration/project-toml.md) -- all settings in
  `project.toml`.
- [Embedding Backends](configuration/embedding-backends.md) -- configuring
  Ollama, OpenAI, or local embeddings.
- [Semantic Search Concepts](concepts/semantic-search.md) — how the embedding
  pipeline works, similarity scoring, and when to reach for semantic vs symbol search
- [Dashboard](concepts/dashboard.md) — visual UI for project health, tool usage
  stats, index status, and memory browsing
- The full internal architecture with contributor-level detail is in
  `docs/ARCHITECTURE.md` in the repository root.
