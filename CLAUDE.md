# codescout

Rust MCP server giving LLMs IDE-grade code intelligence — symbol-level navigation, semantic search, git integration. Inspired by [Serena](https://github.com/oraios/serena).

You are a proficient Rust developer. You follow all known good/scalable patterns. You are honest and recognize your limits and your mistakes, you own them. If you are not sure, you always ask me for feedback.

## Development Commands

```bash
cargo build                        # Build (dev)
cargo build --release              # Build release binary (required before testing via MCP)
cargo test                         # Run tests
cargo clippy -- -D warnings        # Lint
cargo fmt                          # Format
cargo run -- start --project .     # Run MCP server (stdio)
cargo run -- index --project .     # Build embedding index
```

**Always run `cargo fmt`, `cargo clippy`, and `cargo test` before completing any task.**

**To test changes via the live MCP server, always run `cargo build --release` first**, then restart the server with `/mcp`. The MCP server runs the release binary — dev builds are not picked up.

## Tool Misbehavior Log — MANDATORY

**`docs/TODO-tool-misbehaviors.md` is a living document. You MUST maintain it.**

- **Before starting any task**, read it to know current tool limitations.
- **While working**, watch for: wrong edits, corrupt output, silent failures, misleading errors from codescout's own MCP tools.
- **When you notice anything unexpected**, add an entry to that file **before continuing** — even a one-liner. Capture: what you did, what you expected, what happened, and a probable cause.
- Do not wait until you finish the task. Log it immediately while context is fresh.

This applies to ALL unexpected tool behavior: `edit_file`, `rename_symbol`, `replace_symbol`, `find_symbol`, `semantic_search`, etc.


## Git Workflow

**This is a public repo.** Do not push incomplete or untested work.

### Branch Strategy

- **`master` is protected.** Only cherry-picked, thoroughly tested commits land here.
- **All experimental work goes on the `experiments` branch** (or a dedicated feature branch). Iterate freely there.
- **Cherry-pick to `master`** only after: all tests pass, clippy clean, manually verified via MCP (`cargo build --release` + `/mcp` restart).
- Never commit directly to `master` for in-progress or exploratory work.

### Publishing to crates.io

- **Always publish from `master`**, never from `experiments` or feature branches.
- Bump the version in `Cargo.toml` on master, commit it, then run `cargo publish`.
- Tag every release: `git tag vX.Y.Z` on the publish commit, then `git push --tags`.
- Token is stored in `.env` (gitignored): `CARGO_REGISTRY_TOKEN=...` — load with `CARGO_REGISTRY_TOKEN=$(grep CARGO_REGISTRY_TOKEN .env | cut -d= -f2) cargo publish`.

### Commit Discipline

- **Batch related changes** into a single well-tested commit rather than committing every incremental step.
- **Only commit when the full fix/feature is working** — all tests pass, clippy clean, manually verified if applicable.
- **Do not push after every commit.** Accumulate local commits during a work session; push once when the work is solid.
- When iterating on a fix, keep working locally until the fix is confirmed, then commit the final state — not every intermediate attempt.

## Project Structure

```
src/
├── main.rs          # CLI: start (MCP server), index, and dashboard subcommands
├── lib.rs           # Crate root for library/integration use
├── server.rs        # rmcp ServerHandler — bridges Tool trait to MCP, signal handling + graceful LSP shutdown
├── agent.rs         # Orchestrator: active project, config, memory
├── logging.rs       # --debug mode: file logging with rotation (tracing-appender)
├── config/          # ProjectConfig (.codescout/project.toml), modes
├── lsp/             # LSP types, server configs (9 langs), JSON-RPC client
├── ast/             # Language detection (20+ exts), tree-sitter parser
├── git/             # git2: blame, file_log, open_repo
├── embed/           # Chunker, SQLite index, RemoteEmbedder, schema, drift detection
├── library/         # LibraryRegistry, Scope enum, manifest discovery
├── memory/          # Markdown-based MemoryStore (.codescout/memories/)
├── usage/           # UsageRecorder: append-only SQLite call stats (usage.db)
├── prompts/         # LLM guidance: server_instructions.md, onboarding_prompt.md
├── tools/           # Tool implementations by category
│   ├── output.rs          #   OutputGuard: progressive disclosure (exploring/focused)
│   ├── output_buffer.rs   #   OutputBuffer: session-scoped LRU (@cmd_*/@file_* handles)
│   ├── progress.rs        #   ProgressReporter: MCP progress notifications
│   ├── format.rs          #   Shared format helpers (format_line_range, format_overflow, truncate_path)
│   ├── file.rs            #   read_file, list_dir, search_pattern, create_file, find_file, edit_file
│   ├── file_summary.rs    #   Smart per-type summarizers (source, markdown, JSON, TOML, YAML)
│   ├── workflow.rs        #   onboarding, run_command
│   ├── symbol.rs          #   9 LSP-backed tools (find_symbol, list_symbols, goto_definition, hover, remove_symbol, etc.)
│   ├── git.rs             #   git_blame, file_log (not registered; used by dashboard)
│   ├── semantic.rs        #   semantic_search, index_project, index_status
│   ├── github.rs          #   github_identity, github_issue, github_pr, github_file, github_repo
│   ├── library.rs         #   list_libraries
│   ├── memory.rs          #   memory (action: read/write/list/delete/remember/recall/forget/refresh_anchors)
│   ├── usage.rs           #   GetUsageStats (dashboard API; not an MCP tool)
│   ├── ast.rs             #   list_functions, list_docs (not registered; tree-sitter offline tools)
│   ├── command_summary.rs #   Smart output summarization, terminal filter detection
│   └── config.rs          #   activate_project, project_status
└── util/            # fs helpers, text processing, path security
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

**Tool trait** (`src/tools/mod.rs`): Each tool is a struct implementing `name()`, `description()`, `input_schema()`, `async call(Value, &ToolContext) -> Result<Value>`. 29 tools registered. All use `#[async_trait]`.

**Tool↔MCP bridge** (`src/server.rs`): Tools registered as `Vec<Arc<dyn Tool>>`, dispatched dynamically in `call_tool`. Errors are routed through `route_tool_error`:
- `RecoverableError` (`src/tools/mod.rs`) → `isError: false` with JSON `{"error":"…","hint":"…"}` — LLM sees the problem and a corrective hint, **sibling parallel calls are not aborted by Claude Code**.
- Any other `anyhow::Error` → `isError: true` (fatal; something truly broke).

Use `RecoverableError` for expected, input-driven failures (path not found, unsupported file type, empty glob). Use plain `anyhow::bail!` for genuine tool failures (LSP crash, security violation, programming error).

**`ToolContext`** fields: `agent` (project state + config access), `lsp` (LSP client pool), `output_buffer` (session-scoped `@cmd_*`/`@file_*` handle store), `progress` (MCP progress reporter).

**Config** (`.codescout/project.toml`): Per-project settings including embedding model, chunk size, ignored paths. `ProjectConfig::load_or_default()` handles missing config gracefully.

**Embedding pipeline**: `chunker::split()` → `RemoteEmbedder::embed()` → `index::insert_chunk()` → `index::search()` (cosine similarity). All stored in `.codescout/embeddings.db`. Incremental updates via `find_changed_files()`: git diff → mtime → SHA-256 fallback chain. `semantic_search` warns when the index is behind HEAD.

## Prompt Surface Consistency

The project has **three prompt surfaces** that reference tool names:
- `src/prompts/server_instructions.md` — injected every MCP request
- `src/prompts/onboarding_prompt.md` — one-time onboarding
- `build_system_prompt_draft()` in `src/tools/workflow.rs` — generated per-project

**When tools get renamed/consolidated, all three need coordinated updates.** Files
closer to the change get updated; distant ones accumulate stale refs ("distance
from change" problem). Always grep all three surfaces when modifying tool names.

## Companion Plugin: code-explorer-routing

This project has a companion Claude Code plugin at **`../claude-plugins/code-explorer-routing/`** that is **always active** when working on codescout. You must be aware of it.

**What it does:**
- `SessionStart` hook (`hooks/session-start.sh`) — injects tool guidance + memory hints into every session
- `SubagentStart` hook (`hooks/subagent-guidance.sh`) — same for all subagents
- `PreToolUse` hook on `Grep|Glob|Read` (`hooks/semantic-tool-router.sh`) — **blocks native Read/Grep/Glob on source files**, redirecting to codescout MCP tools

**Critical implication for working on this codebase:**
The `PreToolUse` hook will **block** any attempt to use the native `Read`, `Grep`, or `Glob` tools on source code files (`.rs`, `.ts`, `.py`, etc). You will see `PreToolUse:Read hook error` if you try.

**You MUST use codescout's own MCP tools to read source code:**
- `mcp__codescout__list_symbols(path)` — see all symbols in a file/dir
- `mcp__codescout__find_symbol(name, include_body=true)` — read a function body
- `mcp__codescout__search_pattern(pattern)` — regex search
- `mcp__codescout__semantic_search(query)` — concept-level search
- `mcp__codescout__read_file(path)` — for non-source files (markdown, toml, json)

**Configuration:**
- Auto-detects codescout from `.mcp.json` or `~/.claude/settings.json`
- Can be overridden via `.claude/code-explorer-routing.json`
- `block_reads: false` in that config to disable blocking (dev/debug use)

## Rust Coding Standards

### Ownership first
- Restructure data and function signatures to satisfy the borrow checker.
  Never reach for `.clone()` as a first resort — only clone when semantically
  appropriate (the clone represents intentional duplication, not a workaround).
- Design ownership topology before writing implementations. Ask: who owns this
  data, and what borrows it?

### Error handling
- All fallible functions return `Result<T, E>`. Never use `.unwrap()` in
  library code. In application code, only use `.unwrap()` where a panic is
  genuinely the correct behavior and document why.
- Use `anyhow` for application-level error propagation, `thiserror` for
  library error types.
- Propagate errors with `?`. Avoid nested match blocks for error handling.

### Iterators over loops
- Prefer iterator adapters (`.map()`, `.filter()`, `.fold()`, `.collect()`)
  over explicit `for` loops when the intent is a transformation pipeline.
- Use `.iter()` / `.iter_mut()` / `.into_iter()` correctly — do not
  implicitly rely on auto-deref behavior.

### Generics vs trait objects
- Default to generics (`fn foo<T: Trait>(x: T)`) for zero-cost dispatch.
- Only use `Box<dyn Trait>` / `Arc<dyn Trait>` when you need runtime
  polymorphism (heterogeneous collections, plugin systems, dynamic dispatch).
  Always comment why dynamic dispatch is needed.

### Concurrency
- Before reaching for `Arc<Mutex<T>>`, ask whether ownership can be
  structured to avoid shared state entirely.
- Prefer message passing (channels) over shared state for complex coordination.
- Use `tokio` for async I/O. Never block inside async functions.

### Clippy
- All code must pass `cargo clippy -- -D warnings`.
- When clippy suggests an alternative, prefer it unless there's a documented
  reason not to.

### Lifetimes
- Annotate lifetimes explicitly when the compiler cannot infer them and
  explain the relationship in a comment.
- Prefer owned types in struct fields over references where the struct
  needs to be self-contained or sent across threads.

### unsafe
- Never write `unsafe` without a `// SAFETY:` comment explaining the
  invariants being upheld.
- Prefer safe abstractions. Reach for `unsafe` only when there is no
  safe alternative and the performance gain is measured and documented.

## Docs

- **`docs/PROGRESSIVE_DISCOVERABILITY.md`** — Canonical guide for output sizing, overflow hints, and agent guidance patterns. **READ THIS before adding or modifying any tool.**
- `docs/plans/2026-02-25-v1-implementation-plan.md` — Sprint-level plan (Phase 0–5, 15 sprints)
- `docs/ARCHITECTURE.md` — Component details, tech stack, design principles
- `docs/ROADMAP.md` — Quick status overview
