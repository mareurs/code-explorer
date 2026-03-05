# Documentation Structure — Semantic Search Coverage & Cross-References

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a semantic search concept page, fix the SUMMARY.md orphan, add model selection guidance, and wire up all missing cross-references across the manual and developer docs.

**Architecture:** Pure documentation changes — 10 markdown files touched, 1 created. No Rust code changes. No tests (markdown). Each task is a self-contained edit + commit.

**Design doc:** `docs/plans/2026-03-01-docs-structure-design.md`

---

### Task 1: Create the semantic search concept page

**Files:**
- Create: `docs/manual/src/concepts/semantic-search.md`

**Step 1: Create the file with this exact content**

```markdown
# Semantic Search

Semantic search finds code by meaning rather than by name or text pattern. It
answers queries like "authentication middleware", "retry with exponential backoff",
or "parse JSON from HTTP response" — without knowing what the relevant functions
are called.

It complements symbol tools: use symbol tools when you know the name, semantic
search when you know the concept.

## How It Works

Three steps happen when you call `semantic_search`:

**1. Chunking** — The first time `index_project` runs, every source file is split
into overlapping chunks of roughly 1500 characters. Splits follow language
structure: function and class boundaries are preferred over arbitrary line cuts.
Each chunk records its 1-indexed start and end line so results link back to
exact source locations.

**2. Embedding** — Each chunk is converted to a vector (a list of floating-point
numbers) by the configured embedding model. Semantically similar text produces
vectors that point in similar directions in high-dimensional space. The vectors
are stored in `.codescout/embeddings.db`.

**3. Search** — Your query is embedded with the same model and compared to every
stored chunk using cosine similarity. The closest chunks are returned, ranked by
score.

The index is incremental. On subsequent `index_project` calls, only files that
changed since the last run are re-embedded — detected via git diff, then file
mtime, then SHA-256 as a fallback chain.

## Similarity Scores

Results include a score between 0 and 1:

| Score | Meaning |
|---|---|
| > 0.85 | Almost certainly what you're looking for |
| 0.70 – 0.85 | Likely relevant — worth inspecting |
| 0.50 – 0.70 | Tangentially related |
| < 0.50 | Probably noise |

Code embeddings score lower than prose embeddings for the same conceptual
similarity — a score of 0.75 in a code search is strong. Do not compare
scores across different embedding models; they are not on the same scale.

## When to Use Semantic Search

| You know... | Use |
|---|---|
| The exact name | `find_symbol(pattern)` |
| The file it's in | `list_symbols(path)` |
| A text fragment | `search_pattern(regex)` |
| The concept, not the name | `semantic_search(query)` |
| The concept, inside a library | `semantic_search(query, scope: "lib:<name>")` |

Semantic search is slowest of these options (it embeds your query at call time
and scans all stored vectors). Prefer symbol tools when you know the name.

## Index Lifecycle

Build the index once before first use:

```json
{ "name": "index_project", "arguments": {} }
```

Check its health:

```json
{ "name": "index_status", "arguments": {} }
```

The index is stored in `.codescout/embeddings.db` and excluded from version
control by default. Each team member builds their own local copy.

**Drift detection:** `index_status` can report per-file drift scores — a measure
of how much file content has changed since it was last indexed. Pass `threshold`
to surface files with high drift:

```json
{ "name": "index_status", "arguments": { "threshold": 0.3 } }
```

Switching embedding models invalidates the entire index — all chunks must be
re-embedded. See [Embedding Backends](../configuration/embedding-backends.md)
for model selection guidance.

## Further Reading

- [Semantic Search Setup Guide](../semantic-search-guide.md) — step-by-step:
  choose a backend, configure, build the index, write effective queries
- [Embedding Backends](../configuration/embedding-backends.md) — all supported
  backends and model selection guidance
- [Semantic Search Tools](../tools/semantic-search.md) — full reference for
  `semantic_search`, `index_project`, and `index_status`
```

**Step 2: Verify the file was created correctly**

Check it exists and has the expected sections:
```bash
grep "^## " docs/manual/src/concepts/semantic-search.md
```
Expected output:
```
## How It Works
## Similarity Scores
## When to Use Semantic Search
## Index Lifecycle
## Further Reading
```

**Step 3: Commit**

```bash
git add docs/manual/src/concepts/semantic-search.md
git commit -m "docs: add semantic search concept page"
```

---

### Task 2: Restructure SUMMARY.md

**Files:**
- Modify: `docs/manual/src/SUMMARY.md`

**Step 1: Read the current SUMMARY to find the exact text to replace**

Open `docs/manual/src/SUMMARY.md`. Find this block (around lines 19–27):

