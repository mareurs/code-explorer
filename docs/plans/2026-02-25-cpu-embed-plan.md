# CPU-Friendly Local Embedding Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `local:<EmbeddingModel>` embedding support via `fastembed-rs` (ONNX, CPU/WSL2-friendly) behind a `local-embed` feature flag, plus model-change mismatch detection for all backends.

**Architecture:** New `src/embed/local.rs` wraps `fastembed::TextEmbedding` and implements the `Embedder` trait. A `meta` SQLite table stores the active model string; `build_index()` checks it on every run and errors clearly if the model changed. The `local:` prefix in `create_embedder()` dispatches to `LocalEmbedder`.

**Tech Stack:** Rust, `fastembed` crate v4 (ONNX Runtime + HuggingFace model hub), `rusqlite`, `tokio::task::spawn_blocking` (fastembed embed is sync).

---

### Task 1: Add `meta` table and get/set helpers

**Files:**
- Modify: `src/embed/index.rs`

**Step 1: Write the failing tests**

At the bottom of the `tests` module in `src/embed/index.rs`, add:

```rust
#[test]
fn open_db_creates_meta_table() {
    let (_dir, conn) = open_test_db();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn meta_get_missing_key_returns_none() {
    let (_dir, conn) = open_test_db();
    let val = get_meta(&conn, "embed_model").unwrap();
    assert!(val.is_none());
}

#[test]
fn meta_set_then_get_roundtrip() {
    let (_dir, conn) = open_test_db();
    set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
    let val = get_meta(&conn, "embed_model").unwrap();
    assert_eq!(val.as_deref(), Some("ollama:mxbai-embed-large"));
}

#[test]
fn meta_set_overwrites_existing_value() {
    let (_dir, conn) = open_test_db();
    set_meta(&conn, "embed_model", "old-model").unwrap();
    set_meta(&conn, "embed_model", "new-model").unwrap();
    let val = get_meta(&conn, "embed_model").unwrap();
    assert_eq!(val.as_deref(), Some("new-model"));
}
```

**Step 2: Run to confirm failure**

```bash
cargo test -p code-explorer embed::index::tests::open_db_creates_meta_table 2>&1 | tail -5
```

Expected: `FAILED` — table `meta` does not exist.

**Step 3: Add `meta` table to `open_db()`**

In `src/embed/index.rs`, inside the `conn.execute_batch(...)` SQL string in `open_db()`, append after the existing `chunk_embeddings` table creation:

```sql
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

**Step 4: Add `get_meta` and `set_meta` functions**

Add these two public functions anywhere before the `tests` module:

```rust
/// Read a value from the `meta` key-value table.
pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Write (insert or replace) a value in the `meta` key-value table.
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value],
    )?;
    Ok(())
}
```

**Step 5: Run the tests to confirm they pass**

```bash
cargo test -p code-explorer embed::index::tests::open_db_creates_meta_table
cargo test -p code-explorer embed::index::tests::meta_
```

Expected: all 4 new tests `PASS`.

**Step 6: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): add meta table with get_meta/set_meta helpers"
```

---

### Task 2: Model mismatch detection in `build_index`

**Files:**
- Modify: `src/embed/index.rs`

**Step 1: Write the failing tests**

Add to the `tests` module:

```rust
#[test]
fn check_model_mismatch_first_run_is_ok() {
    let (_dir, conn) = open_test_db();
    // No meta entry yet — first run should succeed
    assert!(check_model_mismatch(&conn, "ollama:mxbai-embed-large").is_ok());
}

#[test]
fn check_model_mismatch_same_model_is_ok() {
    let (_dir, conn) = open_test_db();
    set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
    assert!(check_model_mismatch(&conn, "ollama:mxbai-embed-large").is_ok());
}

#[test]
fn check_model_mismatch_different_model_is_err() {
    let (_dir, conn) = open_test_db();
    set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
    let err = check_model_mismatch(&conn, "local:JinaEmbeddingsV2BaseCode")
        .unwrap_err()
        .to_string();
    assert!(err.contains("ollama:mxbai-embed-large"), "error should name stored model");
    assert!(err.contains("local:JinaEmbeddingsV2BaseCode"), "error should name new model");
    assert!(err.contains("embeddings.db"), "error should hint at DB deletion");
}
```

**Step 2: Run to confirm failure**

```bash
cargo test -p code-explorer embed::index::tests::check_model_mismatch 2>&1 | tail -5
```

Expected: `FAILED` — `check_model_mismatch` not defined.

**Step 3: Add `check_model_mismatch` function**

Add before the `tests` module:

