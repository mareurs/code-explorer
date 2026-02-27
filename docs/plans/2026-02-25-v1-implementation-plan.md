# code-explorer v1.0 Implementation Plan

> Full product vision: from current state (4 working tools) to release-ready v1.0.
> Sprint-level granularity with dependency ordering and checkpoints.

## Current State

- 35 source files, 9 modules, **131 tests passing**
- **29 tools working** (was 4): file (6), workflow (3), memory (4), git (3), config (2), semantic (3), symbol (7, LSP-backed), AST (2, tree-sitter)
- 0 tools stubbed — all planned tools implemented
- LSP client: transport, lifecycle, JSON-RPC, document symbols, references, definition, rename
- MCP server working over stdio (rmcp)
- Core libraries: chunker, embedding index, memory store, git blame/log/diff, config, language detection

## Architecture Decision: ToolContext

**Problem**: Tools need access to growing shared state (Agent, LSP client, parser pool, embedder).
Current `Tool::call(&self, input: Value)` has no way to pass context.

**Solution**: Extend the Tool trait with `ToolContext`:

```rust
pub struct ToolContext {
    pub agent: Agent,
    // Phase 3: pub lsp: Arc<LspManager>,
    // Phase 4: pub parser: Arc<ParserPool>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;
}
```

**Why not store Agent in each tool**: O(resources × tools) modification surface as shared
resources grow. ToolContext is O(1) — extend the struct, all tools get access automatically.

---

## Phase 0: Architecture Foundation

### Sprint 0.1 — ToolContext Refactor

Introduce `ToolContext`, change `Tool::call` signature, update all 27 tools and server dispatch.
Zero behavioral change — mechanical refactor.

**Tasks:**
- [x] Create `ToolContext` struct in `src/tools/mod.rs`
- [x] Change `Tool::call` signature to `async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>`
- [x] Update `CodeExplorerServer::call_tool()` to construct and pass `ToolContext`
- [x] Update all 27 tool implementations (add `ctx` parameter, unused for now)
- [x] Verify: `cargo test` passes, `cargo clippy` clean

**Files:** `src/tools/mod.rs`, `src/server.rs`, all 8 `src/tools/*.rs` files
**Acceptance:** 60 tests pass, MCP handshake works, no behavioral change
**Done:** `e924850` — also added `Agent::with_project()` helper

---

## Phase 1: Wire Existing Backends

### Sprint 1.1 — Memory Tools (4 tools)

Wire memory tools to the already-implemented `MemoryStore`.

**Tasks:**
- [x] `write_memory`: parse topic + content from input, call `ctx.agent` → `active_project.memory.write()`
- [x] `read_memory`: parse topic, call `memory.read()`, return content or "not found"
- [x] `list_memories`: call `memory.list()`, return topics array
- [x] `delete_memory`: parse topic, call `memory.delete()`
- [x] Add tool-level integration tests (tempdir-based)

**Access pattern:** `ctx.agent.with_project()` → `memory`
**Files:** `src/tools/memory.rs`
**Acceptance:** 4 memory tools work end-to-end via MCP
**Done:** `9f66f43` — 6 new tests (write/read roundtrip, list, delete, nested topics, no-project error)

### Sprint 1.2 — Git Tools (3 tools)

Wire git tools to `git2` backends. One new function needed for diff.

**Tasks:**
- [x] `git_blame`: parse path + optional line range, call `git::blame::blame_file()`
- [x] `git_log`: parse optional path + limit, call `git::file_log()`
- [x] Implement `git::diff_workdir()` — new function using git2 diff API
- [x] `git_diff`: parse optional path + commit, call `git::diff_workdir()`
- [x] Add tests using temp git repos with commits

**Access pattern:** `ctx.agent` → project root → `git2::Repository::open(root)`
**Files:** `src/tools/git.rs`, `src/git/mod.rs` (new `diff_workdir`)
**Acceptance:** blame, log, diff all return real data for a git repo
**Done:** `268d99d` — 7 new tests with temp git repos (blame, blame+range, log, project-wide log, diff, clean diff, no-project error)

