# code-explorer

Rust MCP server giving LLMs IDE-grade code intelligence — symbol-level navigation, semantic search, git integration. Inspired by [Serena](./serena-as-reference/) and [cocoindex-code](../cocoindex-code/).

## Development Commands

```bash
cargo build                        # Build
cargo test                         # Run tests (150+ passing)
cargo clippy -- -D warnings        # Lint
cargo fmt                          # Format
cargo run -- start --project .     # Run MCP server (stdio)
cargo run -- index --project .     # Build embedding index
```

**Always run `cargo fmt`, `cargo clippy`, and `cargo test` before completing any task.**

## Project Structure

```
src/
├── main.rs          # CLI: start (MCP server) and index subcommands
├── server.rs        # rmcp ServerHandler — bridges Tool trait to MCP
├── agent.rs         # Orchestrator: active project, config, memory
├── config/          # ProjectConfig (.code-explorer/project.toml), modes
├── lsp/             # LSP types, server configs (9 langs), client (stub)
├── ast/             # Language detection (20+ exts), parser (stub)
├── git/             # git2: blame, file_log, open_repo
├── embed/           # Chunker, SQLite index, RemoteEmbedder, schema
├── memory/          # Markdown-based MemoryStore (.code-explorer/memories/)
├── tools/           # Tool implementations by category
│   ├── file.rs      #   read_file, list_dir, search_for_pattern  ← working
│   ├── workflow.rs  #   execute_shell_command  ← working
│   ├── symbol.rs    #   7 LSP-backed tools  ← stubs
│   ├── git.rs       #   blame, log, diff  ← stubs (backends exist)
│   ├── semantic.rs  #   search, index_project, index_status  ← stubs (backends exist)
│   ├── memory.rs    #   CRUD tools  ← stubs (MemoryStore exists)
│   ├── ast.rs       #   list_functions, extract_docstrings  ← stubs
│   └── config.rs    #   activate_project, get_current_config  ← stubs
└── util/            # fs helpers, text processing
```

## Design Principles

**Progressive Disclosure** — Every tool defaults to the most compact useful
representation. Details are available on demand via `detail_level: "full"` +
pagination. Tools never dump unbounded output. See `docs/plans/2026-02-25-progressive-disclosure-design.md`.

**Token Efficiency** — The LLM's context window is a scarce resource. Tools
minimize output by default: names + locations in exploring mode, full bodies
only in focused mode. Overflow produces actionable guidance ("showing N of M,
narrow with..."), not truncated garbage.

**Two Modes** — `Exploring` (default): compact, capped at 200 items. `Focused`:
full detail, paginated via offset/limit. Enforced via `OutputGuard`
(`src/tools/output.rs`), a project-wide pattern not per-tool logic.

**Tool Selection by Knowledge Level** — Know the name → LSP/AST tools
(`find_symbol`, `get_symbols_overview`). Know the concept → semantic search
first, then drill down. Know nothing → `list_dir` + `get_symbols_overview` at
top level, then semantic search.

## Key Patterns

**Tool trait** (`src/tools/mod.rs`): Each tool is a struct implementing `name()`, `description()`, `input_schema()`, `async call(Value) -> Result<Value>`. All use `#[async_trait]`.

**Tool↔MCP bridge** (`src/server.rs`): Tools registered as `Vec<Arc<dyn Tool>>`, dispatched dynamically in `call_tool`. Tool errors return `CallToolResult::error` (shown to LLM), never panic.

**Config** (`.code-explorer/project.toml`): Per-project settings including embedding model, chunk size, ignored paths. `ProjectConfig::load_or_default()` handles missing config gracefully.

**Embedding pipeline**: `chunker::split()` → `RemoteEmbedder::embed()` → `index::insert_chunk()` → `index::search()` (cosine similarity). All stored in `.code-explorer/embeddings.db`.

## Docs

- `docs/plans/2026-02-25-v1-implementation-plan.md` — Sprint-level plan (Phase 0–5, 15 sprints)
- `docs/ARCHITECTURE.md` — Component details, tech stack, design principles
- `docs/ROADMAP.md` — Quick status overview

## Reference

- `serena-as-reference/` — Tool API patterns, LSP integration, memory system
- `../cocoindex-code/` — Chunking strategy, sqlite-vec schema, incremental indexing
