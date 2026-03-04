# Features

Implemented capabilities in codescout. For what's coming next, see [`ROADMAP.md`](ROADMAP.md).

---

## OutputBuffer (`@cmd_*` and `@file_*` refs)

Session-scoped LRU buffer (max 20 entries) that stores large command output and file content without flooding the context window. Agents query stored output using Unix tools.

**How it works:**
- `run_command` output > 50 lines → stored in buffer, returns smart summary + `@cmd_xxxx` handle; buffer-ref queries (`grep @ref`, `jq @tool_ref`) return up to 100 lines inline with `truncated`/`shown`/`total` metadata and a next-page hint when truncated
- `read_file` on large files → returns summary + `@file_xxxx` handle
- Buffer refs can be passed directly to follow-up `run_command` calls:
  ```
  run_command("grep FAILED @cmd_a1b2")
  run_command("sed -n '42,80p' @file_abc1")
  run_command("diff @cmd_a1b2 @file_abc1")
  ```
- `is_buffer_only` flag: commands that only operate on buffer refs skip all security checks
- Stderr captured automatically; no `2>&1` needed

---

## run_command Redesign

`run_command` gained significant new capabilities beyond basic shell execution.

**New parameters:**
- `cwd` — run from a subdirectory (relative to project root); never prefix commands with `cd /abs/path &&`
- `acknowledge_risk: true` — bypass the two-round-trip speed bump for dangerous commands
- `timeout_secs` — configurable execution timeout

**Dangerous command detection:**
- 11 built-in dangerous patterns (rm -rf, git reset --hard, DROP TABLE, etc.)
- First call returns `RecoverableError` describing the risk; second call with `acknowledge_risk: true` executes
- Config overrides in `project.toml [security]`:
  ```toml
  shell_allow_always = ["cargo clean"]
  shell_dangerous_patterns = ["my-custom-destructive-cmd"]
  ```

**Smart summaries:** Long output is summarized by command type — cargo test (pass/fail counts, failed test names), cargo build (error extraction), generic (head + tail with omission count).

**Source file blocking:** `run_command` cannot `cat`/`head`/`tail` source files (`.rs`, `.ts`, `.py`, etc.) — use codescout symbol tools instead. Pass `acknowledge_risk: true` only if truly necessary.

---

## read_file Smart Buffering

Large files are no longer dumped raw into context.

**Behaviour by file type:**
- **Source code files** (Rust, Python, TS, Go, etc.): blocked for direct reads — use `list_symbols` or `find_symbol(include_body=true)` instead. Targeted reads with `start_line` + `end_line` are allowed.
- **Non-source files** (Markdown, TOML, JSON, etc.): returned directly for short files; buffered with smart summary + `@file_*` ref for large files (> 200 lines).

**Smart summarizers** per file type: Markdown (headings outline), TOML/JSON (top-level key structure), source fallback (top-level symbol names via AST).

---

## Dual-Audience Output

8 read-heavy tools emit two representations in a single response: structured JSON for the agent and a human-readable preview rendered in the UI.

**Affected tools:** `read_file`, `list_dir`, `list_symbols`, `find_symbol`, `search_pattern`

**Why:** Agents need machine-parseable data; humans reviewing tool output in the UI need readable prose. The split is handled via `call_content()` — the agent sees JSON, the UI renders markdown.

---

## Progressive Discoverability

Overflow responses guide agents toward narrower follow-up queries rather than truncating silently.

**Key additions:**
- `find_symbol` exploring cap: 50 results (down from 200) — but overflow includes `by_file` breakdown
- `by_file` array `[{"file":"...","count":N}]` — agents zoom into the top file with `path=` parameter
- `by_file` capped at 15 entries; `by_file_overflow` count for omitted files
- `kind` filter on `find_symbol`: `"function"`, `"class"`, `"struct"`, `"interface"`, `"type"`, `"enum"`, `"module"`, `"constant"`
- `list_symbols` single-file cap: 100 top-level symbols

**Canonical guide:** [`PROGRESSIVE_DISCOVERABILITY.md`](PROGRESSIVE_DISCOVERABILITY.md)

---

## Symbol Signatures and Detail

LSP `DocumentSymbol.detail` is now captured and surfaced in symbol tool output.

**New fields:**
- `detail` — raw LSP detail string (e.g. return type, parameter list fragment)
- `signature` — synthesized display signature: `name: detail` for short details, `name(…): detail` for functions
- `show_file: bool` parameter on `find_symbol` / `list_symbols` — opt-in to include `file` field in each result (replaces the old `source` parameter)

---

## edit_file

