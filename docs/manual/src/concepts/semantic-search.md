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
vectors that point in similar directions in high-dimensional space.
The default model is `ollama:mxbai-embed-large`; see
[Embedding Backends](../configuration/embedding-backends.md) to change it.
The vectors are stored in `.code-explorer/embeddings.db`.

**3. Search** — Your query is embedded with the same model and compared to every
stored chunk using cosine similarity. The closest chunks are returned, ranked by
score.

The index is incremental. On subsequent `index_project` calls, only files that
changed since the last run are re-embedded — detected via git diff, then file
mtime, then SHA-256 as a fallback chain.

## Similarity Scores

Results include a score between 0 and 1:

| Score     | Meaning                                |
|-----------|----------------------------------------|
| > 0.85    | Almost certainly what you're looking for |
| 0.70 – 0.85 | Likely relevant — worth inspecting   |
| 0.50 – 0.70 | Tangentially related                 |
| < 0.50    | Probably noise                         |

Code embeddings score lower than prose embeddings for the same conceptual
similarity — a score of 0.75 in a code search is strong. Do not compare
scores across different embedding models; they are not on the same scale.

## When to Use Semantic Search

| You know...                    | Use                                              |
|--------------------------------|--------------------------------------------------|
| The exact name                 | `find_symbol(pattern)`                           |
| The file it's in               | `list_symbols(path)`                             |
| A text fragment                | `search_pattern(regex)`                          |
| The concept, not the name      | `semantic_search(query)`                         |
| The concept, inside a library  | `semantic_search(query, scope: "lib:<name>")`    |

Semantic search is slowest of these options (it embeds your query at call time
and scans all stored vectors). Prefer symbol tools when you know the name.

## Index Lifecycle

Build the index once before first use:

```json
{ "tool": "index_project", "arguments": {} }
```

Check its health:

```json
{ "tool": "index_status", "arguments": {} }
```

The index is stored in `.code-explorer/embeddings.db` and excluded from version
control by default. Each team member builds their own local copy.

**Drift detection:** `index_status` can report per-file drift scores — a measure
of how much file content has changed since it was last indexed. Pass `threshold`
to surface files with high drift:

```json
{ "tool": "index_status", "arguments": { "threshold": 0.3 } }
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
