# Roadmap

See the detailed implementation plan: [`plans/2026-02-25-v1-implementation-plan.md`](plans/2026-02-25-v1-implementation-plan.md)

## Quick Status

| Phase | Description | Sprints | Status |
|-------|-------------|---------|--------|
| 0 | Architecture Foundation (ToolContext) | 0.1 | **Done** |
| 1 | Wire Existing Backends | 1.1–1.4 | **Done** |
| 2 | Complete File Tools | 2.1 | **Done** |
| 3 | LSP Client | 3.1–3.5 | **Done** |
| 4 | Tree-sitter AST Engine | 4.1–4.2 | **Done** |
| 5 | Polish & v1.0 | 5.1–5.3 | **In progress** |

## What's Built

- 30 tools across 8 categories (file, workflow, symbol, AST, git, semantic, memory, config)
- LSP client with transport, lifecycle, document symbols, references, definition, rename
- Tree-sitter symbol extraction + docstrings for Rust, Python, TypeScript, Go, Java, Kotlin
- Embedding pipeline: chunker, SQLite index, remote + local embedders
- Git integration: blame, log, diff via git2
- Persistent memory store with markdown-based topics
- Progressive disclosure output (exploring/focused modes via OutputGuard)
- MCP server over stdio (rmcp)
- 232 tests (227 passing, 5 ignored)

## What's Next

