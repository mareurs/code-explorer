# Semantic Search Guide

Semantic search lets you find code by describing what it does rather than
knowing what it is called. This page walks you through the full setup from
choosing a backend to writing effective queries. For a reference of the
individual tools, see [Semantic Search Tools](tools/semantic-search.md).

## Choosing an Embedding Backend

code-explorer supports four embedding backends. The model string prefix in
`project.toml` selects which one is used:

| Prefix | Example | When to use |
|--------|---------|-------------|
| `ollama:` | `ollama:mxbai-embed-large` | Local development — free, private, no API key |
| `openai:` | `openai:text-embedding-3-small` | Best retrieval quality, cloud cost |
| `custom:` | `custom:my-model@http://host:8080` | Any OpenAI-compatible endpoint |
| `local:` | `local:BGESmallENV15Q` | Offline / air-gapped, no daemon required |

**Recommended starting point:** Ollama with `mxbai-embed-large`. It runs
entirely on your machine, requires no API key, and produces good results on
code. For more detail on all backends including OpenAI and local models, see
[Embedding Backends](configuration/embedding-backends.md).

## Setting Up Ollama

Install Ollama, pull the default model, and verify it responds correctly before
touching `project.toml`.

```bash
# Install Ollama (Linux/macOS)
curl -fsSL https://ollama.com/install.sh | sh

# Pull the default model
ollama pull mxbai-embed-large

# Verify the embedding endpoint is responding
curl http://localhost:11434/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"model": "mxbai-embed-large", "input": "test"}'
```

A successful response looks like:

```json
{
  "object": "list",
  "data": [{ "object": "embedding", "index": 0, "embedding": [0.012, -0.034, ...] }],
  "model": "mxbai-embed-large"
}
```

If `curl` returns a connection error, Ollama is not running. Start it with
`ollama serve` in a separate terminal and retry.

If you run Ollama on a non-default host or port, set the `OLLAMA_HOST`
environment variable before starting Claude Code:

```bash
export OLLAMA_HOST=http://192.168.1.10:11434
```

## Configuring code-explorer

The `[embeddings]` section of `.code-explorer/project.toml` controls which
model is used and how files are chunked. The defaults work well for most
projects:

```toml
[embeddings]
model = "ollama:mxbai-embed-large"
chunk_size = 1200
chunk_overlap = 200
```

