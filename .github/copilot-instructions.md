# code-explorer

Rust MCP server giving LLMs IDE-grade code intelligence — symbol navigation, semantic search, git blame, shell integration, and persistent memory. Built for [Claude Code](https://code.claude.com/).

## Build, Test, and Lint

```bash
# Build
cargo build

# Test (862 tests)
cargo test                        # All tests
cargo test test_name              # Single test by name
cargo test module_name            # All tests in a module

# E2E tests (require LSP servers installed)
cargo test --features e2e         # All E2E tests
cargo test --features e2e-rust    # Rust E2E tests only

# Lint & format
cargo clippy -- -D warnings       # Lint (must be clean)
cargo fmt                         # Format
```

**Pre-commit requirement:** All three must pass before completing any task:
```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

## Running the Server

```bash
# Start MCP server (stdio transport)
cargo run -- start --project .

# Build embedding index
cargo run -- index --project .

# Launch dashboard (web UI on port 8099)
cargo run -- dashboard --project .
```

## High-Level Architecture

### Component Flow

```
Claude Code (MCP client)
    ↓
MCP Server (src/server.rs) — bridges Tool trait to rmcp
    ↓
Agent (src/agent.rs) — orchestrator: active project, config, memory
    ↓
┌────────────┬─────────────┬────────────┬──────────────┐
LSP Client   AST Engine    Git Engine   Embedding      
(30+ langs)  (tree-sitter) (git2-rs)    Engine         
                                         (sqlite-vec)   
```

### Tool System

Each tool implements the `Tool` trait (`src/tools/mod.rs`):
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    async fn call(&self, params: Value, ctx: &ToolContext) -> Result<Value>;
}
```

32 tools registered in `src/server.rs` as `Vec<Arc<dyn Tool>>`, dispatched dynamically in `call_tool`.

**Error routing** (`route_tool_error` in `src/server.rs`):
- `RecoverableError` → `isError: false` with JSON `{"error":"…","hint":"…"}` — LLM sees the problem and correction, **sibling parallel calls are not aborted**
- Other errors → `isError: true` (fatal)

Use `RecoverableError` for expected failures (path not found, unsupported file type). Use `anyhow::bail!` for genuine tool failures (LSP crash, security violation).

### Progressive Disclosure

**Every tool defaults to compact output.** Full detail requires `detail_level: "full"`.

Two modes enforced via `OutputGuard` (`src/tools/output.rs`):
- **Exploring** (default): compact, capped at 200 items, no function bodies
- **Focused** (`detail_level: "full"`): full detail, paginated via `offset`/`limit`

When overflow occurs, responses include:
- Total count
- Actionable hints ("to narrow: add path=..., kind=...")
- `by_file` distribution map

**Read `docs/PROGRESSIVE_DISCOVERABILITY.md` before adding or modifying any tool.** It's the canonical guide for output sizing, overflow hints, and agent guidance patterns.

### Write Response Convention

**Mutation tools never echo back what the LLM just sent.** The caller already knows the path, content, and size — reflecting them wastes tokens.

Return `json!("ok")` for successful writes. Only include additional info if the tool *discovers* something new (e.g., LSP diagnostics after a write).

Applies to: `create_file`, `edit_lines`, `replace_symbol`, `insert_code`, `rename_symbol`.

### Embedding Pipeline

`chunker::split()` → `RemoteEmbedder::embed()` → `index::insert_chunk()` → `index::search()`

- Storage: `.code-explorer/embeddings.db` (SQLite)
- Change detection: git diff → mtime → SHA-256 fallback chain
- Drift detection: `compute_file_drift()` tracks semantic changes during re-indexing (opt-out via `drift_detection_enabled = false`)
- Staleness warnings: `semantic_search` warns when index is behind HEAD

### LSP Integration

`LspClient` (`src/lsp/client.rs`):
- JSON-RPC transport over stdin/stdout
- Lifecycle: `initialize` → `initialized` → tool calls → `shutdown` → `exit`
- Stores `child_pid` for kill-on-drop safety (SIGTERM via `libc::kill` in `Drop`)
- Graceful shutdown: `shutdown_signal()` listens for SIGINT/SIGTERM, calls `lsp.shutdown_all()` before exit

9 languages with default configs (`src/lsp/servers/mod.rs`):
rust-analyzer, pyright, typescript-language-server, gopls, jdtls, kotlin-language-server, clangd, omnisharp, solargraph.

Install LSP servers:
```bash
./scripts/install-lsp.sh --check          # See what's installed
./scripts/install-lsp.sh --all            # Install everything
./scripts/install-lsp.sh rust python go   # Install specific languages
```

### Library Navigation

Third-party dependency source code is read-only but fully navigable.

- `LibraryRegistry` (`src/library/registry.rs`) — persists known library paths in `.code-explorer/libraries.json`
- `discover_library_from_path()` (`src/library/discovery.rs`) — auto-triggered when `goto_definition` returns a path outside project root
- Walks parent dirs to find manifests: `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`
- All symbol/semantic tools accept `scope` parameter: `Project`, `Library(name)`, `Libraries`, `All`

## Key Conventions

### Code Reading & Editing — Always Use code-explorer MCP

**Never use generic file tools (open file, read file, grep, find) on source code.** Always use the code-explorer MCP tools instead. This applies to all source files (`.rs`, `.ts`, `.py`, `.go`, etc.) for both reading and editing.

**Two-phase exploration rule:**
1. **Locate** — use `semantic_search` or `search_pattern` to find relevant files and concepts
2. **Drill down** — use dedicated symbol tools (`find_symbol`, `list_symbols`, `list_functions`, `goto_definition`, `hover`) to read specific code; **never** use `sed`, `grep`, `cat`, `awk`, or `read_file` on source files for drill-down

| Task | Use this tool |
|---|---|
| Read a function / class body | `find_symbol(name, include_body=true)` |
| List all symbols in a file or dir | `list_symbols(path)` |
| Quick function signatures | `list_functions(path)` |
| Jump to a definition | `goto_definition(path, line)` |
| Type info / docs at a position | `hover(path, line)` |
| Search by text / regex | `search_pattern(pattern)` |
| Search by concept | `semantic_search(query)` |
| Read non-source files (markdown, toml, json) | `read_file(path)` |
| Replace a function / struct body | `replace_symbol(name_path, path, new_body)` |
| Insert code before/after a symbol | `insert_code(name_path, path, code, position)` |
| Delete a symbol | `remove_symbol(name_path, path)` |
| Rename a symbol across the codebase | `rename_symbol(name_path, path, new_name)` |
| Non-structural edits (imports, strings, config) | `edit_file(path, old_string, new_string)` |

**Never do this for source code:**
- ❌ `sed -n '100,200p' src/foo.rs` → ✅ `find_symbol("MyStruct", include_body=true)`
- ❌ `grep -n "fn rename" src/` → ✅ `find_symbol("rename", kind="function")`
- ❌ `cat src/tools/symbol.rs` → ✅ `list_symbols("src/tools/symbol.rs")`
- ❌ `read_file(path, start_line, end_line)` on source → ✅ `find_symbol(name, include_body=true)`

Use `semantic_search` first when you don't know the exact name; drill into results with `find_symbol`. Use `list_dir` + `list_symbols` when exploring unknown territory.

### Three-Query Sandwich (Cache Invalidation Tests)

Cache-invalidation tests use **three queries**, not two:
1. Query → record baseline
2. Mutate underlying data *without* going through normal notification path
3. Query again → assert result is **stale** (same as baseline) — **proves the bug exists**
4. Trigger invalidation (e.g., `did_change`, cache flush)
5. Query again → assert result is **fresh** (reflects mutation)

Without step 3, you're only testing the happy path. The stale assertion is what makes it a regression test.

Example: `did_change_refreshes_stale_symbol_positions` in `src/lsp/client.rs`.

### Tool Misbehavior Log

`docs/TODO-tool-misbehaviors.md` is a **living document**.

- **Before starting any task**, read it to know current tool limitations
- **While working**, watch for wrong edits, corrupt output, silent failures, misleading errors
- **When you notice anything unexpected**, add an entry **immediately** — even a one-liner

Captures: what you did, what you expected, what happened, probable cause.

Applies to all MCP tools: `edit_file`, `rename_symbol`, `replace_symbol`, `find_symbol`, `semantic_search`, etc.

### Git Workflow

**This is a public repo.** Do not push incomplete or untested work.

- Batch related changes into a single commit
- Only commit when the full fix/feature is working (tests pass, clippy clean)
- Do not push after every commit — accumulate locally, push once when solid
- When iterating on a fix, commit the final state, not every intermediate attempt

### Code Review

**Review early, review often.** Catch issues before they compound.

**When to review:**
- After completing a major feature or multi-step plan
- Before merging to main
- When stuck (a fresh review surfaces assumptions)
- After fixing a complex or subtle bug

**How to review:**

1. Get the git range:
   ```bash
   BASE_SHA=$(git rev-parse origin/main)   # or HEAD~N for local commits
   HEAD_SHA=$(git rev-parse HEAD)
   git diff --stat $BASE_SHA..$HEAD_SHA
   git diff $BASE_SHA..$HEAD_SHA
   ```

2. Check against this list:
   - **Code quality:** clean separation of concerns, proper error handling, DRY, edge cases
   - **Architecture:** sound design, performance implications, security concerns
   - **Testing:** tests exercise real logic (not just mocks), edge cases covered, all passing
   - **Requirements:** all plan items met, no scope creep, breaking changes documented
   - **Production readiness:** no obvious bugs, backward compatibility, migrations if needed

3. Categorize findings by severity:
   - **Critical** — bugs, data loss, security, broken functionality → fix immediately
   - **Important** — architecture problems, missing features, poor error handling → fix before proceeding
   - **Minor** — style, optimization, docs → note for later

4. For each issue: specify file + line, explain what's wrong, why it matters, how to fix.

**Never skip review because "it's simple."** Never ignore Critical issues. Never proceed with unfixed Important issues.

### Companion Plugin: code-explorer-routing

**`../claude-plugins/code-explorer-routing/` is always active when working on this codebase.**

What it does:
- `SessionStart` hook — injects tool guidance + memory hints
- `SubagentStart` hook — propagates to all subagents
- `PreToolUse` hook on `Read|Grep|Glob` — **blocks native tools on source files**, redirects to code-explorer MCP tools

**Critical implication:** You cannot use native `Read`, `Grep`, or `Glob` on `.rs`, `.ts`, `.py`, etc. Use code-explorer's MCP tools instead:
- `mcp__code-explorer__list_symbols(path)` — see all symbols in a file/dir
- `mcp__code-explorer__find_symbol(name, include_body=true)` — read function body
- `mcp__code-explorer__list_functions(path)` — quick signatures
- `mcp__code-explorer__search_pattern(pattern)` — regex search
- `mcp__code-explorer__semantic_search(query)` — concept-level search
- `mcp__code-explorer__read_file(path)` — for non-source files (markdown, toml, json)

## Project Structure

```
src/
├── main.rs          # CLI: start (MCP server) and index subcommands
├── server.rs        # rmcp ServerHandler — bridges Tool trait to MCP
├── agent.rs         # Orchestrator: active project, config, memory
├── config/          # ProjectConfig, modes
├── lsp/             # LSP types, server configs (9 langs), JSON-RPC client
├── ast/             # Language detection (20+ exts), tree-sitter parser
├── git/             # git2: blame, file_log, open_repo
├── embed/           # Chunker, SQLite index, RemoteEmbedder, drift detection
├── library/         # LibraryRegistry, Scope enum, manifest discovery
├── memory/          # Markdown-based MemoryStore (.code-explorer/memories/)
├── prompts/         # LLM guidance: server_instructions.md, onboarding_prompt.md
├── tools/           # Tool implementations by category
│   ├── output.rs    #   OutputGuard: progressive disclosure
│   ├── format.rs    #   Shared format helpers
│   ├── file.rs      #   read_file, list_dir, search_pattern, create_file, etc.
│   ├── workflow.rs  #   onboarding, run_command
│   ├── symbol.rs    #   LSP-backed tools (find_symbol, goto_definition, etc.)
│   ├── git.rs       #   git_blame
│   ├── semantic.rs  #   semantic_search, index_project, index_status
│   ├── library.rs   #   list_libraries, index_library
│   ├── memory.rs    #   write/read/list/delete memory
│   ├── ast.rs       #   list_functions, list_docs
│   └── config.rs    #   activate_project, get_config
└── util/            # fs helpers, text processing
```

## Documentation

- **`docs/PROGRESSIVE_DISCOVERABILITY.md`** — **READ THIS** before adding or modifying any tool
- `docs/ARCHITECTURE.md` — Component details, tech stack
- `docs/ROADMAP.md` — Quick status overview
- `docs/TODO-tool-misbehaviors.md` — **Mandatory log** for unexpected tool behavior
- `docs/manual/` — User-facing documentation (installation, tools, semantic search)

## Config

Per-project settings in `.code-explorer/project.toml`:
- Embedding model (OpenAI, Ollama, custom API)
- Chunk size for semantic search
- Ignored paths
- Drift detection toggle

`ProjectConfig::load_or_default()` handles missing config gracefully.