- Additional tree-sitter grammars
- Additional LSP server configurations
- sqlite-vec integration for vector similarity (currently pure-Rust cosine)
- Companion Claude Code plugin: `code-explorer-routing` (live at [mareurs/claude-plugins](https://github.com/mareurs/claude-plugins))

## Future Improvements

### Library Search

Search and navigate third-party library/dependency source code. Read-only access via LSP-inferred discovery, symbol navigation, and semantic search. See [`plans/2026-02-26-library-search-design.md`](plans/2026-02-26-library-search-design.md) for the full design.

**Progressive rollout:**

| Level | Name | What it enables |
|-------|------|-----------------|
| A | Follow-through reads | Read files LSP points to outside project root |
| B | Symbol navigation | `find_symbol` / `get_symbols_overview` on library code with `scope` parameter |
| C | Semantic search | Explicit `index_library` + scoped `semantic_search` on dependency source |
| D | LSP-inferred discovery | Auto-register libraries from `goto_definition` responses |

**Key concepts:** `LibraryRegistry` (`.code-explorer/libraries.json`) tracks known library paths. All library access is read-only. Results tagged `"source": "lib:<name>"` to distinguish from project code.

---

### Tool Usage Monitor / Statistics

Track tool call patterns to surface bugs, usage drift, and performance regressions over time.

**Motivation:** As the tool set grows and agents evolve, subtle behavioral shifts are hard to detect without data — e.g. semantic_search being called on every query instead of symbol tools, rising error rates on a specific tool, or LSP timeouts clustering around large files.

**What to capture per call:**
- Tool name, input shape (key names, not values), timestamp
- Outcome: success / error / overflow
- Latency (ms)
- Output mode (exploring vs focused), result count

**Storage:** Append-only SQLite table in `.code-explorer/usage.db` — same pattern as `embeddings.db`. Lightweight, local, no external dependencies.

**Surfacing:**
- `get_usage_stats` tool: per-tool call counts, error rates, p50/p99 latency, top error messages
- Time-bucketed view (last hour / day / week) to detect drift
- Overflow rate per tool (high overflow = agent is asking too broadly)

**Implementation sketch:**
- `UsageRecorder` wraps the dispatch loop in `server.rs` — transparent to individual tools
- Periodic rollup into summary rows to keep the table small
- Optional: emit structured logs for external aggregation (Prometheus, etc.)

---

### Multi-Agent Support (Generalize Beyond Claude Code)

Make code-explorer usable by any MCP-capable agent — Copilot, Cursor, Cline, custom agents — with routing knowledge included so agents know *when* to reach for each tool.

**Motivation:** The server already speaks MCP over stdio. The gap is that agents other than Claude Code lack the curated routing guidance (the `server_instructions.md` prompt) that tells Claude *how* to choose between `semantic_search`, `find_symbol`, `get_symbols_overview`, etc. Without this, agents default to over-using a single tool (usually semantic search).

**Work streams:**

1. **HTTP/SSE transport** (already planned) — lets non-CLI agents connect without spawning a subprocess.

2. **Agent-neutral routing prompt** — refactor `server_instructions.md` into a well-structured decision tree that any agent can consume as a system prompt or tool description prefix. Avoid Claude-specific framing.

3. **`code-explorer-routing` plugin / extension** — a thin adapter per agent platform:
   - Claude Code: existing plugin approach
   - VS Code Copilot: Language Model Tools API (`vscode.lm.registerTool`)
   - Cursor: `.cursorrules` + MCP config
   - Generic: OpenAPI spec + routing hints as tool descriptions

4. **Tool description quality** — every tool's `description()` should embed just enough routing guidance to work even without a system prompt (one-sentence "prefer this over X when Y" hint).

5. **Benchmark routing quality** — extend the live benchmark to test tool selection accuracy across agent backends, not just result quality.

---

### Incremental Index Rebuilding (Hash-Based Change Detection) — **Implemented**

Smart change detection for the embedding index — avoid re-hashing and re-embedding unchanged files. See [`plans/2026-02-26-incremental-index-design.md`](plans/2026-02-26-incremental-index-design.md) for the full design.

**Status:** Layers 0 (smart explicit) and staleness detection are implemented. Layer 1 (hook-driven) is ready to use via git hooks or Claude Code hooks. Layer 2 (filesystem watcher) is deferred.

**Layered trigger model:**

| Layer | Trigger | What it does |
|-------|---------|--------------|
| 0 — Smart explicit | `index_project` call | Git-diff + mtime hybrid detection, deleted-file cleanup |
| 1 — Hook-driven | Commit / pre-push hook | Same code path as Layer 0, triggered automatically |
| 2 — Watcher (deferred) | Filesystem events | Background `notify` crate watcher, debounced, opt-in |

**Key concepts:** `diff_and_reindex` is a single code path called by all triggers. Change detection fallback chain: git diff (cheapest) → mtime comparison → SHA-256 hash (always correct). `semantic_search` gains a staleness warning when the index is behind HEAD. Schema adds `mtime` column to `files` table and `last_indexed_commit` to `meta` table.

**Coordination:** Designed alongside Library Search — no schema conflicts. Library indexing can adopt the same pattern once `build_library_index` is implemented.

---

### Semantic Drift Detection — **Implemented**

Detect *how much* code changed in meaning, not just *that* it changed. SHA-256 is the gatekeeper (cheap, per-file); semantic comparison is the intelligent filter that tells you which differences actually matter. See [`plans/2026-02-26-semantic-drift-detection-design.md`](plans/2026-02-26-semantic-drift-detection-design.md) for the full design.

**Architecture:** Computed inside `build_index` Phase 3 — before `delete_file_chunks`, read old embeddings; after inserting new chunks, compare old vs new using a content-hash-first matching algorithm with greedy cosine fallback. Results persisted in a `drift_report` table (cleared each build).

**Use cases:**
- **Smart doc staleness filtering** — SHA-256 flags 20 files, drift scores show only 3 had meaningful changes
- **Drift-aware re-indexing feedback** — `IndexReport` gains drift summary so the agent knows *what kind* of changes happened
- **On-demand query** — `check_drift` tool reads `drift_report` with threshold/path filters

**Scoring:** Per-file `avg_drift` + `max_drift` (captures both broad-but-shallow and narrow-but-deep changes). Chunk matching: exact content match first (drift 0.0), then greedy cosine pairing on remainder, with unmatched chunks as added/removed (drift 1.0).

**Configuration:** Opt-in via `.code-explorer/project.toml` (defaults to `false` — reads old embeddings before deletion, adding memory/DB overhead):
```toml
[embeddings]
drift_detection_enabled = true
```

**Future improvement:** File-level semantic fingerprints (mean of chunk embeddings per file) for codebase evolution tracking across git tags/releases. Deferred until the transient comparison proves useful.

---

### Filesystem Watcher (Realtime Index Updates)

Background filesystem watcher for near-realtime index updates. **Depends on** Incremental Index Rebuilding (Layer 2 of that design).

**Motivation:** Layers 0+1 of incremental indexing cover commit-oriented workflows well, but some users want the index to stay current as they edit — especially in long coding sessions where commits are infrequent.

**Implementation sketch:**
- Use the `notify` crate (cross-platform: inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows)
- Spawn a background `tokio::spawn` task in the MCP server on startup
- Debounce events with a 2s window to batch rapid saves
- Filter events through `.gitignore` + `ignored_paths` config
- Call `diff_and_reindex` with a per-file candidate list (no git diff needed — watcher knows exactly which files changed)
- Opt-in via `project.toml`: `[index] watch = true`

**Platform considerations:**
- Linux: `inotify` has a per-user watch limit (`fs.inotify.max_user_watches`), may need guidance for large repos
- macOS: `FSEvents` is directory-level, efficient for large trees
- Windows: `ReadDirectoryChangesW` works but has buffer overflow edge cases on burst writes
- The `notify` crate abstracts all of this, but platform-specific tuning docs may be needed

---

### Glossary & Documentation Management (Hash-Based Change Tracking)

Maintain project glossaries and documentation that stay in sync with the codebase via content-hash change detection.

**Motivation:** LLM-generated documentation (onboarding summaries, architecture glossaries, API docs) goes stale the moment the underlying code changes. Manual upkeep is unsustainable. By tracking file content hashes, code-explorer can detect *which* documented files changed, compute targeted diffs, and trigger glossary/documentation updates — keeping project knowledge accurate without full re-indexing.

**Core mechanism:**

1. **Hash tracking** — Store a content hash (e.g. SHA-256) for every file that contributes to a glossary or documentation entry. Persist in `.code-explorer/doc-hashes.db` (SQLite, same pattern as `embeddings.db`).

2. **Change detection** — On a `check_docs` or `sync_docs` tool call (or automatically during `onboarding`), compare stored hashes against current file content. Files with mismatched hashes are flagged as stale.

3. **Targeted diff** — For each stale file, compute a diff (reusing `git/diff` infra or direct content comparison). Surface only the *meaningful* changes (skip whitespace-only, comment-only changes via configurable filters).

4. **Update trigger** — Present the diffs to the LLM with the current glossary entry, prompting a targeted update rather than a full rewrite. Alternatively, for structured glossaries, apply rule-based updates (renamed symbol → rename in glossary).

**Glossary features:**
- **Term extraction** — Build a glossary from codebase symbols, domain concepts, and abbreviations (combining AST/LSP data with semantic search)
- **Cross-reference** — Link glossary terms to source locations (file:line), kept accurate via hash tracking
- **Scope** — Per-project glossary in `.code-explorer/glossary.md` or structured `.code-explorer/glossary.json`

**Documentation management features:**
- **Doc registration** — `register_doc(path, sources: [file globs])` links a documentation file to the source files it describes
- **Staleness report** — `check_docs()` tool returns which docs are stale, what changed, and suggested update scope
- **Auto-update** — `sync_docs(path)` re-generates or patches a specific doc using the diffs as context

**Storage schema (doc-hashes.db):**
```sql
CREATE TABLE doc_sources (
    doc_path    TEXT NOT NULL,     -- the documentation/glossary file
    source_path TEXT NOT NULL,     -- a source file it depends on
    hash        TEXT NOT NULL,     -- SHA-256 of source content at last sync
    synced_at   TEXT NOT NULL,     -- ISO 8601 timestamp
    PRIMARY KEY (doc_path, source_path)
);
```

**Implementation sketch:**
- New `src/tools/docs.rs` module with `register_doc`, `check_docs`, `sync_docs`, `build_glossary` tools
- `src/docs/` module for hash computation, staleness detection, diff generation
- Integration with existing memory store — glossary terms can cross-reference memory topics
- Progressive disclosure: `check_docs` in exploring mode shows only stale counts; focused mode shows full diffs

**Example workflow:**
1. Onboarding creates `glossary.md` with key terms and `architecture.md` summary
2. `register_doc("glossary.md", sources: ["src/**/*.rs"])` tracks all Rust source hashes
3. Developer adds a new tool module — hash changes detected on next `check_docs()`
4. LLM receives: "3 files changed since last sync" + targeted diffs → updates glossary with new tool's terms

---

## Contributor Skills

Three Claude Code skills living in `.claude/skills/` within this repo. Contributors who open code-explorer in Claude Code get them automatically — no build step required. See [`plans/2026-02-26-contributor-skills-design.md`](plans/2026-02-26-contributor-skills-design.md) for the full design.

| Skill | Purpose | Status |
|---|---|---|
| `project-management` | Navigate sprint status, roadmap, open PRs and issues | Planned |
| `debugging` | Systematic debugging workflow for the Rust codebase | Planned |
| `log-stat-analyzer` | Analyze `usage.db` for call pattern drift and latency regressions | Blocked on Tool Usage Monitor |

### `project-management`

Surface current sprint status from the roadmap, map recent commits to sprint items, and guide contributors through opening correctly-structured PRs. Uses `git_log`, `git_diff`, and the GitHub MCP tools alongside `docs/ROADMAP.md` and `docs/plans/`.

### `debugging`

Systematic workflow from symptom to fix to verification — covering build failures, test failures, LSP timeouts, tree-sitter parse errors, and embedding pipeline issues. Guides contributors through hypothesis formation (`semantic_search`, `find_symbol`), targeted investigation (`git_blame`, `search_for_pattern`), and the `cargo build` / `cargo test` / `cargo clippy` verification loop.

### `log-stat-analyzer`

Structured workflow for interpreting Tool Usage Monitor data: per-tool call counts, error rates, p50/p99 latency, overflow rates, and time-bucketed drift detection. Produces actionable summaries (e.g. "semantic_search error rate up 3× in last 24h"). **Blocked** until the Tool Usage Monitor (`get_usage_stats` tool) is implemented.