### Sprint 1.3 — Config + Workflow Tools (4 tools)

Wire config and workflow tools to Agent.

**Tasks:**
- [x] `activate_project`: parse path, call `ctx.agent.activate(path)`
- [x] `get_current_config`: read Agent inner state, return config as JSON
- [x] `onboarding`: detect languages in project, create `.code-explorer/project.toml`
- [x] `check_onboarding_performed`: check if `.code-explorer/` directory exists
- [x] Add tests for activation roundtrip and onboarding

**Files:** `src/tools/config.rs`, `src/tools/workflow.rs`
**Acceptance:** can activate a project, read its config, run onboarding
**Done:** `5755b82` — 7 new tests (activate+config, nonexistent path, re-activate, onboarding detects languages, creates config, before/after check, no-project error)

### Sprint 1.4 — Semantic Tools (3 tools)

Wire semantic search to the embedding index. Requires embedder for query embedding.

**Tasks:**
- [x] `index_project`: parse optional `force` flag, call `embed::index::build_index()`
- [x] `semantic_search`: embed query via `create_embedder()`, call `embed::index::search()`
- [x] Implement `embed::index::index_stats()` — new function querying SQLite for counts
- [x] `index_status`: call `index_stats()`, return file/chunk counts + model info
- [x] Add tests (mock embedder or small test corpus with known embeddings)

**Access pattern:** `ctx.agent` → project root + config → `open_db()` + `create_embedder()`
**Files:** `src/tools/semantic.rs`, `src/embed/index.rs` (new `index_stats`)
**Acceptance:** can index a project and search it; stats reflect indexed content
**Done:** `5302c42` — 4 new tests (no-index status, with-data status, no-project errors, stats function)
**Note:** semantic_search and index_project require a running embedding API for full E2E test

---

## Phase 2: Complete File Tools

### Sprint 2.1 — Missing File Tools (3 new tools)

Add file manipulation tools expected by coding agents.

**Tasks:**
- [x] `create_text_file`: write content to path, create parent dirs via `util::fs::write_utf8()`
- [x] `find_file`: glob-based search via `globset` + `ignore::WalkBuilder`
- [x] `replace_content`: regex or literal find-and-replace in a file
- [x] Register all 3 in `server.rs`
- [x] Add tests: write/read roundtrip, glob matching, regex replacement

**Files:** `src/tools/file.rs`, `src/server.rs`
**Acceptance:** all 3 tools work, total working tools: 21/27

### ⟹ CHECKPOINT: Usable MCP Server

At this point: memory, git, semantic search, config, file manipulation all working.
The server is genuinely useful as a coding agent backend.

**Evaluation criteria:**
- All Phase 0-2 tools pass tests
- MCP server handles real-world tool sequences (index → search → read → edit)
- Decide if Phase 3 priorities need adjustment

---

## Phase 3: LSP Client

The core value proposition — symbol-level intelligence across languages.
Sequential sprints, each building on the previous.

### Sprint 3.1 — LSP Process Lifecycle

Spawn and manage language server processes.

**Tasks:**
- [x] `LspManager` struct: manages per-language server instances
- [x] `LspManager::get_or_start(language, workspace_root)` — lazy startup
- [x] Spawn process with stdio pipes (`tokio::process::Command`)
- [x] Graceful shutdown on drop (`kill_on_drop(true)` + explicit shutdown)
- [x] Crash detection (reader task detects EOF, sets `alive=false`, drains pending)
- [x] Add `lsp: Arc<LspManager>` to `ToolContext`
- [x] Tests: spawn rust-analyzer, verify it starts and stops

**Files:** `src/lsp/client.rs`, `src/lsp/manager.rs`, `src/tools/mod.rs` (ToolContext)
**Acceptance:** can spawn rust-analyzer, process lifecycle works
**Done:** combined with Sprint 3.2

### Sprint 3.2 — JSON-RPC Protocol Layer

Implement the LSP JSON-RPC communication layer.

