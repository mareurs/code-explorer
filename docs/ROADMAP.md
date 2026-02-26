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

- HTTP/SSE transport (in addition to stdio)
- Additional tree-sitter grammars
- Additional LSP server configurations
- sqlite-vec integration for vector similarity (currently pure-Rust cosine)
- Companion Claude Code plugin: `code-explorer-routing`

## Future Improvements

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