```rust
/// Return an error if the index was built with a different embedding model.
///
/// Call this at the start of `build_index` before processing any files.
/// Returns `Ok(())` when:
///   - no model has been stored yet (first run), OR
///   - the stored model matches `configured`
pub fn check_model_mismatch(conn: &Connection, configured: &str) -> Result<()> {
    match get_meta(conn, "embed_model")? {
        None => Ok(()), // first run
        Some(stored) if stored == configured => Ok(()),
        Some(stored) => anyhow::bail!(
            "Index was built with model '{stored}'.\n\
             Configured model is '{configured}'.\n\
             Delete .code-explorer/embeddings.db and re-run `index` to rebuild."
        ),
    }
}
```

**Step 4: Wire into `build_index()`**

In `build_index()`, immediately after `let embedder = ...` (around line 230), add:

```rust
check_model_mismatch(&conn, &config.embeddings.model)?;
```

Then at the very end of `build_index()`, after all DB writes succeed, add:

```rust
set_meta(&conn, "embed_model", &config.embeddings.model)?;
```

**Step 5: Run the tests**

```bash
cargo test -p code-explorer embed::index::tests::check_model_mismatch
```

Expected: all 3 new tests `PASS`.

**Step 6: Run full test suite to catch regressions**

```bash
cargo test -p code-explorer 2>&1 | tail -10
```

Expected: all tests pass.

**Step 7: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): check model mismatch before indexing, store model in meta"
```

---

### Task 3: Surface stored model in `IndexStats` and `index_status` tool

**Files:**
- Modify: `src/embed/index.rs`
- Modify: `src/tools/semantic.rs`

**Step 1: Write the failing test**

Add to the `tests` module in `src/embed/index.rs`:

```rust
#[test]
fn index_stats_returns_stored_model() {
    let (_dir, conn) = open_test_db();
    set_meta(&conn, "embed_model", "ollama:mxbai-embed-large").unwrap();
    let stats = index_stats(&conn).unwrap();
    assert_eq!(stats.model.as_deref(), Some("ollama:mxbai-embed-large"));
}

#[test]
fn index_stats_model_is_none_when_unset() {
    let (_dir, conn) = open_test_db();
    let stats = index_stats(&conn).unwrap();
    assert!(stats.model.is_none());
}
```

**Step 2: Run to confirm failure**

```bash
cargo test -p code-explorer embed::index::tests::index_stats_returns_stored_model 2>&1 | tail -5
```

Expected: `FAILED` — `IndexStats` has no `model` field.

**Step 3: Update `IndexStats` and `index_stats()`**

In `src/embed/index.rs`, update the struct:

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexStats {
    pub file_count: usize,
    pub chunk_count: usize,
    pub embedding_count: usize,
    /// Model string stored at index time, if any.
    pub model: Option<String>,
}
```

Update `index_stats()`:

```rust
pub fn index_stats(conn: &Connection) -> Result<IndexStats> {
    let file_count: usize = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let chunk_count: usize = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let embedding_count: usize =
        conn.query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))?;
    let model = get_meta(conn, "embed_model")?;
    Ok(IndexStats {
        file_count,
        chunk_count,
        embedding_count,
        model,
    })
}
```

**Step 4: Update `IndexStatus` tool response**

In `src/tools/semantic.rs`, in the `Ok(json!({...}))` return of `IndexStatus::call()`, add `"indexed_with_model"` from the DB stats. The `model` field currently reads from config — keep it, add the DB field alongside:

```rust
Ok(json!({
    "indexed": true,
    "configured_model": model,        // from project.toml
    "indexed_with_model": stats.model, // from meta table (may differ if config changed)
    "file_count": stats.file_count,
    "chunk_count": stats.chunk_count,
    "embedding_count": stats.embedding_count,
    "db_path": db_path.display().to_string(),
}))
```

**Step 5: Run the tests**

```bash
cargo test -p code-explorer embed::index::tests::index_stats_
cargo test -p code-explorer 2>&1 | tail -10
```

Expected: all tests pass.

**Step 6: Commit**

```bash
git add src/embed/index.rs src/tools/semantic.rs
git commit -m "feat(embed): surface indexed_with_model in index_stats and index_status tool"
```

---

### Task 4: Add `fastembed` dependency and `LocalEmbedder`

**Files:**
- Modify: `Cargo.toml`
- Create: `src/embed/local.rs`
- Modify: `src/embed/mod.rs`

**Step 1: Add fastembed to `Cargo.toml`**

Replace the commented-out `candle` block (lines 80-81) with:

```toml
[features]
default = ["remote-embed"]
# Remote embedding via OpenAI-compatible HTTP API (Ollama, OpenAI, etc.)
remote-embed = ["dep:reqwest"]
# Local CPU embedding via fastembed-rs (ONNX Runtime + HuggingFace model hub).
# First use downloads the chosen model (~20-300MB) to ~/.cache/huggingface/hub/.
local-embed = ["dep:fastembed"]
```