```markdown
- [Project Configuration](configuration/project-toml.md)
  - [Embedding Backends](configuration/embedding-backends.md)

- [Semantic Search Guide](semantic-search-guide.md)

- [Language Support](language-support.md)
```

**Step 2: Replace it with**

```markdown
- [Semantic Search](concepts/semantic-search.md)
  - [Setup Guide](semantic-search-guide.md)

- [Project Configuration](configuration/project-toml.md)
  - [Embedding Backends](configuration/embedding-backends.md)

- [Language Support](language-support.md)
```

The new concept page sits alongside the other top-level concepts (Progressive
Disclosure). The guide becomes its child. Embedding Backends stays under
Configuration — both new pages link to it.

**Step 3: Verify SUMMARY.md still has all entries**

```bash
grep -c "\- \[" docs/manual/src/SUMMARY.md
```
The count should be the same as before (one entry replaced, one added → net +1).

**Step 4: Commit**

```bash
git add docs/manual/src/SUMMARY.md
git commit -m "docs: move Semantic Search Guide under new concept page in SUMMARY"
```

---

### Task 3: Add model selection guidance to `embedding-backends.md`

**Files:**
- Modify: `docs/manual/src/configuration/embedding-backends.md`

**Step 1: Read the file to find the insertion point**

Open `docs/manual/src/configuration/embedding-backends.md`. Find the line:

```markdown
## Ollama (Default)
```

The new "Recommended Models" section goes between `## Backend Comparison` and
`## Ollama (Default)`.

**Step 2: Insert the following block between those two sections**

```markdown
## Recommended Models

Start with the default. Switch only when you have a specific reason.

| Model string | Backend | Dims | Speed | Code quality | Notes |
|---|---|---|---|---|---|
| `ollama:mxbai-embed-large` | Ollama | 1024 | Medium | Good | **Default. Best starting point.** |
| `ollama:nomic-embed-text` | Ollama | 768 | Fast | Good | Lighter; slightly lower recall |
| `ollama:all-minilm` | Ollama | 384 | Very fast | Fair | Large repos where indexing speed matters |
| `openai:text-embedding-3-small` | OpenAI | 1536 | Fast (network) | Excellent | Best quality/cost if cloud spend is acceptable |
| `openai:text-embedding-3-large` | OpenAI | 3072 | Fast (network) | Best | Overkill for most codebases |
| `local:BGESmallENV15Q` | fastembed | 384 | Medium (CPU) | Good | Air-gapped or no daemon; no GPU needed |

**Switching models requires a full reindex** — see
[Rebuilding After a Model Change](#rebuilding-after-a-model-change) below.
Scores are not comparable across models; a score of 0.75 means different things
with different models.

---
```

**Step 3: Verify the section was inserted**

```bash
grep -n "## Recommended Models\|## Backend Comparison\|## Ollama" docs/manual/src/configuration/embedding-backends.md | head -5
```
Expected: "Recommended Models" appears between "Backend Comparison" and "Ollama".

**Step 4: Commit**

```bash
git add docs/manual/src/configuration/embedding-backends.md
git commit -m "docs: add Recommended Models table to embedding-backends"
```

---

### Task 4: Fix cross-links in tool reference pages

**Files:**
- Modify: `docs/manual/src/tools/semantic-search.md`
- Modify: `docs/manual/src/tools/overview.md`

**Step 1: Add "See also" to `tools/semantic-search.md`**

Open the file. Find the opening paragraph (first few lines after `# Semantic Search Tools`). After that paragraph, insert:

```markdown
> **See also:** [Semantic Search Concepts](../concepts/semantic-search.md) — how
> chunking, embedding, and scoring work; when to use semantic search vs symbol
> tools. [Setup Guide](../semantic-search-guide.md) — step-by-step configuration
> and indexing walkthrough.
```

**Step 2: Fix the link in `tools/overview.md`**

Open `docs/manual/src/tools/overview.md`. Find the Semantic Search section (around the `## [Semantic Search](semantic-search.md)` heading). It currently says something like "requires an embedding index built with `index_project`". Add a link at the end of that sentence:

```markdown
index built with `index_project` — see the [Semantic Search Setup Guide](../semantic-search-guide.md).
```

**Step 3: Verify**

```bash
grep -n "Setup Guide\|semantic-search-guide\|semantic-search\.md" docs/manual/src/tools/semantic-search.md docs/manual/src/tools/overview.md
```
Both files should show a reference to the guide.

**Step 4: Commit**

```bash
git add docs/manual/src/tools/semantic-search.md docs/manual/src/tools/overview.md
git commit -m "docs: add cross-links to semantic search guide from tool reference pages"
```

---

### Task 5: Fix cross-links in `semantic-search-guide.md` and `first-project.md`

