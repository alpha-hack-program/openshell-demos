//! Standalone batch tool: (re)builds the regulatory corpus' embedding index
//! from `data/corpus/*.md`, overwriting whatever index was there before.
//!
//! `mcp_server` builds this index itself on first startup if it's missing
//! (see `common::regulatory_corpus::RegulatoryCorpus::load_or_build`), so
//! running this binary is optional — it exists for manually regenerating
//! the index in CI or local dev (e.g. after editing a corpus document)
//! without starting the whole server.

use mcp_kyc_compliance::common::embedder::Embedder;
use mcp_kyc_compliance::common::regulatory_corpus::RegulatoryCorpus;

const DEFAULT_CORPUS_DIR: &str = "data/corpus";
const DEFAULT_CORPUS_INDEX_PATH: &str = "data/corpus.tv";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".to_string().into()),
        )
        .init();

    let corpus_dir = std::env::var("CORPUS_DIR").unwrap_or_else(|_| DEFAULT_CORPUS_DIR.to_string());
    let corpus_index_path = std::env::var("CORPUS_INDEX_PATH")
        .unwrap_or_else(|_| DEFAULT_CORPUS_INDEX_PATH.to_string());

    tracing::info!(
        corpus_dir,
        corpus_index_path,
        "building regulatory corpus index"
    );
    let embedder = Embedder::new()?;
    let corpus = RegulatoryCorpus::build(&corpus_dir, &corpus_index_path, embedder).await?;
    tracing::info!(
        documents = corpus.len(),
        "wrote {} and its doc sidecar",
        corpus_index_path
    );
    Ok(())
}