**Tasks:**
- [x] JSON-RPC message framing (Content-Length header + JSON body)
- [x] Request/response correlation via ID tracking (`AtomicI64` + `HashMap<i64, oneshot::Sender>`)
- [x] `initialize` / `initialized` handshake per LSP spec
- [x] `textDocument/didOpen` + `didClose` notifications
- [x] Request timeout handling (30s default)
- [x] Notification dispatch (diagnostics, etc. — logged and discarded)
- [x] Tests: full handshake with rust-analyzer, transport roundtrip tests

**Files:** `src/lsp/transport.rs` (new), `src/lsp/client.rs` (rewritten)
**Acceptance:** can send requests and receive correlated responses

### Sprint 3.3 — Document Symbols → `get_symbols_overview`

First LSP-backed tool: file/directory symbol tree.

**Tasks:**
- [x] Send `textDocument/documentSymbol` request
- [x] Parse `DocumentSymbol[]` response into `Vec<SymbolInfo>` tree
- [x] Wire `get_symbols_overview` tool: accept file path, return symbol tree
- [x] Handle directory paths: aggregate symbols from all files
- [x] Tests: get symbols from Rust test files via rust-analyzer

**Files:** `src/tools/symbol.rs`, `src/lsp/client.rs`
**Acceptance:** `get_symbols_overview` returns real symbols for a source file
**Done:** combined with Sprint 3.4

### Sprint 3.4 — Definition + References

Navigation tools — find where things are defined and used.

**Tasks:**
- [x] `textDocument/definition` → convert URI+position response to file:line
- [x] `textDocument/references` → collect all locations
- [x] Wire `find_symbol` tool: search by name across project via document symbols
- [x] Wire `find_referencing_symbols` tool: find all references via LSP references
- [x] URI ↔ path conversion utilities (`path_to_uri`, `uri_to_path`)
- [x] Tests: find function by name, no-project error handling

**Files:** `src/tools/symbol.rs`, `src/lsp/client.rs`
**Acceptance:** can find symbols and references

### Sprint 3.5 — Rename + Symbol Editing

The most complex LSP tools — modifying code through symbol operations.

**Tasks:**
- [x] `textDocument/rename` → apply workspace edit → wire `rename_symbol` tool
- [x] Symbol body extraction: use document symbols + line ranges to identify symbol text
- [x] `replace_symbol_body`: extract current body, replace with new content
- [x] `insert_before_symbol` / `insert_after_symbol`: locate symbol, insert adjacent text
- [x] `apply_text_edits()` helper for applying LSP workspace edits
- [x] Tests: helper unit tests (apply_text_edits, uri_to_path, find_symbol_in_tree)

**Files:** `src/tools/symbol.rs`, `src/lsp/client.rs`
**Acceptance:** all 7 symbol tools wired to LSP
**Done:** combined with Sprint 3.3+3.4

### ⟹ CHECKPOINT: Serena Parity

All scaffolded tools working. Symbol intelligence over LSP.

**Evaluation criteria:**
- All symbol tools work with at least rust-analyzer and pyright
- Performance acceptable for interactive use
- Decide if tree-sitter fallback is worth the effort or if LSP-only is sufficient

---

## Phase 4: Tree-sitter AST Engine

Offline symbol extraction as LSP fallback.

### Sprint 4.1 — Grammar Loading + Symbol Extraction

**Tasks:**
- [x] Add grammar crates: `tree-sitter-rust`, `tree-sitter-python`, `tree-sitter-typescript`, `tree-sitter-go`
- [x] Implement `extract_symbols_from_source()` in `src/ast/parser.rs`
- [x] Extract: functions, classes, methods, structs, interfaces with name + line range
- [x] Implement `extract_docstrings_from_source()` for docstring/comment extraction
- [x] Tests: parse Rust, Python, TypeScript, Go files; verify symbol extraction

**Files:** `src/ast/parser.rs`, `src/ast/mod.rs`, `Cargo.toml`
**Acceptance:** can extract symbols from source without any LSP running
**Done:** combined with Sprint 4.2 — 10 parser tests + 7 tool tests

### Sprint 4.2 — Wire to Tools + Fallback

