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

### Documenting Features on `experiments`

When adding a feature commit to `experiments`, you MUST include documentation in the same commit:

1. Create `docs/manual/src/experimental/<feature-name>.md` — written as final user-facing
   docs with a single `> ⚠ Experimental — may change without notice.` callout at the top.
2. Add a line to `docs/manual/src/experimental/index.md` linking to the new page.

**Only features, not bug fixes.** Bug fixes need no experimental doc.

**If a feature is removed from `experiments`** (reverted or abandoned), delete its page and
remove its entry from `index.md` in the same commit.

### Graduating a Feature (`experiments` → `master`)

When cherry-picking a feature to `master`, use `--no-commit` to bundle the doc graduation
into the same commit:

```bash
git cherry-pick --no-commit <sha>
# then make the four graduation changes:
# 1. Move docs/manual/src/experimental/<feature-name>.md to its target chapter
# 2. Remove the `> ⚠ Experimental` callout from the top of the page
# 3. Add the page to docs/manual/src/SUMMARY.md in the right place
# 4. Remove the feature's entry from docs/manual/src/experimental/index.md
git commit -m "feat(...): <description>"
```

**Rebase note:** Because the graduation commit on `master` includes additional doc changes,
its patch differs from the original `experiments` commit. Git will **not** auto-skip it
during the subsequent `git rebase master` on `experiments`. After rebasing, drop the
now-superseded original commit manually:

```bash
git checkout experiments
git rebase master          # the original feature commit will NOT be auto-dropped
git rebase -i master       # drop the original feature commit from the list
```

### Publishing to crates.io

- **Always publish from `master`**, never from `experiments` or feature branches.
- Bump the version in `Cargo.toml` on master, commit it, then run `cargo publish`.
- Tag every release: `git tag vX.Y.Z` on the publish commit, then `git push --tags`.
- Token is stored in `.env` (gitignored): `CARGO_REGISTRY_TOKEN=...` — load with `CARGO_REGISTRY_TOKEN=$(grep CARGO_REGISTRY_TOKEN .env | cut -d= -f2) cargo publish`.

### Standard Ship Sequence

When a bug fix or tested feature on `experiments` is ready to land in `master`:

```bash
# 1. Commit on experiments (tests passing, clippy clean)
git add <files> && git commit -m "..."

# 2. Cherry-pick to master and push
git checkout master
git cherry-pick <commit-sha>
git push

# 3. Rebase experiments back on master (drops the cherry-picked commit automatically)
git checkout experiments
git rebase master
```

This is the default workflow for all completed work. The rebase step keeps `experiments`
clean — git detects the cherry-pick and skips the duplicate commit automatically.

### Commit Discipline

- **Batch related changes** into a single well-tested commit rather than committing every incremental step.
- **Only commit when the full fix/feature is working** — all tests pass, clippy clean, manually verified if applicable.
- **Do not push after every commit.** Accumulate local commits during a work session; push once when the work is solid.
- When iterating on a fix, keep working locally until the fix is confirmed, then commit the final state — not every intermediate attempt.

## Project Structure

See codescout memory `architecture` (Source Tree section).

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

See codescout memory `language-patterns` (Rust section) for anti-patterns and idiomatic patterns.

## Docs

- **`docs/PROGRESSIVE_DISCOVERABILITY.md`** — Canonical guide for output sizing, overflow hints, and agent guidance patterns. **READ THIS before adding or modifying any tool.**
- `docs/plans/2026-02-25-v1-implementation-plan.md` — Sprint-level plan (Phase 0–5, 15 sprints)
- `docs/ARCHITECTURE.md` — Component details, tech stack, design principles
- `docs/ROADMAP.md` — Quick status overview
