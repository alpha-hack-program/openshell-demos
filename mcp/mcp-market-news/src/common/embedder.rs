//! Pure-Rust sentence embeddings via `candle-transformers`, no ONNX runtime
//! and no external embedding service — everything runs in-process on CPU.
//!
//! Model: `sentence-transformers/all-MiniLM-L6-v2` (384 dims). Weights are
//! fetched once via `hf-hub` and cached under the standard Hugging Face
//! cache directory (`~/.cache/huggingface` by default, or `$HF_HOME`) so
//! subsequent process starts don't re-download anything.

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use tokenizers::Tokenizer;

/// Hugging Face Hub repo id for the embedding model.
pub const MODEL_ID: &str = "sentence-transformers/all-MiniLM-L6-v2";

/// Output dimensionality of `MODEL_ID` — also the `dim` the TurboVec index
/// must be constructed with.
pub const EMBEDDING_DIM: usize = 384;

pub struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl Embedder {
    /// Downloads (or reuses the local HF cache for) the model weights,
    /// config and tokenizer, and loads them onto CPU. Call once at process
    /// start and reuse the resulting `Embedder` for every text — reloading
    /// per-call would repeat the (relatively expensive) weight load.
    pub fn load() -> anyhow::Result<Self> {
        let device = Device::Cpu;

        // NOTE: `hf_hub::api::sync::Api::new()` calls `ApiBuilder::new()`,
        // which builds its cache from `Cache::default()` — i.e. always
        // `dirs::home_dir()/.cache/huggingface`, completely ignoring
        // `HF_HOME`. This was confirmed the hard way: it "worked" in a
        // plain `cargo run` because `$HOME` happened to already have the
        // model cached from an earlier run, and only failed loudly once
        // this ran as a non-root container user whose `$HOME`
        // (`/home/mcpserver`) doesn't exist and isn't writable — the
        // `HF_HOME` pointing at the mounted PVC was silently never used.
        // `ApiBuilder::from_env()` is the one that actually reads
        // `HF_HOME` (see `Cache::from_env()` in the hf-hub 0.4.3 source).
        let api = hf_hub::api::sync::ApiBuilder::from_env().build()?;
        let repo = api.model(MODEL_ID.to_string());

        let config_path = repo.get("config.json")?;
        let config: BertConfig = serde_json::from_str(&std::fs::read_to_string(config_path)?)?;

        let tokenizer_path = repo.get("tokenizer.json")?;
        let tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let weights_path = repo.get("model.safetensors")?;
        // Safety: we trust the file we just fetched from the Hub cache; this
        // is the standard candle pattern for loading safetensors weights.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, &device)? };
        let model = BertModel::load(vb, &config)?;

        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    /// Embeds `text` and returns an L2-normalized vector of length
    /// [`EMBEDDING_DIM`], ready for cosine similarity (a dot product of two
    /// normalized vectors *is* the cosine similarity) or direct use with
    /// TurboVec.
    pub fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let ids = Tensor::new(encoding.get_ids(), &self.device)?.unsqueeze(0)?;
        let mask = Tensor::new(encoding.get_attention_mask(), &self.device)?.unsqueeze(0)?;
        let token_type_ids = ids.zeros_like()?;

        // hidden: (batch=1, n_tokens, hidden_size)
        let hidden = self
            .model
            .forward(&ids, &token_type_ids, Some(&mask))?
            .to_dtype(DType::F32)?;
        let (_n, n_tokens, _dim) = hidden.dims3()?;

        // Mean pooling over the token dimension.
        let pooled = (hidden.sum(1)? / n_tokens as f64)?;
        let norm = pooled.sqr()?.sum_keepdim(1)?.sqrt()?;
        let normalized = pooled.broadcast_div(&norm)?.squeeze(0)?;

        Ok(normalized.to_vec1::<f32>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    /// Downloads the real model from Hugging Face Hub and runs two real
    /// forward passes. Network-gated and slow (first run downloads ~90MB),
    /// so it is `#[ignore]`d by default:
    ///
    ///   cargo test --release -p mcp-market-news embedder:: -- --ignored --nocapture
    #[test]
    #[ignore]
    fn similar_sentences_score_higher_than_unrelated_ones() {
        let embedder = Embedder::load().expect("model should load from HF hub cache");

        let a = embedder
            .embed("NDFR shares fall after port tariff dispute disrupts logistics network")
            .unwrap();
        let b = embedder
            .embed("Logistics sector hit by new tariff rules affecting freight carriers")
            .unwrap();
        let c = embedder
            .embed("Local bakery wins award for best sourdough bread in the region")
            .unwrap();

        assert_eq!(a.len(), EMBEDDING_DIM);

        let sim_related = cosine(&a, &b);
        let sim_unrelated = cosine(&a, &c);
        assert!(
            sim_related > sim_unrelated,
            "expected related sentences ({sim_related}) to score above unrelated ones ({sim_unrelated})"
        );
    }
}