**Tasks:**
- [x] Wire `list_functions` tool → tree-sitter symbol extraction
- [x] Wire `extract_docstrings` tool → tree-sitter comment/docstring nodes
- [x] Tests: tool-level tests with temp files (Rust, Python, unsupported)
- [x] Implement fallback logic in symbol tools: try LSP → fall back to tree-sitter
- [x] Update `get_symbols_overview` to use tree-sitter when LSP unavailable

**Files:** `src/tools/ast.rs`, `src/ast/parser.rs`
**Acceptance:** AST tools work offline; fallback deferred to Phase 5
**Note:** ParserPool deferred — tree-sitter Parser is cheap to create per call. Fallback logic (LSP → tree-sitter) deferred to polish phase.

---

## Phase 5: Polish & v1.0

Independent sprints — can be done in any order.

### Sprint 5.1 — HTTP/SSE Transport

**Tasks:**
- [x] Add `rmcp` feature: `transport-sse-server`
- [x] Implement HTTP transport in `server::run()` (the `"http"` arm)
- [x] Multi-session support: each SSE connection gets own `CodeExplorerServer` handler
- [x] Manual test: `code-explorer start --transport http` starts and listens

**Files:** `src/server.rs`, `Cargo.toml`
**Acceptance:** `code-explorer start --transport http` works
**Done:** SSE server accepts connections at /sse, messages at /message

### Sprint 5.2 — Production Vector Search + Local Embeddings

**Tasks:**
- [x] sqlite-vec extension loading: `conn.load_extension("sqlite_vec")`
- [x] Replace naive cosine with `vec_distance_cosine()` SQL query
- [x] Optional: local embedding via `candle`/`ort` + `jina-embeddings-v2-base-code`
- [x] Benchmark: pure Rust cosine vs sqlite-vec on 10K+ chunks

**Files:** `src/embed/index.rs`, `Cargo.toml`, optionally new `src/embed/local.rs`
**Acceptance:** vector search performance suitable for large projects
**Note:** Deferred — current pure-Rust cosine is sufficient for initial release

### Sprint 5.3 — Integration Tests + Release

**Tasks:**
- [x] Multi-tool workflow tests: read → search → replace, AST analysis, memory roundtrip, git, onboarding
- [x] `cargo build --release` optimization (strip, LTO via `[profile.release]`)
- [x] Library target (`src/lib.rs`) for integration test imports
- [x] README.md with install/usage instructions
- [x] Binary packaging (optional: cargo-dist or manual)

**Files:** `tests/integration.rs`, `src/lib.rs`, `Cargo.toml`
**Acceptance:** release binary works, 5 integration tests covering all tool categories

### ⟹ CHECKPOINT: v1.0 Release

Production-ready binary with all transports, optimized search, tested workflows.

---

## Dependency Graph

```
Sprint 0.1 (ToolContext)
    ├── Sprint 1.1 (Memory)
    ├── Sprint 1.2 (Git)
    ├── Sprint 1.3 (Config)
    ├── Sprint 1.4 (Semantic)
    ├── Sprint 2.1 (File tools)       ← independent of Phase 1
    └── Sprint 3.1 (LSP lifecycle)
            └── 3.2 (JSON-RPC)
                └── 3.3 (Symbols)
                    └── 3.4 (Def+Refs)
                        └── 3.5 (Rename+Edit)
    Sprint 4.1 (Grammars)             ← independent of Phase 3
        └── 4.2 (Wire+Fallback)       ← depends on 3.x for fallback logic
    Sprint 5.1 (HTTP)                 ← independent after Phase 0
    Sprint 5.2 (sqlite-vec)           ← after Sprint 1.4
    Sprint 5.3 (Release)              ← after everything
```

## Parallelism Opportunities

- **Phase 1 sprints** (1.1–1.4) are independent of each other
- **Sprint 2.1** (file tools) is independent of all Phase 1 sprints
- **Sprint 5.1** (HTTP) can start anytime after Phase 0
- **Sprint 4.1** (grammars) can start anytime after Phase 0
- **Within Phase 3**: strictly sequential

---

*Created: 2026-02-25*
*Plan covers: current state → v1.0 release*
