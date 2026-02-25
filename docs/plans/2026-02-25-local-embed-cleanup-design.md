# Design: Local-only embedding cleanup

**Date:** 2026-02-25
**Status:** Approved
**Scope:** Option B — default model change + bug fixes in `create_embedder`

## Context

The project uses Ollama as its local embedding provider. The current default model
(`nomic-embed-text`, MTEB 53.01) is suboptimal; `mxbai-embed-large` scores 64.68 on
MTEB — matching OpenAI text-embedding-3-large — and is available in the official Ollama
library. Additionally, two bugs exist in `create_embedder` that are unrelated to the
model change but should be fixed while touching the same files.

## Goals

- Better out-of-the-box embedding quality with zero extra setup cost (Ollama is still required)
- No cloud API dependency by default
- Fix known bugs before they confuse future contributors

## Non-Goals

- Adding in-process (candle/fastembed) embeddings — separate concern
- Removing OpenAI support — YAGNI, it already exists
- Changing the config file format or adding new config fields

## Changes

### 1. Default model (`src/config/project.rs`)

`default_embed_model()` returns `"ollama:mxbai-embed-large"` instead of
`"ollama:nomic-embed-text"`.

Only affects projects that have no `.code-explorer/project.toml` (new projects or
projects without a config). Existing indexed projects are unaffected.

### 2. Fix `http:` prefix bug and rename to `custom:` (`src/embed/mod.rs`)

**Current behaviour (broken):**
```
model = "http://localhost:1234"
model.strip_prefix("http:") → "//localhost:1234"   ← broken URL
RemoteEmbedder::custom("//localhost:1234", "http://localhost:1234")  ← both args wrong
```

**New format:** `custom:<model>@<base_url>`

Examples:
- `custom:mxbai-embed-large@http://localhost:1234`
- `custom:my-model@https://lm-studio.internal:8080`

The `@` separator is unambiguous (model names never contain `@`). Base URL is everything
after the first `@`. `EMBED_API_KEY` env var continues to work for auth.

**Parser:**
```rust
if let Some(rest) = model.strip_prefix("custom:") {
    let (model_id, base_url) = rest
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("custom: format is 'custom:<model>@<base_url>'"))?;
    return Ok(Box::new(remote::RemoteEmbedder::custom(base_url, model_id)?));
}
```

### 3. Fix error message in `local:` branch (`src/embed/mod.rs`)

Change `ollama:nomic-embed-code` (model does not exist in the official Ollama library)
to `ollama:mxbai-embed-large`.

## Files Changed

| File | Change |
|------|--------|
| `src/config/project.rs` | `default_embed_model()` → `"ollama:mxbai-embed-large"` |
| `src/embed/mod.rs` | Replace `http:` with `custom:`, fix URL parsing, fix error message |

## Testing

- `cargo test` must pass
- `cargo clippy -- -D warnings` must pass
- Manual: `cargo run -- index --project .` with `ollama:mxbai-embed-large` in config
