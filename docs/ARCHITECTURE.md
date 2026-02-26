# Architecture

## Overview

code-explorer is an MCP server that gives LLMs IDE-grade code intelligence. It exposes symbol-level tools so agents can navigate and edit code semantically.

```
┌────────────────────────────────────────────────────────┐
│              MCP Layer (rmcp)                           │
│   CodeExplorerServer → registered tools (33)           │
└────────────────────────────────────────────────────────┘
                          ↓
┌────────────────────────────────────────────────────────┐
│              Agent / Orchestrator                       │
│   ProjectManager, ToolRegistry, ConfigSystem           │
└────────────────────────────────────────────────────────┘
          ↓                    ↓                  ↓                  ↓
┌─────────────────┐  ┌──────────────────┐  ┌──────────────┐  ┌──────────────────┐
│  LSP Client     │  │  AST Engine      │  │  Git Engine  │  │  Embedding       │
│  (30+ langs)    │  │  (tree-sitter)   │  │  (git2-rs)   │  │  Engine          │
└─────────────────┘  └──────────────────┘  └──────────────┘  └──────────────────┘
          ↓                    ↓                                        ↓
┌────────────────────────────────────────────────────────────────────────────────┐
│                         Storage / Index Layer                                   │
│   SymbolIndex, EmbeddingIndex (sqlite-vec), MemoryStore, IncrementalCache      │
└────────────────────────────────────────────────────────────────────────────────┘
```

## Components

### MCP Server (`src/server.rs`)

Bridges the internal `Tool` trait to rmcp's `ServerHandler`. All tools are registered as `Vec<Arc<dyn Tool>>` and dispatched dynamically in `call_tool`.

- Stdio transport via `rmcp::transport::stdio()`
- HTTP/SSE transport via `rmcp::transport::sse_server::SseServer`
- `route_tool_error` in `server.rs` routes tool failures:
  `RecoverableError` → `isError:false` + JSON hint (sibling calls not aborted);
  other errors → `isError:true` (fatal)
- **Graceful shutdown**: `shutdown_signal()` listens for SIGINT/SIGTERM via `tokio::select!`. Both transport paths call `lsp.shutdown_all()` before exiting, ensuring child LSP processes are properly terminated.

### Agent (`src/agent.rs`)

Central orchestrator holding active project state behind `RwLock`. Manages:
- Active project root and config
- Memory store reference
- Project detection and activation

### Config (`src/config/`)

- `project.rs` — `ProjectConfig` loaded from `.code-explorer/project.toml` or sensible defaults. Holds embeddings config, ignored paths, project metadata.
- `modes.rs` — `Mode` (Planning/Editing/Interactive/OneShot) and `Context` (Agent/DesktopApp/IdeAssistant) enums.

### LSP Client (`src/lsp/`)

- `symbols.rs` — Language-agnostic `SymbolInfo`/`SymbolKind` types with `From<lsp_types::SymbolKind>`
- `servers/mod.rs` — Default LSP server configs for 9 languages (rust-analyzer, pyright, typescript-language-server, gopls, jdtls, kotlin-language-server, clangd, omnisharp, solargraph)
- `client.rs` — `LspClient` with JSON-RPC transport, lifecycle management, and full LSP request support. Stores `child_pid` for kill-on-drop safety net (SIGTERM via `libc::kill` in `Drop` impl).

### AST Engine (`src/ast/`)

- `mod.rs` — `detect_language()` supporting 20+ file extensions; `extract_symbols()` delegates to parser
- `parser.rs` — `extract_symbols()` via tree-sitter grammars for Rust, Python, TypeScript, Go, Java, Kotlin

### Git Engine (`src/git/`)

- `mod.rs` — `open_repo()`, `head_short_sha()`, `file_log()` returning `Vec<CommitSummary>` via git2
- `blame.rs` — `blame_file()` returning `Vec<BlameLine>` with author, date, SHA, line content

### Embedding Engine (`src/embed/`)

Embedded semantic search with zero external services.

