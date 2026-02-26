# Embedding Backends

Semantic search requires converting source code into vector embeddings. code-explorer supports
four backends, selected at runtime by the `model` field in `[embeddings]` inside
`.code-explorer/project.toml`. The prefix before the colon determines which backend is used.

```toml
[embeddings]
model = "ollama:mxbai-embed-large"   # default
```

---

## Backend Comparison

| Backend | Speed | Quality | Cost | Privacy | Setup |
|---|---|---|---|---|---|
| Ollama (default) | Medium | Good | Free | Local | Install Ollama + pull model |
| OpenAI | Fast (network) | Excellent | Pay-per-token | Data sent to OpenAI | Set `OPENAI_API_KEY` |
| Custom endpoint | Varies | Varies | Varies | Depends on host | Point at any compatible server |
| Local (fastembed) | Slow (CPU) | Good–Excellent | Free | Fully local | `--features local-embed` at build time |

---

## Ollama (Default)

Uses a locally running [Ollama](https://ollama.com/) daemon. No API key is required.

**Model string format:** `"ollama:<model-name>"`

**Default:** `"ollama:mxbai-embed-large"`

**Endpoint:** `$OLLAMA_HOST/v1/embeddings` (default: `http://localhost:11434/v1/embeddings`)

### Setup

1. Install Ollama from [ollama.com](https://ollama.com/).
2. Pull the embedding model:

   ```bash
   ollama pull mxbai-embed-large
   ```

3. Make sure the daemon is running:

   ```bash
   ollama serve
   ```

### Configuration

```toml
[embeddings]
model = "ollama:mxbai-embed-large"
```

To use a different Ollama host (e.g. a remote machine or a custom port), set the
`OLLAMA_HOST` environment variable before starting the MCP server:

```bash
export OLLAMA_HOST=http://192.168.1.50:11434
```

### Automatic CPU Fallback

When code-explorer is built with both `remote-embed` and `local-embed` features, it probes
Ollama before every indexing or search call. If the daemon is not reachable within 2 seconds,
it automatically falls back to `local:BGESmallENV15Q` and emits a warning:

```
Ollama not reachable at http://localhost:11434: …
Falling back to local:BGESmallENV15Q (CPU-friendly, ~20 MB).
Set embeddings.model in .code-explorer/project.toml to suppress this.
```

This means machines without Ollama installed — or without a GPU — still get working semantic
search out of the box, just with the smaller local model. To silence the warning and make the
fallback permanent, set the model explicitly:

```toml
[embeddings]
model = "local:BGESmallENV15Q"
```

### Recommended Ollama Models

| Model | Dimensions | Notes |
|---|---|---|
| `mxbai-embed-large` | 1024 | Strong general-purpose model, recommended default |
| `nomic-embed-text` | 768 | Good quality, smaller download |

---

## OpenAI

Calls the OpenAI embeddings API. Requires an active OpenAI account and an API key.

**Model string format:** `"openai:<model-name>"`

**Endpoint:** `https://api.openai.com/v1/embeddings`

**Authentication:** `$OPENAI_API_KEY` environment variable (required)

### Setup

Set your API key in the environment before starting the MCP server:

```bash
export OPENAI_API_KEY=sk-...
```

### Configuration

```toml
[embeddings]
model = "openai:text-embedding-3-small"
```

### Recommended OpenAI Models

| Model | Dimensions | Notes |
|---|---|---|
| `text-embedding-3-small` | 1536 | Low cost, good quality, recommended |
| `text-embedding-3-large` | 3072 | Highest quality, higher cost |
| `text-embedding-ada-002` | 1536 | Legacy model, still widely used |

---

## Custom Endpoint

Points at any OpenAI-compatible embeddings API — useful for self-hosted models, Azure OpenAI,
Together AI, or other third-party providers.

**Model string format:** `"custom:<model-name>@<base-url>"`

code-explorer appends `/v1/embeddings` to `<base-url>`, so a base URL of
`http://localhost:1234` becomes `http://localhost:1234/v1/embeddings`.

**Authentication:** `$EMBED_API_KEY` environment variable (optional — set it if the server
requires a bearer token)

### Setup

Start your compatible server, then set the API key if needed:

```bash
export EMBED_API_KEY=your-token-here
```

### Configuration

```toml
[embeddings]
model = "custom:mxbai-embed-large@http://localhost:1234"
```

Examples for common providers:

```toml
# Azure OpenAI
model = "custom:text-embedding-3-small@https://my-resource.openai.azure.com/openai/deployments/my-deployment"

# Together AI
model = "custom:togethercomputer/m2-bert-80M-8k-retrieval@https://api.together.xyz"

# Hugging Face Text Embeddings Inference (TEI)
model = "custom:BAAI/bge-large-en-v1.5@http://localhost:8080"
```

---

## Local (fastembed)

Runs entirely on-device using [fastembed-rs](https://github.com/Anush008/fastembed-rs) and
ONNX Runtime. No external daemon, no API key, and no network traffic after the initial model
download.

**Model string format:** `"local:<EmbeddingModel-variant>"`

**Requires:** building with the `local-embed` Cargo feature (not included in the default
`remote-embed` build)

**Model cache:** `~/.cache/huggingface/hub/` — downloaded on first use, then fully offline

### Build

Install with local embedding support:

```bash
cargo install code-explorer --features local-embed
```

Or, to have both local and remote backends available simultaneously:

```bash
cargo install code-explorer --features remote-embed,local-embed
```

### Configuration

```toml
[embeddings]
model = "local:BGESmallENV15Q"
```

### Supported Local Models

| Model string | Dimensions | Download size | Notes |
|---|---|---|---|
| `local:JinaEmbeddingsV2BaseCode` | 768 | ~300 MB | Code-specific, highest quality for source code |
| `local:BGESmallENV15Q` | 384 | ~20 MB | Quantized, fast on CPU, recommended for most users |
| `local:AllMiniLML6V2Q` | 384 | ~22 MB | Quantized, lightest, good for limited disk/RAM |
| `local:BGESmallENV15` | 384 | ~65 MB | Full f32 precision variant of BGESmallENV15Q |
| `local:AllMiniLML6V2` | 384 | ~90 MB | Full f32 precision variant of AllMiniLML6V2Q |

For most local setups, `BGESmallENV15Q` gives the best tradeoff: small download, fast CPU
inference, and solid retrieval quality. Use `JinaEmbeddingsV2BaseCode` when search quality
on code is the priority and the larger download is acceptable.

### Error When Feature Is Missing

If you try to use a `local:` model without the `local-embed` feature compiled in, you will
see an error like:

```
Local embedding requires the 'local-embed' feature.
Rebuild with: cargo build --features local-embed
```

---

## Batching

All backends send texts in batches of 8. This avoids HTTP 400 errors from servers that have
payload size limits (Ollama is particularly strict about this) and keeps per-request latency
manageable. The batch size is fixed and not configurable.

---

## Rebuilding After a Model Change

The embedding index stores which model was used to build it. If you change the `model` field
in `project.toml`, you must rebuild the index with the `force` flag to avoid mixing vectors
from different models:

```json
{ "name": "index_project", "arguments": { "force": true } }
```

code-explorer will warn if it detects a mismatch between the configured model and the model
recorded in the existing index.

---

## Choosing a Backend

A practical decision tree:

- **You want zero setup and are comfortable with a local daemon** → use the default
  `ollama:mxbai-embed-large`. If Ollama is absent, it falls back to `local:BGESmallENV15Q`
  automatically (requires the `local-embed` feature).
- **You have no GPU or just want something that works everywhere** → build with
  `--features remote-embed,local-embed` and leave the default model in place. The fallback
  kicks in automatically whenever Ollama is unreachable.
- **You want the best search quality and do not mind API costs** → use
  `openai:text-embedding-3-small`.
- **You are on an air-gapped machine or want complete data privacy** → use
  `local:BGESmallENV15Q` (build with `--features local-embed`).
- **You already run a TEI, vLLM, or similar server** → use `custom:<model>@<base-url>`.
- **You are indexing a code-heavy project and want best code retrieval** → use
  `local:JinaEmbeddingsV2BaseCode`.