`model` is the only required change if you want a different backend. The
`chunk_size` and `chunk_overlap` values are in characters (not tokens). Their
effects are described in the [Tuning](#tuning) section below.

To use OpenAI instead, set the model and export your API key:

```toml
[embeddings]
model = "openai:text-embedding-3-small"
```

```bash
export OPENAI_API_KEY=sk-...
```

## Building the Index

Once Ollama is running and `project.toml` is configured, build the index:

```json
{ "name": "index_project", "arguments": {} }
```

What happens internally:

1. code-explorer walks the project tree, skipping directories listed in
   `ignored_paths` (by default: `.git`, `node_modules`, `target`,
   `__pycache__`, `.venv`, `dist`, `build`, `.code-explorer`).
2. Each source file is split into overlapping chunks using an AST-aware
   chunker. Function and class boundaries are used as natural split points
   before falling back to character-count limits.
3. Each chunk is sent to the configured embedding backend, which returns a
   dense vector.
4. The vectors and chunk metadata are stored in
   `.code-explorer/embeddings.db` (SQLite).

**How long it takes:** With Ollama on a modern laptop, expect roughly 80–120
files per minute. OpenAI's API is faster in wall-clock time because requests
are batched and network latency is low — typically 3–5x faster for large
projects. A 10,000-line project usually indexes in under two minutes with
either backend.

**Incremental updates:** Running `index_project` again after editing a few
files is cheap. code-explorer hashes each file's content and only re-embeds
files whose hash has changed since the last run. Unchanged files are skipped
at negligible cost.

**Force reindex:** Use `force: true` when you change the `model` in
`project.toml`. Vectors from different models are not comparable, so the entire
index must be rebuilt:

```json
{ "name": "index_project", "arguments": { "force": true } }
```

You can check index health at any time:

```json
{ "name": "index_status", "arguments": {} }
```

The output shows `configured_model` (from `project.toml`) and
`indexed_with_model` (what was used to build the current index). If they
differ, a force reindex is needed.

## Searching Effectively

### Natural Language Queries

Describe what the code does in plain language. You do not need to know the
function name or file location:

```json
{ "name": "semantic_search", "arguments": { "query": "how errors are handled" } }
```

```json
{ "name": "semantic_search", "arguments": { "query": "database connection setup" } }
```

```json
{ "name": "semantic_search", "arguments": { "query": "authentication token validation" } }
```

Concrete, specific queries outperform vague ones. Prefer "retry logic with
exponential backoff" over "error handling". Prefer "connection pool
initialization" over "database".

### Code Snippet Queries

Paste a function signature or a short snippet as the query to find similar
code elsewhere in the project. This is useful for spotting duplication or
locating the canonical version of a pattern:

```json
{
  "name": "semantic_search",
  "arguments": {
    "query": "fn connect(host: &str, port: u16) -> Result<Connection>"
  }
}
```

### Interpreting Scores

Each result includes a `score` between 0 and 1 (cosine similarity):

| Score range | Interpretation |
|-------------|----------------|
| > 0.85 | Strong match — the chunk directly addresses your query |
| 0.6 – 0.85 | Related — the concept is present but may not be the primary focus |
| < 0.6 | Tangential — treat as background context at best |

The top result is not always the most useful one. Scan the top five results
before drilling into any single chunk.

### Recommended Workflow

Semantic search is the entry point for concept-first exploration. After finding
relevant chunks, use the symbol tools to navigate the surrounding code:

1. `semantic_search` — find the files and line ranges where a concept lives.
2. `list_symbols` on those files — see the surrounding structure.
3. `find_symbol` with `include_body: true` — read the exact implementation.
4. `find_references` — trace callers if needed.

## Tuning

### `chunk_size`

Controls the maximum character length of each embedded chunk (default: 1200).

- Larger chunks (1500–2000): more surrounding context per result, better for
  finding architectural patterns, but each result covers more code and
  similarity is averaged over a larger region.
- Smaller chunks (600–900): more precise location of the matching concept,
  useful for large files where a single function is what you want.

### `chunk_overlap`

Controls how many characters of overlap exist between consecutive chunks
(default: 200). Overlap prevents a concept from being silently split across
a chunk boundary. Reducing this below 100 risks missing matches that span
the end of one chunk and the start of the next.

### Model Choice

The embedding model has the largest effect on search quality. General-purpose
text models (`nomic-embed-text`, `text-embedding-3-small`) work well for
documentation and comments. Code-specific models
(`mxbai-embed-large`, `local:JinaEmbeddingsV2BaseCode`) tend to perform
better on function signatures and code identifiers.

After changing the model, always run `index_project` with `force: true`.

## Troubleshooting

**"No results" or empty results list**

The index may not be built yet. Run `index_status` to check. If
`indexed: false`, run `index_project`. If the index exists but results are
empty, the query may be too generic — try a more specific description.

**"Connection refused" when indexing**

Ollama is not running. Start it with `ollama serve`. If you are using a
non-default host, ensure `OLLAMA_HOST` is set correctly and matches what
Ollama is actually listening on.

**"Model not found" error**

The model has not been pulled. Run `ollama pull mxbai-embed-large` (or
whatever model is configured) and retry.

**Stale results after editing many files**

Run `index_project` without arguments. The incremental update will re-embed
only the files that changed.

**Results seem wrong after changing the model**

The index was built with a different model and the vectors are no longer
compatible. Run `index_project` with `force: true`. You can confirm the
mismatch by checking `index_status`: if `configured_model` and
`indexed_with_model` differ, a force reindex is required.

**Indexing is very slow**

Check that Ollama is running locally and not routing over a slow network
connection. If you need faster indexing and have an OpenAI account, switching
to `openai:text-embedding-3-small` typically reduces indexing time
significantly for large projects.
