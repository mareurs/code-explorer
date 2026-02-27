//! Remote embedding via OpenAI-compatible HTTP API.
//!
//! Works with OpenAI, Ollama, LM Studio, and any other server that
//! implements the `/v1/embeddings` endpoint.

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{Embedder, Embedding};

pub struct RemoteEmbedder {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
    index: usize,
}

impl RemoteEmbedder {
    /// Build a reqwest client with a per-request timeout so that a hung
    /// embedding server (e.g. Ollama during GPU discovery failure) doesn't
    /// block `index_project` forever.
    fn http_client() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client")
    }

    pub fn openai(model: &str) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY env var not set"))?;
        Ok(Self {
            client: Self::http_client(),
            endpoint: "https://api.openai.com/v1/embeddings".into(),
            model: model.to_string(),
            api_key: Some(api_key),
        })
    }

    pub fn ollama(model: &str) -> Result<Self> {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
        Ok(Self {
            client: Self::http_client(),
            endpoint: format!("{}/v1/embeddings", host.trim_end_matches('/')),
            model: model.to_string(),
            api_key: None,
        })
    }

    pub fn custom(base_url: &str, model: &str) -> Result<Self> {
        let endpoint = format!("{}/v1/embeddings", base_url.trim_end_matches('/'));
        Ok(Self {
            client: Self::http_client(),
            endpoint,
            model: model.to_string(),
            api_key: std::env::var("EMBED_API_KEY").ok(),
        })
    }
}

#[async_trait::async_trait]
impl Embedder for RemoteEmbedder {
    fn dimensions(&self) -> usize {
        // Unknown at construction time for remote models; callers should
        // derive dimensions from the first response.
        0
    }

    async fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        // Send in small batches to avoid HTTP 400 from servers with payload limits (e.g. Ollama).
        const BATCH_SIZE: usize = 8;
        let mut all = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH_SIZE) {
            let mut req = self
                .client
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .json(&EmbedRequest {
                    model: &self.model,
                    input: batch,
                });
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let resp: EmbedResponse = req.send().await?.error_for_status()?.json().await?;
            let mut data = resp.data;
            data.sort_by_key(|d| d.index);
            all.extend(data.into_iter().map(|d| d.embedding));
        }
        Ok(all)
    }
}

/// Probe whether the Ollama daemon is reachable at the given host URL.
///
/// Issues a GET to the Ollama root with a 2-second timeout. Used by
/// `create_embedder` to detect when Ollama is absent and fall back to a
/// local CPU model. Returns `Ok(())` on any HTTP response (even 4xx/5xx —
/// the daemon is at least up), or an error if the connection is refused or
/// times out.
pub async fn probe_ollama(host: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    client
        .get(host.trim_end_matches('/'))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Ollama not reachable at {}: {}", host, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "nomic-embed-text";

    fn make_embedder() -> RemoteEmbedder {
        RemoteEmbedder::ollama(MODEL).unwrap()
    }

    async fn embed_one(text: &str) -> Vec<f32> {
        let mut results = make_embedder().embed(&[text]).await.expect("embed failed");
        results.pop().expect("empty response")
    }

    #[tokio::test]
    #[ignore = "requires running Ollama"]
    async fn ollama_returns_nonzero_dimensions() {
        let vec = embed_one("fn main() {}").await;
        assert!(!vec.is_empty(), "embedding should be non-empty");
        assert!(
            vec.iter().any(|&v| v != 0.0),
            "embedding should be non-zero"
        );
    }

    #[tokio::test]
    #[ignore = "requires running Ollama"]
    async fn ollama_batch_consistent_dimensions() {
        let embedder = make_embedder();
        let texts = &["fn main() {}", "struct Config {}", "impl Foo for Bar {}"];
        let results = embedder.embed(texts).await.expect("embed failed");
        assert_eq!(results.len(), texts.len(), "one vector per input");
        let dims = results[0].len();
        assert!(dims > 0);
        assert!(
            results.iter().all(|v| v.len() == dims),
            "all vectors same dims"
        );
    }

    #[tokio::test]
    #[ignore = "requires running Ollama"]
    async fn ollama_different_texts_produce_different_vectors() {
        let a = embed_one("fn authenticate_user(password: &str) -> bool").await;
        let b = embed_one("SELECT * FROM orders WHERE status = 'pending'").await;
        let l1_diff: f32 = a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum();
        assert!(
            l1_diff > 1.0,
            "distinct texts should produce distinct embeddings (diff={l1_diff:.3})"
        );
    }

    #[tokio::test]
    #[ignore = "requires running Ollama"]
    async fn ollama_similar_texts_score_higher_than_unrelated() {
        fn cosine(a: &[f32], b: &[f32]) -> f32 {
            let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
            let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            if na == 0.0 || nb == 0.0 {
                return 0.0;
            }
            (dot / (na * nb)).clamp(-1.0, 1.0)
        }

        let auth1 = embed_one("fn check_password(hash: &str, input: &str) -> bool").await;
        let auth2 = embed_one("fn verify_credentials(username: &str, pwd: &str) -> bool").await;
        let unrelated = embed_one("CREATE TABLE products (id INT, price DECIMAL)").await;

        let sim_related = cosine(&auth1, &auth2);
        let sim_unrelated = cosine(&auth1, &unrelated);
        assert!(
            sim_related > sim_unrelated,
            "semantically similar code should score higher: {sim_related:.3} vs {sim_unrelated:.3}"
        );
    }

    #[tokio::test]
    #[ignore = "requires running Ollama"]
    async fn ollama_large_batch_exceeding_batch_size() {
        // BATCH_SIZE is 8; send 20 texts to exercise the chunking logic
        let embedder = make_embedder();
        let texts: Vec<String> = (0..20)
            .map(|i| format!("fn function_{i}() -> i32 {{ {i} }}"))
            .collect();
        let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let results = embedder.embed(&refs).await.expect("large batch failed");
        assert_eq!(results.len(), 20);
        let dims = results[0].len();
        assert!(
            results.iter().all(|v| v.len() == dims),
            "all vectors same dims"
        );
    }

    #[tokio::test]
    async fn probe_ollama_errors_when_unreachable() {
        // Port 1 is a reserved system port that is never listening in practice,
        // so the connection is refused immediately without waiting for the timeout.
        let result = super::probe_ollama("http://127.0.0.1:1").await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not reachable"),
            "error message should mention 'not reachable'"
        );
    }
}
