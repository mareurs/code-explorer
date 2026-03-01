# Semantic Search Tools

Semantic search lets you find code by meaning rather than by exact name or
keyword. Instead of knowing what a function is called, you describe what it
does — "retry with exponential backoff", "authentication middleware", "how
errors are serialized to JSON" — and the tool finds the most relevant code
chunks in the project.

The backend stores vector embeddings of your source code in a SQLite database
at `.code-explorer/embeddings.db`. The embedding model is configurable (see
[Project Configuration](../configuration/project-toml.md)); the default works
with any OpenAI-compatible endpoint or a local Ollama server.

You must build the index before searching. Use `index_project` once, then
`semantic_search` as many times as you like. Incremental re-indexing is cheap:
only files that changed since the last run are re-embedded.

> **See also:** [Semantic Search Concepts](../concepts/semantic-search.md) — how
> chunking, embedding, and scoring work; when to use semantic search vs symbol
> tools. [Setup Guide](../semantic-search-guide.md) — step-by-step configuration
> and indexing walkthrough.

---

## `semantic_search`

**Purpose:** Find code by natural language description or code snippet. Returns
ranked chunks with file path, line range, and similarity score.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `query` | string | yes | — | Natural language description or code snippet to search for |
| `limit` | integer | no | `10` | Maximum number of results to return |
| `detail_level` | string | no | compact | `"full"` returns the complete chunk content instead of a 150-character preview |
| `offset` | integer | no | `0` | Skip this many results (for pagination) |
| `scope` | string | no | `"project"` | Search scope: `"project"` (default), `"lib:<name>"` for a specific library, `"libraries"` for all libraries, `"all"` for everything |

**Example:**

```json
{
  "query": "retry with exponential backoff",
  "limit": 5
}
```

**Output (compact, default):**

```json
{
  "results": [
    {
      "file_path": "src/embed/remote.rs",
      "language": "rust",
      "content": "async fn with_retry<F, Fut, T>(mut f: F, max_attempts: u8) -> anyhow::Result<T>\nwhere\n    F: FnMut() -> Fut,...",
      "start_line": 42,
      "end_line": 68,
      "score": 0.91,
      "source": "project"
    },
    {
      "file_path": "src/util/http.rs",
      "language": "rust",
      "content": "/// Exponential back-off starting at 200ms, doubling each attempt up to...",
      "start_line": 12,
      "end_line": 30,
      "score": 0.84,
      "source": "project"
    }
  ],
  "total": 2
}
```

In compact mode, `content` is truncated to 150 characters followed by `"..."`.
Use `detail_level: "full"` to get complete chunk bodies.

**Output (full detail):**

```json
{
  "query": "retry with exponential backoff",
  "limit": 5,
  "detail_level": "full"
}
```

The `content` field contains the full source text of each chunk. Combine with
`offset` to page through results:

```json
{
  "query": "retry with exponential backoff",
  "limit": 5,
  "detail_level": "full",
  "offset": 5
}
```

**Tips:**

- Use `semantic_search` when you know the concept but not the exact function
  name. For example: "where is the JWT decoded", "rate limiting logic",
  "database connection pool initialization".
- Paste a code snippet as the `query` to find similar code elsewhere in the
  project. This is useful for spotting duplication or finding the canonical
  version of a pattern.
- Scores above 0.85 are typically a strong match. Scores below 0.6 usually
  indicate the concept is not well represented in the index.
- If results are poor, check `index_status` to confirm the index is up to date,
  and `index_project` to rebuild if files have changed.
- For finding a symbol by name, `find_symbol` is faster and more precise.
  Semantic search is for concepts, not identifiers.

---

## `index_project`

**Purpose:** Build or incrementally update the semantic search index for the
active project. Only re-embeds files whose content has changed since the last
run unless `force` is set.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `force` | boolean | no | `false` | Force full reindex, ignoring cached file hashes |

**Example (incremental update):**

```json
{}
```

**Example (full reindex):**

