//! Embedding engine: semantic code search via local or remote embeddings.
//!
//! Inspired by cocoindex-code (../cocoindex-code/) but implemented natively
//! in Rust with sqlite-vec for zero-dependency vector storage.
//!
//! Architecture:
//!   chunker → Embedder trait → sqlite-vec index
//!
//! Two Embedder backends:
//!   - LocalEmbedder  (fastembed/ONNX, feature "local-embed") — fully offline, CPU/WSL2-friendly
//!   - RemoteEmbedder (reqwest, feature "remote-embed")   — OpenAI-compatible API

pub mod ast_chunker;
pub mod chunker;
pub mod drift;
pub mod index;
pub mod schema;

#[cfg(feature = "remote-embed")]
pub mod remote;

#[cfg(feature = "local-embed")]
pub mod local;

use anyhow::Result;

/// Embedding vector — dimensions depend on the configured model
/// (e.g. 768 for jina-embeddings-v2-base-code, 384 for bge-small).
pub type Embedding = Vec<f32>;

/// Trait implemented by all embedding backends.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Return the dimensionality of the produced vectors.
    fn dimensions(&self) -> usize;

    /// Embed a batch of texts, returning one vector per text.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>>;
}

/// Returns the chunk size in characters appropriate for the given model spec.
///
/// Derived from each model's documented maximum sequence length using a
/// conservative formula: `max_tokens × 0.85 × 3 chars/token`.
///
/// - The 0.85 factor leaves 15 % headroom for tokenisation variance and
///   control tokens (BOS/EOS).
/// - Code tokenises at roughly 3–4 chars/token; 3 is the conservative lower
///   bound, ensuring chunks stay within the context window even for files with
///   many short identifiers and operators.
///
/// Unknown or custom models fall back to 512 tokens (the most common context
/// window among small embedding models). This is intentionally conservative —
/// chunks will be smaller than necessary but will never be truncated.
///
/// This value is not user-configurable. It is derived from the model spec
/// so that users cannot accidentally misconfigure it.
pub fn chunk_size_for_model(model_spec: &str) -> usize {
    // 85 % of context × 3 chars/token.
    fn from_tokens(n: usize) -> usize {
        (n as f64 * 0.85 * 3.0) as usize
    }

    // Map well-known model name substrings to their published max sequence
    // lengths. Matching is done on the bare model name (prefix stripped) so
    // that "ollama:nomic-embed-text" and "openai:nomic-embed-text" both match.
    fn tokens_for_bare(name: &str) -> usize {
        let l = name.to_lowercase();
        // 8 192-token models
        if l.contains("nomic-embed") || l.contains("jina") {
            return 8192;
        }
        // OpenAI text-embedding-3-* and text-embedding-ada-002
        if l.starts_with("text-embedding-") {
            return 8191;
        }
        // mxbai-embed-large (MixedBread)
        if l.contains("mxbai") {
            return 512;
        }
        // BGE Small variants
        if l.contains("bge-small") || l.starts_with("bge_small") {
            return 512;
        }
        // all-MiniLM-L6-v2
        if l.contains("all-minilm") || l.contains("minilm-l6") {
            return 256;
        }
        // Unknown — conservative fallback
        512
    }

    // Local fastembed models use their documented sequence lengths.
    // These are listed here rather than in local.rs to avoid a feature-gate
    // dependency (local.rs is #[cfg(feature = "local-embed")]).
    if let Some(local_name) = model_spec.strip_prefix("local:") {
        let max_tokens = match local_name.to_lowercase().as_str() {
            "jinaembeddingsv2basecode" => 8192,
            "bgesmallenv15q" | "bgesmallenv15" => 512,
            "allminilml6v2q" | "allminilml6v2" => 256,
            _ => 512,
        };
        return from_tokens(max_tokens);
    }

    // Strip backend prefix to get the bare model name.
    let bare = model_spec
        .strip_prefix("ollama:")
        .or_else(|| model_spec.strip_prefix("openai:"))
        .or_else(|| {
            // "custom:model-name@base_url" — extract only the model-name part
            model_spec
                .strip_prefix("custom:")
                .map(|rest| rest.split('@').next().unwrap_or(rest))
        })
        .unwrap_or(model_spec);

    from_tokens(tokens_for_bare(bare))
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
        // When the local-embed feature is compiled in, probe Ollama before
        // committing to it. A missing or stopped daemon is the most common
        // reason embedding silently fails on machines without a GPU/Ollama
        // setup, so we fall back to a CPU-friendly quantized local model.
        #[cfg(feature = "local-embed")]
        {
            let host =
                std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
            if let Err(e) = remote::probe_ollama(&host).await {
                const FALLBACK: &str = "BGESmallENV15Q";
                tracing::warn!(
                    "{e}. Falling back to local:{FALLBACK} (CPU-friendly, ~20 MB). \
                     Set embeddings.model in .code-explorer/project.toml to suppress this."
                );
                return Ok(Box::new(local::LocalEmbedder::new(FALLBACK)?));
            }
        }
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
        return Ok(Box::new(remote::RemoteEmbedder::custom(
            base_url, model_id,
        )?));
    }

    #[cfg(feature = "local-embed")]
    if let Some(model_id) = model.strip_prefix("local:") {
        return Ok(Box::new(local::LocalEmbedder::new(model_id)?));
    }

    if model.starts_with("local:") {
        anyhow::bail!(
            "Local embedding requires the 'local-embed' feature.\n\
             Rebuild with: cargo build --features local-embed\n\n\
             Recommended (code-specific, CPU/WSL2):\n\
             • local:JinaEmbeddingsV2BaseCode   (768d, ~300MB)\n\
             • local:BGESmallENV15Q             (384d, quantized, ~20MB, fast)"
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

    #[cfg(not(feature = "local-embed"))]
    #[test]
    fn local_prefix_returns_helpful_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(super::create_embedder("local:anything"));
        let err = result.err().expect("expected an error");
        assert!(err.to_string().contains("local-embed"));
    }

    #[cfg(feature = "remote-embed")]
    #[test]
    fn custom_prefix_missing_at_sign_returns_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(super::create_embedder("custom:no-at-sign"));
        let err = result.err().expect("expected an error");
        assert!(err.to_string().contains("custom:<model>@<base_url>"));
    }

    // ---------- chunk_size_for_model ----------

    #[test]
    fn chunk_size_mxbai_embed_large() {
        // Default model: 512-token context. Formula: 512 × 0.85 × 3 = 1305.
        let sz = super::chunk_size_for_model("ollama:mxbai-embed-large");
        assert_eq!(sz, 1305);
    }

    #[test]
    fn chunk_size_nomic_embed_text() {
        // 8 192-token context. Formula: 8192 × 0.85 × 3 = 20 889.
        let sz = super::chunk_size_for_model("ollama:nomic-embed-text");
        assert_eq!(sz, 20889);
    }

    #[test]
    fn chunk_size_openai_text_embedding_3_small() {
        let sz = super::chunk_size_for_model("openai:text-embedding-3-small");
        assert_eq!(sz, 20887); // 8191 × 0.85 × 3
    }

    #[test]
    fn chunk_size_local_jina() {
        let sz = super::chunk_size_for_model("local:JinaEmbeddingsV2BaseCode");
        assert_eq!(sz, 20889); // 8192 × 0.85 × 3
    }

    #[test]
    fn chunk_size_local_bge_small() {
        let sz = super::chunk_size_for_model("local:BGESmallENV15Q");
        assert_eq!(sz, 1305); // 512 × 0.85 × 3
    }

    #[test]
    fn chunk_size_local_all_minilm() {
        let sz = super::chunk_size_for_model("local:AllMiniLML6V2Q");
        assert_eq!(sz, 652); // 256 × 0.85 × 3
    }

    #[test]
    fn chunk_size_custom_model() {
        // custom: prefix with @url — model name extracted before @
        let sz = super::chunk_size_for_model("custom:mxbai-embed-large@http://localhost:1234");
        assert_eq!(sz, 1305);
    }

    #[test]
    fn chunk_size_unknown_model_falls_back_to_512_tokens() {
        let sz = super::chunk_size_for_model("ollama:some-unknown-model");
        assert_eq!(sz, 1305); // 512 × 0.85 × 3
    }
}
