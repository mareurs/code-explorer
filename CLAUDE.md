# code-explorer

Rust MCP server giving LLMs IDE-grade code intelligence — symbol-level navigation, semantic search, git integration. Inspired by [Serena](https://github.com/oraios/serena).

## Development Commands

```bash
cargo build                        # Build
cargo test                         # Run tests (862 passing)
cargo clippy -- -D warnings        # Lint
cargo fmt                          # Format
cargo run -- start --project .     # Run MCP server (stdio)
cargo run -- index --project .     # Build embedding index
```

**Always run `cargo fmt`, `cargo clippy`, and `cargo test` before completing any task.**

## Tool Misbehavior Log — MANDATORY

**`docs/TODO-tool-misbehaviors.md` is a living document. You MUST maintain it.**

- **Before starting any task**, read it to know current tool limitations.
- **While working**, watch for: wrong edits, corrupt output, silent failures, misleading errors from code-explorer's own MCP tools.
- **When you notice anything unexpected**, add an entry to that file **before continuing** — even a one-liner. Capture: what you did, what you expected, what happened, and a probable cause.
- Do not wait until you finish the task. Log it immediately while context is fresh.

This applies to ALL unexpected tool behavior: `edit_file`, `rename_symbol`, `replace_symbol`, `find_symbol`, `semantic_search`, etc.


## Git Workflow

**This is a public repo.** Do not push incomplete or untested work.

- **Batch related changes** into a single well-tested commit rather than committing every incremental step.
- **Only commit when the full fix/feature is working** — all tests pass, clippy clean, manually verified if applicable.
- **Do not push after every commit.** Accumulate local commits during a work session; push once when the work is solid.
- When iterating on a fix (e.g. debugging a concurrency issue), keep working locally until the fix is confirmed, then commit the final state — not every intermediate attempt.

## Project Structure

```
src/
├── main.rs          # CLI: start (MCP server) and index subcommands
├── server.rs        # rmcp ServerHandler — bridges Tool trait to MCP, signal handling + graceful LSP shutdown
├── agent.rs         # Orchestrator: active project, config, memory
├── config/          # ProjectConfig (.code-explorer/project.toml), modes
├── lsp/             # LSP types, server configs (9 langs), JSON-RPC client
├── ast/             # Language detection (20+ exts), tree-sitter parser
├── git/             # git2: blame, file_log, open_repo
├── embed/           # Chunker, SQLite index, RemoteEmbedder, schema, drift detection
├── library/         # LibraryRegistry, Scope enum, manifest discovery
├── memory/          # Markdown-based MemoryStore (.code-explorer/memories/)
├── prompts/         # LLM guidance: server_instructions.md, onboarding_prompt.md
├── tools/           # Tool implementations by category
│   ├── output.rs    #   OutputGuard: progressive disclosure (exploring/focused)
│   ├── format.rs    #   Shared format helpers (format_line_range, format_overflow, truncate_path)
│   ├── file.rs      #   read_file, list_dir, search_pattern, create_file, find_file, edit_file
│   ├── workflow.rs  #   onboarding, run_command
│   ├── symbol.rs    #   10 LSP-backed tools (find_symbol, list_symbols, goto_definition, hover, remove_symbol, etc.)
│   ├── git.rs       #   git_blame
│   ├── semantic.rs  #   semantic_search, index_project, index_status (incl. drift)
│   ├── library.rs   #   list_libraries, index_library
│   ├── memory.rs    #   CRUD tools (write/read/list/delete)
│   ├── ast.rs       #   list_functions, list_docs
│   └── config.rs    #   activate_project, get_config
└── util/            # fs helpers, text processing
```

## Design Principles

**Progressive Disclosure & Discoverability** — Every tool defaults to the most
compact useful representation. Details are available on demand via
`detail_level: "full"` + pagination. When results overflow, responses include
actionable hints and file distribution maps (`by_file`). See
`docs/PROGRESSIVE_DISCOVERABILITY.md` for the canonical patterns and
anti-patterns — **read it before adding or modifying any tool**.

**Token Efficiency** — The LLM's context window is a scarce resource. Tools
minimize output by default: names + locations in exploring mode, full bodies
only in focused mode. Overflow produces actionable guidance ("showing N of M,
narrow with..."), not truncated garbage.

**No Echo in Write Responses** — Mutation tools (`create_file`, `edit_file`,
`replace_symbol`, etc.) must never echo back what the LLM just sent. The caller
already knows the path, content, and size — reflecting them wastes tokens with
zero information gain. The only new information after a write is success/failure.
Return `json!("ok")` for writes; reserve richer responses for cases where the
tool discovers genuinely new information (e.g. LSP diagnostics after a write).

**Two Modes** — `Exploring` (default): compact, capped at 200 items. `Focused`:
full detail, paginated via offset/limit. Enforced via `OutputGuard`
(`src/tools/output.rs`), a project-wide pattern not per-tool logic.

**Tool Selection by Knowledge Level** — Know the name → LSP/AST tools
(`find_symbol`, `list_symbols`, `goto_definition`, `hover`). Know the concept →
semantic search first, then drill down. Know nothing → `list_dir` +
`list_symbols` at top level, then semantic search.

## Testing Patterns

**Cache-invalidation tests use a three-query sandwich** — not two. The structure is:
1. Query → record baseline state
2. Mutate the underlying data (disk, cache, external system) without going through the normal notification path
3. Query again → assert result is **stale** (same as baseline) — this proves the bug exists
4. Trigger the invalidation (e.g. `did_change`, cache flush)
5. Query again → assert result is **fresh** (reflects the mutation)

A two-query test (baseline → post-invalidation) only confirms the happy path. The stale-assertion in step 3 is what makes it a *regression* test — it will fail if the underlying system ever changes to eagerly re-read on every query, alerting you that the invalidation logic has become wrong or unnecessary.

See `did_change_refreshes_stale_symbol_positions` in `src/lsp/client.rs` for the canonical example.

## Key Patterns

**Tool trait** (`src/tools/mod.rs`): Each tool is a struct implementing `name()`, `description()`, `input_schema()`, `async call(Value, &ToolContext) -> Result<Value>`. 30 tools registered. All use `#[async_trait]`.

**Tool↔MCP bridge** (`src/server.rs`): Tools registered as `Vec<Arc<dyn Tool>>`, dispatched dynamically in `call_tool`. Errors are routed through `route_tool_error`:
- `RecoverableError` (`src/tools/mod.rs`) → `isError: false` with JSON `{"error":"…","hint":"…"}` — LLM sees the problem and a corrective hint, **sibling parallel calls are not aborted by Claude Code**.
- Any other `anyhow::Error` → `isError: true` (fatal; something truly broke).

Use `RecoverableError` for expected, input-driven failures (path not found, unsupported file type, empty glob). Use plain `anyhow::bail!` for genuine tool failures (LSP crash, security violation, programming error).

**Config** (`.code-explorer/project.toml`): Per-project settings including embedding model, chunk size, ignored paths. `ProjectConfig::load_or_default()` handles missing config gracefully.

**Embedding pipeline**: `chunker::split()` → `RemoteEmbedder::embed()` → `index::insert_chunk()` → `index::search()` (cosine similarity). All stored in `.code-explorer/embeddings.db`. Incremental updates via `find_changed_files()`: git diff → mtime → SHA-256 fallback chain. `semantic_search` warns when the index is behind HEAD.

## Companion Plugin: code-explorer-routing

This project has a companion Claude Code plugin at **`../claude-plugins/code-explorer-routing/`** that is **always active** when working on code-explorer. You must be aware of it.

**What it does:**
- `SessionStart` hook (`hooks/session-start.sh`) — injects tool guidance + memory hints into every session
- `SubagentStart` hook (`hooks/subagent-guidance.sh`) — same for all subagents
- `PreToolUse` hook on `Grep|Glob|Read` (`hooks/semantic-tool-router.sh`) — **blocks native Read/Grep/Glob on source files**, redirecting to code-explorer MCP tools

**Critical implication for working on this codebase:**
The `PreToolUse` hook will **block** any attempt to use the native `Read`, `Grep`, or `Glob` tools on source code files (`.rs`, `.ts`, `.py`, etc). You will see `PreToolUse:Read hook error` if you try.

**You MUST use code-explorer's own MCP tools to read source code:**
- `mcp__code-explorer__list_symbols(path)` — see all symbols in a file/dir
- `mcp__code-explorer__find_symbol(name, include_body=true)` — read a function body
- `mcp__code-explorer__list_functions(path)` — quick signatures
- `mcp__code-explorer__search_pattern(pattern)` — regex search
- `mcp__code-explorer__semantic_search(query)` — concept-level search
- `mcp__code-explorer__read_file(path)` — for non-source files (markdown, toml, json)

**Configuration:**
- Auto-detects code-explorer from `.mcp.json` or `~/.claude/settings.json`
- Can be overridden via `.claude/code-explorer-routing.json`
- `block_reads: false` in that config to disable blocking (dev/debug use)

## Docs

- **`docs/PROGRESSIVE_DISCOVERABILITY.md`** — Canonical guide for output sizing, overflow hints, and agent guidance patterns. **READ THIS before adding or modifying any tool.**
- `docs/plans/2026-02-25-v1-implementation-plan.md` — Sprint-level plan (Phase 0–5, 15 sprints)
- `docs/ARCHITECTURE.md` — Component details, tech stack, design principles
- `docs/ROADMAP.md` — Quick status overview