And in `[dependencies]`, add alongside `reqwest`:

```toml
# Local CPU embeddings via ONNX Runtime (fastembed model hub)
fastembed = { version = "4", optional = true }
```

Verify it compiles:

```bash
cargo build --features local-embed 2>&1 | tail -5
```

Expected: compiles (fastembed downloads as a dependency).

**Step 2: Write a failing test for unknown model name**

Create `src/embed/local.rs` with just:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_unknown_name_returns_error() {
        let err = parse_model("NotARealModel").unwrap_err().to_string();
        assert!(err.contains("NotARealModel"));
        assert!(err.contains("JinaEmbeddingsV2BaseCode"), "error should list supported models");
    }

    #[test]
    fn parse_model_known_names_return_ok() {
        assert!(parse_model("JinaEmbeddingsV2BaseCode").is_ok());
        assert!(parse_model("BGESmallENV15Q").is_ok());
        assert!(parse_model("AllMiniLML6V2Q").is_ok());
        assert!(parse_model("SnowflakeArcticEmbedXSQ").is_ok());
    }
}
```

Run to confirm failure:

```bash
cargo test --features local-embed -p code-explorer embed::local::tests 2>&1 | tail -5
```

Expected: `FAILED` — `parse_model` not defined.

**Step 3: Implement `local.rs`**

Replace the content of `src/embed/local.rs` with:

```rust
//! Local CPU embedding via fastembed-rs (ONNX Runtime).
//!
//! Model strings use fastembed's `EmbeddingModel` variant names directly,
//! e.g. `local:JinaEmbeddingsV2BaseCode` or `local:BGESmallENV15Q`.
//! Models are downloaded on first use to `~/.cache/huggingface/hub/`.

use anyhow::Result;
use std::sync::Arc;

use crate::embed::Embedding;

pub struct LocalEmbedder {
    model: Arc<fastembed::TextEmbedding>,
    dims: usize,
}

impl LocalEmbedder {
    pub fn new(model_name: &str) -> Result<Self> {
        let embedding_model = parse_model(model_name)?;
        let model = fastembed::TextEmbedding::try_new(
            fastembed::InitOptions::new(embedding_model),
        )?;
        // Derive actual dims by embedding a probe string.
        let probe = model.embed(vec!["probe".to_string()], None)?;
        let dims = probe.first().map(|v| v.len()).unwrap_or(0);
        Ok(Self {
            model: Arc::new(model),
            dims,
        })
    }
}

fn parse_model(name: &str) -> Result<fastembed::EmbeddingModel> {
    match name {
        "JinaEmbeddingsV2BaseCode"  => Ok(fastembed::EmbeddingModel::JinaEmbeddingsV2BaseCode),
        "BGESmallENV15Q"            => Ok(fastembed::EmbeddingModel::BGESmallENV15Q),
        "AllMiniLML6V2Q"            => Ok(fastembed::EmbeddingModel::AllMiniLML6V2Q),
        "SnowflakeArcticEmbedXSQ"   => Ok(fastembed::EmbeddingModel::SnowflakeArcticEmbedXSQ),
        // Non-quantized variants for users who want full f32 precision
        "BGESmallENV15"             => Ok(fastembed::EmbeddingModel::BGESmallENV15),
        "AllMiniLML6V2"             => Ok(fastembed::EmbeddingModel::AllMiniLML6V2),
        other => anyhow::bail!(
            "Unknown local model '{other}'. Supported variants:\n\
             • local:JinaEmbeddingsV2BaseCode   (768d, code-specific, ~300MB, recommended)\n\
             • local:BGESmallENV15Q             (384d, quantized, ~20MB, fast CPU)\n\
             • local:AllMiniLML6V2Q             (384d, quantized, ~22MB, lightest)\n\
             • local:SnowflakeArcticEmbedXSQ    (384d, quantized, tiny but strong)\n\
             • local:BGESmallENV15              (384d, full precision)\n\
             • local:AllMiniLML6V2              (384d, full precision)"
        ),
    }
}

