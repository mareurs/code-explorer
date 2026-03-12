# Architecture

## Overview

> **User documentation:** This file covers contributor-level internals. For the
> user-facing manual — installation, tool reference, and semantic search guide — see
> [`docs/manual/src/`](manual/src/introduction.md).

codescout is an MCP server that gives LLMs IDE-grade code intelligence. It exposes symbol-level tools so agents can navigate and edit code semantically.

```
┌────────────────────────────────────────────────────────┐
│              MCP Layer (rmcp)                           │
│   CodeScoutServer → registered tools (29)              │
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
│   SymbolIndex, EmbeddingIndex (SQLite/cosine), MemoryStore, IncrementalCache   │
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

- `project.rs` — `ProjectConfig` loaded from `.codescout/project.toml` or sensible defaults. Holds embeddings config, ignored paths, project metadata.
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

**sqlite-vec**: Fully active — `init_sqlite_vec()` registers the extension at startup. `maybe_migrate_to_vec0()` transparently upgrades existing plain-BLOB `chunk_embeddings` tables to `vec0` virtual tables on first use. `search_scoped` dispatches to `search_scoped_vec0` once migrated, falling back to pure-Rust cosine only for un-migrated DBs. Both the code index and `vec_memories` (semantic memory recall) use vec0.

### Library Registry (`src/library/`)

Third-party library source code navigation (read-only).

- `registry.rs` — `LibraryRegistry` persists known library paths in `.codescout/libraries.json`. CRUD + serialization.
- `discovery.rs` — `discover_library_from_path()`: walks parent dirs to find package manifests (Cargo.toml, package.json, pyproject.toml, go.mod). Auto-triggered when LSP goto_definition returns a path outside the project root.
- `scope.rs` — `Scope` enum: `Project`, `Library(name)`, `Libraries`, `All`. Parsed from the `scope` string parameter on symbol/semantic tools.

### Memory (`src/memory/`)

Two-tier persistent store:

- **File store** — Markdown files in `.codescout/memories/`. Supports nested topics (path-like), directory traversal protection, CRUD operations. Topics accessible via `memory(action: read/write/list/delete)`.
- **Semantic store** — Vector embeddings of memory entries in `.codescout/embeddings.db` (shared with the code index). Supports natural-language recall via `memory(action: remember/recall/forget)`. Each write operation cross-embeds into this store (best-effort, non-fatal if unavailable).
- **Anchor sidecars** — `.anchors.toml` files alongside each memory track which source files the memory references. Used by `project_status` to surface `memory_staleness`. Regenerated on each `write`; manually cleared via `memory(action: refresh_anchors)`.

### Usage Recorder (`src/usage/`)

Transparent wrapper around the tool dispatch loop in `server.rs`. Records every tool call to `.codescout/usage.db` (append-only SQLite). Captures: tool name, timestamp, outcome (success/error/overflow), latency (ms), and output mode. Accessible via the dashboard (`code-explorer dashboard`).

### Dashboard (`src/dashboard/`)

Axum HTTP server launched via `codescout dashboard --project . [--port 8099]`. Serves a static HTML/CSS/JS app with multiple views: Tool Stats (per-tool call charts from `usage.db`), index status, memories browser, and library list. API routes under `/api/` read from the same data sources as the MCP tools. Not started by the MCP server — opt-in via the `dashboard` CLI subcommand.

### Tools (`src/tools/`)

Each tool implements the `Tool` trait (`name`, `description`, `input_schema`, `async call`). Organized by category:

| Category | File | Tools |
|----------|------|-------|
| File | `file.rs` | `read_file`, `list_dir`, `search_pattern`, `find_file`, `create_file`, `edit_file` |
| Workflow | `workflow.rs` | `onboarding`, `run_command` |
| Symbol | `symbol.rs` | `find_symbol`, `list_symbols`, `goto_definition`, `hover`, `find_references`, `replace_symbol`, `remove_symbol`, `insert_code`, `rename_symbol` (all navigation tools support `scope` param) |
| Semantic | `semantic.rs` | `semantic_search`, `index_project`, `index_status` |
| Library | `library.rs` | `list_libraries` |
| Memory | `memory.rs` | `memory` (dispatches `read` / `write` / `list` / `delete` / `remember` / `recall` / `forget` / `refresh_anchors` via `action` param) |
| Config | `config.rs` | `activate_project`, `project_status` |
| GitHub | `github.rs` | `github_identity`, `github_issue`, `github_pr`, `github_file`, `github_repo` |

### Utilities (`src/util/`)

- `fs.rs` — `find_ancestor_with()`, `detect_project_root()`, `read_utf8()`, `write_utf8()`
- `text.rs` — `truncate()`, `count_lines()`, `extract_lines()`
- `path_security.rs` — `PathSecurityConfig`, `validate_read_path()`, `validate_write_path()`. Enforces the permission model: reads are permissive (deny-list only), writes are sandboxed to the project root. All write tools call `validate_write_path()` before any I/O; violations return `RecoverableError` so agents recover without user interruption. See [Security & Permissions](manual/src/concepts/security.md).

## Tech Stack

| Component | Crate(s) |
|-----------|----------|
| Async runtime | `tokio` |
| MCP protocol | `rmcp` (with `transport-io`, `server`, `macros`) |
| LSP types | `lsp-types` |
| AST parsing | `tree-sitter` (Rust, Python, TypeScript, Go, Java, Kotlin) |
| Git | `git2` |
| Serialization | `serde`, `serde_json`, `toml` |
| Regex | `regex` |
| File walking | `walkdir`, `ignore`, `globset` |
| Error handling | `anyhow` |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` (file rotation) |
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
- **Safe by default**: Writes are sandboxed to the project root; shell execution is off by default; credential paths are unconditionally blocked. Violations are recoverable errors, not crashes — agents continue uninterrupted. See [Security & Permissions](manual/src/concepts/security.md).