Find-and-replace tool for any file. Locates `old_string` and replaces it with `new_string`.

**Parameters:**
- `path`, `old_string`, `new_string`
- `replace_all: bool` — replace every occurrence (default: first unique match only)

**Constraints:** `old_string` must match exactly (whitespace-sensitive). Fails if not found; fails if multiple matches unless `replace_all` is true. Setting `new_string` to `""` deletes the match.

Security-gated: blocked when file writes are disabled for the project.

---

## remove_symbol

Deletes a named symbol entirely — including its doc comments and attributes — using LSP for precise location.

**Parameters:** `name_path` (e.g. `"MyStruct/my_method"`), `path`

Removes the full declaration: doc comments (`///`, `/** */`), attributes (`#[derive(...)]`), and the symbol body. Returns `"ok"` on success.

Security-gated alongside other write tools.

---

## Worktree Write Guard

Write tools detect when the active project and the shell's working directory are in different git worktrees, and emit an advisory `worktree_hint` field rather than silently modifying the wrong tree.

**Affected tools:** `create_file`, `edit_file`, `replace_symbol`, `insert_code`, `remove_symbol`

**When triggered:** After `EnterWorktree`, if `activate_project` hasn't been called with the worktree path, write tools include:
```json
{ "worktree_hint": "activate_project(\"/path/to/worktree\") first" }
```

`list_git_worktrees` tool lists all active worktrees for the current repo.

---

## search_pattern context_lines

`search_pattern` now supports `context_lines: N` — lines of context shown before and after each match.

Adjacent matches that share a context window are merged into a single block, reducing noise. The `match_line` field on merged blocks points to the first match — scan `content` for further matches within the block.

---

## rename_symbol Text Sweep

After LSP rename, `rename_symbol` automatically sweeps all project files for remaining textual occurrences of the old name that LSP missed (comments, doc strings, string literals, macro arguments).

**Response fields:**
- `renamed`: number of LSP-renamed locations
- `textual_matches`: list of `{file, line, content}` for remaining occurrences not renamed by LSP

Agents should review `textual_matches` and decide whether manual edits are needed — string literals and macro args may require context-sensitive judgment.

---

## sqlite-vec KNN Search

Semantic search migrated from a pure-Rust cosine similarity loop to `sqlite-vec` (ANN-indexed KNN via `vec0` virtual table).

**Impact:** Sub-millisecond nearest-neighbour lookups even on large indexes. The `chunk_embeddings` table uses `vec0` for approximate nearest-neighbour search, falling back to exact cosine for small indexes.

---

## Project Customization via system-prompt.md

Projects can ship their own navigation guidance that gets injected into the MCP server instructions at startup.

**How to use:** Create `.code-explorer/system-prompt.md` in the project root. Its contents appear in the server instructions as "Custom Instructions" — project-specific hints for tool selection, entry points, search tips, etc.

`onboarding` generates a draft `system-prompt.md` as part of its output, pre-populated with detected languages, key files, and navigation strategy. The `system_prompt` field in `project.toml` can also point to a custom path.

---

## Onboarding Redesign

`onboarding` was redesigned to produce richer, more actionable output.

**What it now does:**
- Detects languages and reads key files (README, build configs, CLAUDE.md)
- Generates a system-prompt draft with language-specific navigation hints (e.g. Rust: start with `src/main.rs` → `list_symbols`; Python: entry points + module tree)
- Returns structured project context: language list, framework hints, important file paths
- `force: true` re-runs even if already onboarded

Language-specific hints cover: Rust, Python, TypeScript/JavaScript, Go, Java, Kotlin.

---

## RecoverableError — Non-Fatal Errors

Tool failures are now split into two classes with different MCP semantics.

| Class | When to use | MCP `isError` | Effect on sibling calls |
|-------|-------------|---------------|------------------------|
| `RecoverableError` | Expected, input-driven failures (path not found, unsupported file type, bad glob) | `false` | Siblings continue |
| `anyhow::bail!` | Genuine tool failure (LSP crash, security violation, programming error) | `true` | Claude Code aborts parallel calls |

`RecoverableError` responses include a `hint` field with a corrective suggestion. Agents see the problem and can retry or adjust without the entire parallel batch failing.

---

## Kotlin LSP (JetBrains Official)

Switched Kotlin language server from the community `fwcd/kotlin-language-server` to the official JetBrains `kotlin-lsp` (v261+, IntelliJ-powered).

**Impact:** Full IntelliJ analysis — completion, navigation, type inference, refactoring — instead of the limited community server. Requires Java 21+. Configurable init timeout via `project.toml`:
```toml
[lsp.kotlin]
init_timeout_secs = 120
```