```json
{
  "force": true
}
```

**Output:**

```json
{
  "status": "ok",
  "files_indexed": 3,
  "files_deleted": 0,
  "detail": "3 deleted",
  "total_files": 47,
  "total_chunks": 312
}
```

When drift detection is enabled (on by default) and files had
meaningful semantic changes, a `drift_summary` field is included with the
top-5 most-drifted files:

```json
{
  "status": "ok",
  "files_indexed": 3,
  "total_files": 47,
  "total_chunks": 312,
  "drift_summary": [
    { "file": "src/auth/service.rs", "avg_drift": "0.72", "max_drift": "0.91", "added": 2, "removed": 1 }
  ]
}
```

**Staleness warning** — if `semantic_search` is called when the index is behind
the current HEAD commit, results include:

```json
{ "stale": true, "behind_commits": 3, "hint": "Index is behind HEAD. Run index_project to update." }
```

**Tips:**

- Run `index_project` once when you first activate a project, then again after
  large refactors or when many files have changed.
- The incremental mode (default) uses a git diff → mtime → SHA-256 fallback
  chain. It is safe to run frequently — unchanged files are skipped at
  negligible cost.
- Use `force: true` if you have changed the embedding model in
  `project.toml`. Changing the model produces incompatible vectors, so a full
  reindex is required.
- Indexing runs synchronously. For large projects (thousands of files), this
  may take a few minutes the first time.

---

## `index_status`

**Purpose:** Show the current state of the embedding index: how many files and
chunks it covers, which model was used to build it, and where the database
lives.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `threshold` | number | no | — | When provided, includes drift data: minimum `avg_drift` to include (0.0–1.0). Omit to skip drift query. |
| `path` | string | no | — | SQL LIKE pattern to filter drift results by file path (e.g. `"src/tools/%"`) |

**Example (basic stats only):**

```json
{}
```

**Output (index exists):**

```json
{
  "indexed": true,
  "configured_model": "text-embedding-3-small",
  "indexed_with_model": "text-embedding-3-small",
  "file_count": 47,
  "chunk_count": 312,
  "embedding_count": 312,
  "db_path": "/home/user/myproject/.code-explorer/embeddings.db"
}
```

**Output (no index):**

```json
{
  "indexed": false,
  "message": "No index found. Run index_project first."
}
```

**Example (with drift query):**

```json
{ "threshold": 0.2 }
```

**Output (with drift data):**

```json
{
  "indexed": true,
  "file_count": 47,
  "chunk_count": 312,
  "drift": {
    "results": [
      {
        "file_path": "src/auth/service.rs",
        "avg_drift": 0.72,
        "max_drift": 0.91,
        "chunks_added": 2,
        "chunks_removed": 1
      }
    ],
    "total": 1
  }
}
```

Drift results are sorted by `max_drift` descending. `avg_drift` is the mean across all chunks; `max_drift` is the worst single chunk. Use `threshold: 0.1` to surface all meaningfully changed files; use `threshold: 0.4` to focus on major rewrites only.

Opt out of drift detection entirely with `drift_detection_enabled = false` in `.code-explorer/project.toml`:

```toml
[embeddings]
drift_detection_enabled = false
```

**Tips:**

- Call `index_status` before `semantic_search` to confirm the index exists and
  is built with the same model currently configured. If `configured_model`
  differs from `indexed_with_model`, search results will be unreliable — run
  `index_project` with `force: true` to rebuild.
- `chunk_count` and `embedding_count` should be equal under normal conditions.
  A mismatch can indicate an interrupted indexing run; fix it with
  `index_project`.
- Use `index_status` with `threshold: 0.1` after re-indexing to surface which files changed semantically, not just syntactically. A whitespace-only reformat will have `avg_drift ≈ 0.0`; a full function rewrite will approach `1.0`.

> **See also:** [Dashboard](../concepts/dashboard.md) — the Overview page
> surfaces index staleness and per-file drift scores visually, without a tool
> call.

