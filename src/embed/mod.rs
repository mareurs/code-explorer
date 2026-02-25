//! Embedding engine: semantic code search via local or remote embeddings.
//!
//! Inspired by cocoindex-code (../cocoindex-code/) but implemented natively
//! in Rust with sqlite-vec for zero-dependency vector storage.
//!
//! Architecture:
//!   chunker → Embedder trait → sqlite-vec index
//!
//! Two Embedder backends:
//!   - LocalEmbedder  (candle/ort, feature "local-embed") — fully offline
//!   - RemoteEmbedder (reqwest, feature "remote-embed")   — OpenAI-compatible API

pub mod chunker;
pub mod index;
pub mod schema;

#[cfg(feature = "remote-embed")]
pub mod remote;

// pub mod local; // TODO: enable with "local-embed" feature

use anyhow::Result;

/// Embedding vector — 768-dim f32 for jina-embeddings-v2-base-code.
pub type Embedding = Vec<f32>;

/// Trait implemented by all embedding backends.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Return the dimensionality of the produced vectors.
    fn dimensions(&self) -> usize;

    /// Embed a batch of texts, returning one vector per text.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>>;
}

/// Convenience extension for embedding a single text.
pub async fn embed_one(embedder: &dyn Embedder, text: &str) -> Result<Embedding> {
    let mut batch = embedder.embed(&[text]).await?;
    batch
        .pop()
        .ok_or_else(|| anyhow::anyhow!("Embedder returned empty batch"))
}

/// Create the default embedder based on a model string.
///
/// Model string format:
///   "local:<model-id>"                      → local inference (requires local-embed feature)
///   "openai:<model-id>"                     → OpenAI API
///   "ollama:<model-id>"                     → Ollama local HTTP API
///   "custom:<model-id>@<base_url>"          → generic OpenAI-compatible endpoint
///     e.g. "custom:mxbai-embed-large@http://localhost:1234"
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

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_prefix_returns_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(super::create_embedder("bogus:model"));
        let err = result.err().expect("expected an error");
        assert!(err.to_string().contains("Unknown model prefix"));
    }

    #[test]
    fn local_prefix_returns_helpful_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(super::create_embedder("local:anything"));
        let err = result.err().expect("expected an error");
        assert!(err.to_string().contains("local-embed"));
    }

    #[test]
    fn custom_prefix_missing_at_sign_returns_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(super::create_embedder("custom:no-at-sign"));
        let err = result.err().expect("expected an error");
        assert!(err.to_string().contains("custom:<model>@<base_url>"));
    }
}