---

---

## Library Search

Search and navigate third-party library/dependency source code. Read-only access via LSP-inferred discovery, symbol navigation, and semantic search.

**Design doc:** [`plans/2026-02-26-library-search-design.md`](plans/2026-02-26-library-search-design.md)

**Progressive rollout levels:**

| Level | Name | What it enables |
|-------|------|-----------------|
| A | Follow-through reads | Read files LSP points to outside project root |
| B | Symbol navigation | `find_symbol` / `list_symbols` on library code with `scope` parameter |
| C | Semantic search | `index_project` on library path + scoped `semantic_search` on dependency source |
| D | LSP-inferred discovery | Auto-register libraries from `goto_definition` responses |

**Key concepts:**
- `LibraryRegistry` (`.code-explorer/libraries.json`) tracks known library paths
- All library access is read-only
- Results tagged `"source": "lib:<name>"` to distinguish from project code
- `list_libraries` — show registered libraries and status
- `index_project` with library path — build embedding index for a specific library

---

## Project Dashboard

A unified view of project health, configuration, and activity — surfaced via the `dashboard` CLI subcommand as a web UI with tool stats charts.

**What it shows:**

| Section | Data |
|---------|------|
| **Project** | Name, root path, detected languages, active LSP servers |
| **Settings** | Embedding model, chunk size, ignored paths, enabled features |
| **Index** | File count, chunk count, last indexed commit/timestamp, staleness status |
| **Drift** | Drift report summary (files changed, avg/max drift score) — if enabled |
| **Tool calls** | Per-tool call counts, error rates, p50/p99 latency — from `usage.db` |
| **Errors** | Recent errors by tool (last N, with timestamps) |
| **LSP** | Active language servers, health status, pending requests |

**Usage:**
```bash
cargo run -- dashboard --project .
```

---

## Tool Usage Monitor / Statistics

Track tool call patterns to surface bugs, usage drift, and performance regressions over time.

**What is captured per call:**
- Tool name, input shape (key names, not values), timestamp
- Outcome: success / error / overflow
- Latency (ms)
- Output mode (exploring vs focused), result count

**Storage:** Append-only SQLite table in `.code-explorer/usage.db`.

**Surfacing via the dashboard** (`codescout dashboard`):
- Per-tool call counts, error rates, p50/p99 latency
- Time-bucketed view: last hour / day / week / 30 days
- Overflow rate per tool (high overflow = agent asking too broadly)

---

## Incremental Index Rebuilding (Hash-Based Change Detection)

Smart change detection for the embedding index — avoids re-hashing and re-embedding unchanged files.

**Design doc:** [`plans/2026-02-26-incremental-index-design.md`](plans/2026-02-26-incremental-index-design.md)

**Layered trigger model:**

| Layer | Trigger | Status |
|-------|---------|--------|
| 0 — Smart explicit | `index_project` call | **Implemented** |
| 1 — Hook-driven | Commit / pre-push hook | **Implemented** (same code path as Layer 0) |
| 2 — Watcher | Filesystem events | Deferred — see Roadmap |

**Change detection fallback chain:** git diff (cheapest) → mtime comparison → SHA-256 hash (always correct).

`semantic_search` emits a staleness warning when the index is behind HEAD.

---

## Semantic Drift Detection

Detect *how much* code changed in meaning, not just *that* it changed. SHA-256 is the gatekeeper (cheap, per-file); semantic comparison is the intelligent filter that tells you which differences actually matter.

**Design doc:** [`plans/2026-02-26-semantic-drift-detection-design.md`](plans/2026-02-26-semantic-drift-detection-design.md)

**Architecture:** Computed inside `build_index` Phase 3 — compares old vs new embeddings using content-hash-first matching with greedy cosine fallback. Results persisted in a `drift_report` table (cleared each build).

**Use cases:**
- **Smart doc staleness filtering** — SHA-256 flags 20 files, drift scores show only 3 had meaningful changes
- **Drift-aware re-indexing feedback** — `IndexReport` includes drift summary
- **On-demand query** — `project_status(threshold, path)` reads `drift_report` with threshold/path filters

**Scoring:** Per-file `avg_drift` + `max_drift`. Chunk matching: exact content match first (drift 0.0), then greedy cosine pairing, with unmatched chunks as added/removed (drift 1.0).

**Configuration** (opt-in, defaults to `false`):
```toml
[embeddings]
drift_detection_enabled = true
```

**Future improvement:** File-level semantic fingerprints (mean of chunk embeddings per file) for codebase evolution tracking across git tags/releases. Deferred until transient comparison proves useful.