- `schema.rs` — `CodeChunk` and `SearchResult` data types
- `chunker.rs` — Language-aware recursive text splitter tracking 1-indexed line numbers. Handles overlap via character-count estimation.
- `index.rs` — SQLite schema (`files`, `chunks`, `chunk_embeddings`, `meta`, `drift_report`), CRUD operations, pure-Rust cosine similarity search, `build_index()` for incremental project indexing. Change detection fallback chain: git diff → mtime → SHA-256.
- `drift.rs` — `compute_file_drift()`: content-hash-first chunk matching, greedy cosine pairing on remainder. Produces per-file `avg_drift` + `max_drift` scores. Opt-out via `drift_detection_enabled = false` config.
- `remote.rs` — `RemoteEmbedder` supporting OpenAI, Ollama, and custom API endpoints
- `mod.rs` — `Embedder` trait, `create_embedder()` factory, `embed_one()` helper

**sqlite-vec**: Extension loading is commented out (TODO). Pure-Rust cosine fallback works but loads all embeddings into memory.

### Library Registry (`src/library/`)

Third-party library source code navigation (read-only).

- `registry.rs` — `LibraryRegistry` persists known library paths in `.code-explorer/libraries.json`. CRUD + serialization.
- `discovery.rs` — `discover_library_from_path()`: walks parent dirs to find package manifests (Cargo.toml, package.json, pyproject.toml, go.mod). Auto-triggered when LSP goto_definition returns a path outside the project root.
- `scope.rs` — `Scope` enum: `Project`, `Library(name)`, `Libraries`, `All`. Parsed from the `scope` string parameter on symbol/semantic tools.

### Memory (`src/memory/`)

Markdown-based persistent store in `.code-explorer/memories/`. Supports nested topics (path-like), directory traversal protection, CRUD operations.

### Tools (`src/tools/`)

Each tool implements the `Tool` trait (`name`, `description`, `input_schema`, `async call`). Organized by category:

| Category | File | Tools | Status |
|----------|------|-------|--------|
| File | `file.rs` | read_file, list_dir, search_for_pattern, find_file, create_text_file, replace_content, edit_lines | Working |
| Workflow | `workflow.rs` | onboarding, check_onboarding_performed, execute_shell_command | Working |
| Symbol | `symbol.rs` | find_symbol, get_symbols_overview, find_referencing_symbols, replace_symbol_body, insert_before_symbol, insert_after_symbol, rename_symbol (all support `scope` param) | Working (LSP) |
| AST | `ast.rs` | list_functions, extract_docstrings | Working (tree-sitter) |
| Git | `git.rs` | git_blame, git_log, git_diff | Working |
| Semantic | `semantic.rs` | semantic_search, index_project, index_status, check_drift | Working |
| Library | `library.rs` | list_libraries, index_library | Working |
| Memory | `memory.rs` | write_memory, read_memory, list_memories, delete_memory | Working |
| Config | `config.rs` | activate_project, get_current_config | Working |

### Utilities (`src/util/`)

- `fs.rs` — `find_ancestor_with()`, `detect_project_root()`, `read_utf8()`, `write_utf8()`
- `text.rs` — `truncate()`, `count_lines()`, `extract_lines()`

## Tech Stack

| Component | Crate(s) |
|-----------|----------|
| Async runtime | `tokio` |
| MCP protocol | `rmcp` (with `transport-io`, `server`, `macros`) |
| LSP types | `lsp-types` |
| AST parsing | `tree-sitter` (grammar integration pending) |
| Git | `git2` |
| Serialization | `serde`, `serde_json`, `toml` |
| Regex | `regex` |
| File walking | `walkdir`, `ignore`, `globset` |
| Error handling | `anyhow` |
| Logging | `tracing`, `tracing-subscriber` |
| CLI | `clap` |
| Embeddings (cloud) | `reqwest` (feature-gated: `remote-embed`) |
| Vector store | `rusqlite` (bundled SQLite) |
| Hashing | `sha2`, `hex` |
| Schema gen | `schemars` |
| Process mgmt | `libc` (SIGTERM in LspClient Drop) |

## Design Principles

- **Symbol-first**: Operate at symbol/AST level, not raw text
- **Language-agnostic**: Uniform interface across all supported languages
- **Offline-first**: All features work without external APIs
- **Composable tools**: Small focused tools that combine well
- **Fail gracefully**: LSP down → tree-sitter → text fallback
- **Token-efficient**: Return minimal context; let the agent request more