**Files:**
- Modify: `docs/manual/src/semantic-search-guide.md`
- Modify: `docs/manual/src/getting-started/first-project.md`

**Step 1: Add concept page link to `semantic-search-guide.md`**

Open the file. After the `# Semantic Search Guide` heading and before `## Choosing an Embedding Backend`, insert:

```markdown
> For an explanation of how semantic search works under the hood — chunking,
> scoring, and when to use it vs symbol tools — see
> [Semantic Search Concepts](concepts/semantic-search.md).
```

**Step 2: Fix the model name in `first-project.md`**

Open `docs/manual/src/getting-started/first-project.md`. Find the index_status
sample output block:

```
  Model         : nomic-embed-text
```

Change it to:

```
  Model         : mxbai-embed-large
```

The actual code default is `ollama:mxbai-embed-large` (confirmed in
`src/config/project.rs:185` and locked by two tests). The `nomic-embed-text`
string only appears as a test constant in `src/embed/remote.rs:135`.

**Step 3: Verify**

```bash
grep -n "nomic-embed-text" docs/manual/src/getting-started/first-project.md
```
Expected: no output (all instances fixed).

```bash
grep -n "mxbai" docs/manual/src/getting-started/first-project.md
```
Expected: the fixed sample output line.

**Step 4: Commit**

```bash
git add docs/manual/src/semantic-search-guide.md docs/manual/src/getting-started/first-project.md
git commit -m "docs: link guide to concept page; fix nomic-embed-text → mxbai-embed-large default"
```

---

### Task 6: Add concept page to manual `architecture.md` Further Reading

**Files:**
- Modify: `docs/manual/src/architecture.md`

**Step 1: Find the Further Reading section**

Open `docs/manual/src/architecture.md`. Find the `## Further Reading` section near the bottom. It currently links to Project Configuration and Embedding Backends.

**Step 2: Add the concept page link**

Add this line to the Further Reading list:

```markdown
- [Semantic Search Concepts](concepts/semantic-search.md) — how the embedding
  pipeline works, similarity scoring, and when to reach for semantic vs symbol search
```

**Step 3: Verify**

```bash
grep -n "Semantic Search\|Further Reading" docs/manual/src/architecture.md
```
Expected: the new link appears in the Further Reading section.

**Step 4: Commit**

```bash
git add docs/manual/src/architecture.md
git commit -m "docs: link semantic search concept page from architecture Further Reading"
```

---

### Task 7: Update developer-facing docs

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/ROADMAP.md`

**Step 1: Add manual pointer to `docs/ARCHITECTURE.md`**

Open `docs/ARCHITECTURE.md`. Find the `# Architecture` heading and the `## Overview` paragraph below it. Add this note directly after the `## Overview` heading:

```markdown
> **User documentation:** This file covers contributor-level internals. For the
> user-facing manual — installation, tool reference, semantic search guide — see
> [`docs/manual/src/`](manual/src/introduction.md).
```

**Step 2: Add links in `docs/ROADMAP.md`**

Open `docs/ROADMAP.md`. In the `## What's Built` bullet list, find:

```markdown
- **Semantic search** — embedding pipeline with sqlite-vec ANN-indexed KNN, incremental rebuilds, drift detection
```

Change it to:

```markdown
- **Semantic search** — embedding pipeline with sqlite-vec ANN-indexed KNN, incremental rebuilds, drift detection ([concept page](manual/src/concepts/semantic-search.md), [backends](manual/src/configuration/embedding-backends.md))
```

In `## What's Next`, find:

```markdown
- sqlite-vec integration for vector similarity (currently pure-Rust cosine)
```

Add a parenthetical:

```markdown
- sqlite-vec integration for vector similarity (currently pure-Rust cosine — see [Semantic Search](manual/src/concepts/semantic-search.md))
```

**Step 3: Verify**

```bash
grep -n "manual/src" docs/ARCHITECTURE.md docs/ROADMAP.md
```
Both files should show the new links.

**Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md docs/ROADMAP.md
git commit -m "docs: link manual from ARCHITECTURE.md and ROADMAP.md"
```

---

## Summary

10 files changed, 1 created, 7 commits total:

| Task | Commit message |
|---|---|
| 1 | `docs: add semantic search concept page` |
| 2 | `docs: move Semantic Search Guide under new concept page in SUMMARY` |
| 3 | `docs: add Recommended Models table to embedding-backends` |
| 4 | `docs: add cross-links to semantic search guide from tool reference pages` |
| 5 | `docs: link guide to concept page; fix nomic-embed-text → mxbai-embed-large default` |
| 6 | `docs: link semantic search concept page from architecture Further Reading` |
| 7 | `docs: link manual from ARCHITECTURE.md and ROADMAP.md` |