#[async_trait::async_trait]
impl crate::embed::Embedder for LocalEmbedder {
    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let model = Arc::clone(&self.model);
        tokio::task::spawn_blocking(move || {
            model.embed(owned, None).map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_unknown_name_returns_error() {
        let err = parse_model("NotARealModel").unwrap_err().to_string();
        assert!(err.contains("NotARealModel"));
        assert!(err.contains("JinaEmbeddingsV2BaseCode"), "error should list supported models");
    }

    #[test]
    fn parse_model_known_names_return_ok() {
        assert!(parse_model("JinaEmbeddingsV2BaseCode").is_ok());
        assert!(parse_model("BGESmallENV15Q").is_ok());
        assert!(parse_model("AllMiniLML6V2Q").is_ok());
        assert!(parse_model("SnowflakeArcticEmbedXSQ").is_ok());
    }
}
```

**Step 4: Register the module and wire `create_embedder()`**

In `src/embed/mod.rs`:

1. Add the module declaration (inside `#[cfg(feature = "local-embed")]`):

```rust
#[cfg(feature = "local-embed")]
pub mod local;
```

2. In `create_embedder()`, add the `local:` branch **before** the existing `local:` bail:

```rust
#[cfg(feature = "local-embed")]
if let Some(model_id) = model.strip_prefix("local:") {
    return Ok(Box::new(local::LocalEmbedder::new(model_id)?));
}
```

3. Update the existing bail message for the missing-feature case:

```rust
if model.starts_with("local:") {
    anyhow::bail!(
        "Local embedding requires the 'local-embed' feature.\n\
         Rebuild with: cargo build --features local-embed\n\n\
         Recommended (code-specific, CPU/WSL2):\n\
         • local:JinaEmbeddingsV2BaseCode   (768d, ~300MB)\n\
         • local:BGESmallENV15Q             (384d, quantized, ~20MB, fast)"
    );
}
```

**Step 5: Run the unit tests**

```bash
cargo test --features local-embed -p code-explorer embed::local::tests
```

Expected: `parse_model_unknown_name_returns_error` and `parse_model_known_names_return_ok` both `PASS`.

**Step 6: Run full test suite with and without the feature**

```bash
cargo test -p code-explorer 2>&1 | tail -10
cargo test --features local-embed -p code-explorer 2>&1 | tail -10
```

Expected: all tests pass in both configurations.

**Step 7: Lint**

```bash
cargo clippy --features local-embed -- -D warnings 2>&1 | tail -10
cargo fmt
```

Expected: no warnings or errors.

**Step 8: Commit**

```bash
git add Cargo.toml src/embed/local.rs src/embed/mod.rs
git commit -m "feat(embed): add local-embed feature with fastembed-rs LocalEmbedder"
```

---

### Task 5: Update config docs and error messages

**Files:**
- Modify: `src/config/project.rs`

**Step 1: Update the doc comment on `EmbeddingsSection.model`**

Find the `model` field in `EmbeddingsSection` and replace its doc comment with:

```rust
/// Model identifier — prefix determines the backend:
///   "ollama:<model>"                    → Ollama local daemon (default)
///   "openai:<model>"                    → OpenAI API (requires OPENAI_API_KEY)
///   "custom:<model>@<base_url>"         → Any OpenAI-compatible endpoint
///   "local:<EmbeddingModel variant>"    → fastembed-rs, no daemon needed,
///                                         CPU/WSL2-friendly. Downloads model
///                                         on first use to ~/.cache/huggingface/
///
/// Recommended local models (rebuild with: cargo build --features local-embed):
///   "local:JinaEmbeddingsV2BaseCode"    → 768d, code-specific, ~300MB
///   "local:BGESmallENV15Q"              → 384d, quantized, ~20MB, fast CPU
///   "local:AllMiniLML6V2Q"              → 384d, quantized, ~22MB, lightest
///   "local:SnowflakeArcticEmbedXSQ"     → 384d, quantized, tiny but strong
pub model: String,
```

**Step 2: Update the `Embedding` type alias comment in `src/embed/mod.rs`**

Find:
```rust
/// Embedding vector — 768-dim f32 for jina-embeddings-v2-base-code.
pub type Embedding = Vec<f32>;
```

Replace with:
```rust
/// Embedding vector — dimensions depend on the configured model
/// (e.g. 768 for jina-embeddings-v2-base-code, 384 for bge-small).
pub type Embedding = Vec<f32>;
```

**Step 3: Run full test suite + lint**

```bash
cargo test -p code-explorer 2>&1 | tail -10
cargo test --features local-embed -p code-explorer 2>&1 | tail -10
cargo clippy --features local-embed -- -D warnings
cargo fmt
```

Expected: all tests pass, no warnings.

**Step 4: Commit**

```bash
git add src/config/project.rs src/embed/mod.rs
git commit -m "docs(embed): document local: prefix with supported fastembed model variants"
```

---

## Verification

After all tasks, confirm:

```bash
# Default build still works (no local-embed)
cargo build
cargo test

# local-embed builds clean
cargo build --features local-embed
cargo test --features local-embed
cargo clippy --features local-embed -- -D warnings
```

The feature is intentionally not in `default` — users who want zero-setup CPU embedding
explicitly opt in with `--features local-embed`. The design doc at
`docs/plans/2026-02-25-cpu-embed-design.md` has the full rationale.
