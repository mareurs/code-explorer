# Local Embedding Cleanup Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Switch the default Ollama model to `mxbai-embed-large` and fix two bugs in `create_embedder` (broken `http:` prefix handling, wrong model name in error message).

**Architecture:** Two-file change. `config/project.rs` holds the default model string. `embed/mod.rs` holds `create_embedder` which parses the `provider:model` config string and dispatches to the right `RemoteEmbedder` constructor. The `http:` prefix is replaced with `custom:` using a `model@base_url` format.

**Tech Stack:** Rust, `tokio` (async), `reqwest` (HTTP), `rusqlite` (SQLite). Run with `cargo test` and `cargo clippy -- -D warnings`.

---

### Task 1: Fix `create_embedder` — replace `http:` with `custom:` and fix URL parsing

**Files:**
- Modify: `src/embed/mod.rs`

The current `http:` branch strips `"http:"` from the model string, producing a broken URL (`"//host:port"`), and passes the whole original string as the model name. Both are wrong.

New format: `custom:<model_name>@<base_url>` — e.g. `custom:mxbai-embed-large@http://localhost:1234`.

**Step 1: Write the failing test**

Add a `#[cfg(test)]` module at the bottom of `src/embed/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    // create_embedder is async and requires the remote-embed feature.
    // We test the error paths (which are synchronous logic) synchronously,
    // and the happy paths via the error message (we can't make real HTTP calls in unit tests).

    #[test]
    fn unknown_prefix_returns_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(super::create_embedder("bogus:model"))
            .unwrap_err();
        assert!(err.to_string().contains("Unknown model prefix"));
    }

    #[test]
    fn local_prefix_returns_helpful_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(super::create_embedder("local:anything"))
            .unwrap_err();
        assert!(err.to_string().contains("local-embed"));
    }

    #[test]
    fn custom_prefix_missing_at_sign_returns_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(super::create_embedder("custom:no-at-sign"))
            .unwrap_err();
        assert!(err.to_string().contains("custom:<model>@<base_url>"));
    }
}
```

**Step 2: Run tests to confirm they fail (or that the new test compiles)**

```bash
cd /home/marius/work/claude/code-explorer
cargo test embed::tests -- --nocapture 2>&1 | tail -20
```

Expected: compile error because `custom:` branch doesn't exist yet. That's the failing state.

**Step 3: Implement the fix**

In `src/embed/mod.rs`, replace the `http:` branch with a `custom:` branch. The full `create_embedder` function should look like this:

```rust
pub async fn create_embedder(model: &str) -> Result<Box<dyn Embedder>> {
    #[cfg(feature = "remote-embed")]
    if let Some(model_id) = model.strip_prefix("openai:") {
        return Ok(Box::new(remote::RemoteEmbedder::openai(model_id)?));
    }
    #[cfg(feature = "remote-embed")]
    if let Some(model_id) = model.strip_prefix("ollama:") {
        return Ok(Box::new(remote::RemoteEmbedder::ollama(model_id)?));
    }
    #[cfg(feature = "remote-embed")]
    if let Some(rest) = model.strip_prefix("custom:") {
        let (model_id, base_url) = rest.split_once('@').ok_or_else(|| {
            anyhow::anyhow!(
                "custom: format is 'custom:<model>@<base_url>', e.g. \
                 'custom:mxbai-embed-large@http://localhost:1234'"
            )
        })?;
        return Ok(Box::new(remote::RemoteEmbedder::custom(base_url, model_id)?));
    }

    if model.starts_with("local:") {
        anyhow::bail!(
            "Local embedding requires the 'local-embed' feature. \
             Rebuild with: cargo build --features local-embed\n\
             Alternatively use an Ollama model: ollama:mxbai-embed-large"
        );
    }

    anyhow::bail!(
        "Unknown model prefix in '{}'. Supported: 'ollama:', 'openai:', 'custom:', 'local:'.",
        model
    )
}
```

**Step 4: Run the tests and clippy**

```bash
cargo test embed::tests -- --nocapture 2>&1 | tail -20
```
Expected: all 3 tests PASS.

```bash
cargo clippy -- -D warnings 2>&1 | tail -10
```
Expected: no warnings.

**Step 5: Commit**

```bash
git add src/embed/mod.rs
git commit -m "Fix custom embed prefix: replace broken http: with custom:<model>@<url>"
```

---

### Task 2: Change default embedding model to `mxbai-embed-large`

**Files:**
- Modify: `src/config/project.rs`

**Step 1: Write the failing test**

Add a `#[cfg(test)]` module at the bottom of `src/config/project.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_is_mxbai() {
        assert_eq!(default_embed_model(), "ollama:mxbai-embed-large");
    }

    #[test]
    fn default_config_has_mxbai_model() {
        let cfg = ProjectConfig::default_for("my-project".into());
        assert_eq!(cfg.embeddings.model, "ollama:mxbai-embed-large");
    }
}
```

**Step 2: Run test to confirm it fails**

```bash
cargo test config::tests -- --nocapture 2>&1 | tail -10
```
Expected: FAIL — `default_embed_model` returns `"ollama:nomic-embed-text"`, not `"ollama:mxbai-embed-large"`.

**Step 3: Change the default**

In `src/config/project.rs`, update `default_embed_model`:

```rust
fn default_embed_model() -> String {
    "ollama:mxbai-embed-large".into()
}
```

**Step 4: Run tests**

```bash
cargo test config::tests -- --nocapture 2>&1 | tail -10
```
Expected: both tests PASS.

```bash
cargo test 2>&1 | tail -5
```
Expected: all tests pass, no regressions.

**Step 5: Commit**

```bash
git add src/config/project.rs
git commit -m "Switch default embedding model to ollama:mxbai-embed-large"
```

---

### Task 3: Final verification

**Step 1: Full test + lint pass**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt --check
```
Expected: all green, no warnings, no formatting issues.

**Step 2: Smoke test the index command**

Ensure Ollama is running with mxbai-embed-large pulled:

```bash
ollama pull mxbai-embed-large
```

Then run the indexer against the project itself:

```bash
cargo run -- index --project . 2>&1 | tail -5
```
Expected: `Index complete: N files indexed, M unchanged` with no errors.

**Step 3: Tag completion**

No additional commit needed — everything is already committed.
