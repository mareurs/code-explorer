# Documentation Structure Design — Semantic Search Coverage & Cross-References

**Date:** 2026-03-01  
**Status:** Approved  
**Scope:** `docs/manual/src/` (mdbook manual), `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`  
**Excludes:** `CLAUDE.md`

## Problem

Semantic search is a core differentiator of code-explorer but its documentation is
fragmented and structurally orphaned:

1. `Semantic Search Guide` in `SUMMARY.md` is a top-level item with no parent — it
   sits between Configuration and Language Support with no conceptual framing.
2. No concept page explains *what* embeddings are, how chunking works, or what
   similarity scores mean — every other major feature (Progressive Disclosure,
   Output Buffers, Routing Plugin, Worktrees) has one.
3. `embedding-backends.md` covers backend mechanics but gives no model selection
   guidance (which model to pick within a backend, dimensions, speed/quality tradeoffs).
4. Several missing cross-links: `tools/semantic-search.md` doesn't link to the guide,
   `tools/overview.md` doesn't link to the guide, `architecture.md` doesn't link to
   the concept page.
5. `first-project.md` references `nomic-embed-text` — the actual code default is
   `ollama:mxbai-embed-large` (locked by tests in `src/config/project.rs`).
6. `docs/ARCHITECTURE.md` and `docs/ROADMAP.md` have no pointer to the manual.

## Approach

**Approach B — Add concept page + structural fix** (chosen over minimal link-only
fix and full restructure with split embedding docs).

## Design

### 1. New concept page: `concepts/semantic-search.md`

Content sections:

- **What it is** — vector similarity search over code chunks; complements symbol tools
- **How it works** — three-step pipeline:
  - Chunking: language-aware text splitting with overlap (tracks 1-indexed line numbers)
  - Embedding: chunks → vectors via configured backend
  - Search: cosine similarity in SQLite; currently pure-Rust, sqlite-vec ANN planned
- **Similarity scores** — cosine 0–1; >0.8 usually directly relevant, <0.5 often
  tangential; code scores run lower than prose so calibrate expectations
- **When to use semantic vs symbol tools** — decision table:
  - Know the name → `find_symbol` / `list_symbols`
  - Know the concept → `semantic_search`
  - Know the file → `list_symbols(path)`
  - Know a pattern → `search_pattern`
- **Index lifecycle** — build once with `index_project`, incremental updates via
  git diff → mtime → SHA-256 fallback chain; drift detection via `index_status`
- **See also** — links to Setup Guide, Embedding Backends, Semantic Search tools

### 2. SUMMARY.md restructure

Before:
```
- [Project Configuration](configuration/project-toml.md)
  - [Embedding Backends](configuration/embedding-backends.md)

- [Semantic Search Guide](semantic-search-guide.md)    ← orphan

- [Language Support](language-support.md)
```

After:
```
- [Progressive Disclosure](concepts/progressive-disclosure.md)
  - ... existing children unchanged ...

- [Semantic Search](concepts/semantic-search.md)        ← NEW concept page
  - [Setup Guide](semantic-search-guide.md)             ← moved here

- [Project Configuration](configuration/project-toml.md)
  - [Embedding Backends](configuration/embedding-backends.md)

- [Language Support](language-support.md)
```

Embedding Backends stays under Configuration (config-focused). Both the concept page
and guide link to it rather than duplicating it.

### 3. Model guidance in `embedding-backends.md`

Add a "Recommended Models" section after "Backend Comparison", before the per-backend
deep dives. Table:

| Model string | Backend | Dims | Speed | Code quality | Notes |
|---|---|---|---|---|---|
| `ollama:mxbai-embed-large` | Ollama | 1024 | Medium | Good | **Default. Best starting point.** |
| `ollama:nomic-embed-text` | Ollama | 768 | Fast | Good | Lighter, slightly lower recall |
| `ollama:all-minilm` | Ollama | 384 | Very fast | Fair | Large repos where speed matters |
| `openai:text-embedding-3-small` | OpenAI | 1536 | Fast (network) | Excellent | Best quality per token cost |
| `openai:text-embedding-3-large` | OpenAI | 3072 | Fast (network) | Best | Overkill for most codebases |
| `local:BGESmallENV15Q` | fastembed | 384 | Medium (CPU) | Good | Air-gapped, no daemon |

Followed by 2–3 sentences: stick with the default unless you have a specific reason;
switching models requires a full reindex (see existing "Rebuilding After a Model Change").

### 4. Cross-link fixes

| File | Change |
|---|---|
| `tools/semantic-search.md` | Add "See also" at top → concept page + setup guide |
| `tools/overview.md` | "requires an embedding index" → link to setup guide |
| `getting-started/first-project.md` | Fix `nomic-embed-text` → `mxbai-embed-large` (two places) |
| `architecture.md` (manual) | Add concept page to Further Reading |
| `concepts/semantic-search.md` (new) | Links out to: guide, embedding-backends, tools/semantic-search |
| `semantic-search-guide.md` | Add link to concept page at top ("For how it works, see…") |

### 5. Developer-facing docs

**`docs/ARCHITECTURE.md`** — add a short note near the top:  
*"This document covers contributor-level internals. For the user-facing manual see `docs/manual/`."*

**`docs/ROADMAP.md`** — add inline links to the concept page and `embedding-backends.md`
where semantic search features are mentioned in "What's Built" and "What's Next".

## Files Changed

| File | Type |
|---|---|
| `docs/manual/src/concepts/semantic-search.md` | Create |
| `docs/manual/src/SUMMARY.md` | Edit |
| `docs/manual/src/configuration/embedding-backends.md` | Edit (add model table) |
| `docs/manual/src/tools/semantic-search.md` | Edit (add see-also) |
| `docs/manual/src/tools/overview.md` | Edit (add link) |
| `docs/manual/src/getting-started/first-project.md` | Edit (fix model name) |
| `docs/manual/src/architecture.md` | Edit (add link) |
| `docs/manual/src/semantic-search-guide.md` | Edit (add link to concept page) |
| `docs/ARCHITECTURE.md` | Edit (add manual pointer) |
| `docs/ROADMAP.md` | Edit (add inline links) |
