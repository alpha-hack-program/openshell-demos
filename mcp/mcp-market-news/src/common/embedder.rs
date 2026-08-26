//! HTTP client for the shared, in-namespace vLLM/KServe embeddings service
//! (`jinaai/jina-embeddings-v3` on CPU, OpenAI-compatible `/v1/embeddings`
//! API), replacing the previous in-process `candle-transformers`/`hf-hub`
//! implementation. Every MCP server in this demo family that needs semantic
//! search (this one and `mcp-kyc-compliance`) calls the same service instead
//! of loading its own copy of an embedding model. See the retired sections
//! of this project's README ("What was verified") for the candle/hf-hub
//! implementation this replaced and the bugs found while building it.
//!
//! This file is duplicated verbatim-in-spirit in `mcp-kyc-compliance`'s
//! `src/common/embedder.rs`, deliberately — same convention this demo family
//! already uses for `auth.rs` (copied per service rather than factored into
//! a shared crate), since the surviving code here is thin enough (an HTTP
//! POST + JSON parse) that duplication beats a shared crate's build/
//! versioning overhead.

use serde::Deserialize;

/// Output dimensionality of `jinaai/jina-embeddings-v3` — also the `dim` the
/// TurboVec index must be constructed with.
pub const EMBEDDING_DIM: usize = 1024;

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// Thin HTTP client — cheap to construct (just reads two env vars and builds
/// a `reqwest::Client`), unlike the retired candle-based `Embedder` which
/// had to download and load a model. There is nothing to "avoid reloading"
/// anymore, so callers can construct a fresh one whenever convenient (e.g.
/// on every periodic corpus reload) instead of threading a long-lived
/// instance through.
#[derive(Clone)]
pub struct Embedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl Embedder {
    /// Reads `EMBEDDINGS_BASE_URL` (no trailing `/v1`, same convention as
    /// this project's own `OPENAI_BASE_URL`) and `EMBEDDINGS_MODEL` (the
    /// served-model name to send in each request body) from the
    /// environment.
    pub fn new() -> anyhow::Result<Self> {
        let base_url = std::env::var("EMBEDDINGS_BASE_URL")
            .map_err(|_| anyhow::anyhow!("EMBEDDINGS_BASE_URL is not set"))?;
        let model = std::env::var("EMBEDDINGS_MODEL")
            .map_err(|_| anyhow::anyhow!("EMBEDDINGS_MODEL is not set"))?;
        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
            model,
        })
    }

    /// Embeds `text` via the shared embeddings service and returns an
    /// L2-normalized vector of length [`EMBEDDING_DIM`], ready for cosine
    /// similarity (a dot product of two normalized vectors *is* the cosine
    /// similarity) or direct use with TurboVec. Normalizes defensively
    /// client-side rather than assuming the service already returns unit
    /// vectors — same as the retired candle-based implementation did.
    pub async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let body = serde_json::json!({ "model": self.model, "input": text });

        let resp = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("embeddings service error ({status}): {text}");
        }

        let parsed: EmbeddingsResponse = resp.json().await?;
        let vector = parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| anyhow::anyhow!("embeddings service returned no data"))?;

        Ok(normalize(vector))
    }
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_produces_a_unit_vector() {
        let v = normalize(vec![3.0, 4.0]);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_leaves_the_zero_vector_unchanged() {
        assert_eq!(normalize(vec![0.0, 0.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn new_fails_fast_when_env_vars_are_missing() {
        // SAFETY: test-only env mutation; this crate's tests run
        // single-threaded within this module (no other test touches these
        // two vars), so there's no cross-test race.
        unsafe {
            std::env::remove_var("EMBEDDINGS_BASE_URL");
            std::env::remove_var("EMBEDDINGS_MODEL");
        }
        assert!(Embedder::new().is_err());
    }
}
