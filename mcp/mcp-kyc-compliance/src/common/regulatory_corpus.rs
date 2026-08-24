//! RAG search over the small, fictional regulatory corpus in `data/corpus/`.
//!
//! ⚠️ See the README's disclaimer: this corpus is a simplified, fictional
//! set of rules written for this demo. It does not reproduce actual text
//! from FATF, MiFID II, or any AML directive.
//!
//! Embeddings are computed via the shared, in-namespace vLLM/KServe
//! embeddings service (`src/common/embedder.rs`, duplicated from
//! `mcp-market-news` — see that project's README for the embedder/TurboVec
//! rationale). Unlike
//! `mcp-market-news`, this corpus is static (four hand-authored markdown
//! files baked into the image, not LLM-generated at runtime), so there is
//! no periodic reload: the index is built once, either ahead of time by the
//! `corpus_indexer` binary or lazily by `mcp_server` on first startup if the
//! index file doesn't exist yet ("self-healing" — no separate initContainer
//! needed, since indexing four short documents takes a fraction of a
//! second once the embedding model is loaded).
//!
//! `TurboQuantIndex` only stores vectors and hands back positional indices
//! from a search — it has no notion of the original text. This module
//! persists a small JSON sidecar (`<index_path>.docs.json`) with the
//! ordered list of `(source_file, text)` pairs, in the exact order vectors
//! were added to the index, so a search result's index can be mapped back
//! to the fragment and source document that produced it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use turbovec::TurboQuantIndex;

use super::embedder::{Embedder, EMBEDDING_DIM};

/// Bit width for the TurboVec index. Same rationale as `mcp-market-news`:
/// the corpus here is tiny (4 documents), so index size is a non-issue and
/// 4 bits/coordinate gives the best recall of the supported widths ({2,3,4}).
pub const INDEX_BIT_WIDTH: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CorpusDoc {
    source: String,
    text: String,
}

/// A single search hit: the matched text fragment, its source document
/// filename (so the caller can cite the clause), and the cosine-similarity
/// score.
#[derive(Debug, Clone, Serialize)]
pub struct RegulatoryMatch {
    pub source: String,
    pub text: String,
    pub score: f32,
}

pub struct RegulatoryCorpus {
    docs: Vec<CorpusDoc>,
    index: TurboQuantIndex,
    embedder: Embedder,
}

fn sidecar_path(index_path: &Path) -> PathBuf {
    let mut path = index_path.as_os_str().to_owned();
    path.push(".docs.json");
    PathBuf::from(path)
}

/// Reads every `*.md` file directly under `corpus_dir`, sorted by filename
/// so a rebuild is deterministic. Each whole file is treated as a single
/// fragment — the corpus documents here are short enough (a few sentences)
/// that finer-grained chunking isn't needed.
fn read_corpus_dir(corpus_dir: &Path) -> anyhow::Result<Vec<CorpusDoc>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(corpus_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    entries.sort();

    if entries.is_empty() {
        anyhow::bail!("no .md files found in corpus dir {}", corpus_dir.display());
    }

    entries
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)?;
            let source = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(CorpusDoc { source, text })
        })
        .collect()
}

impl RegulatoryCorpus {
    /// Loads the persisted index + doc sidecar from `index_path` if both
    /// exist, otherwise builds them fresh from `corpus_dir` and writes them
    /// out. Always constructs its own `Embedder` (cheap — no model to load,
    /// just an HTTP client) — call this once at process start (there is no
    /// periodic reload for this static corpus).
    pub async fn load_or_build(
        corpus_dir: impl AsRef<Path>,
        index_path: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let corpus_dir = corpus_dir.as_ref();
        let index_path = index_path.as_ref();
        let embedder = Embedder::new()?;

        let sidecar = sidecar_path(index_path);
        if index_path.exists() && sidecar.exists() {
            let index = TurboQuantIndex::load(index_path)
                .map_err(|e| anyhow::anyhow!("failed to load TurboVec index: {e}"))?;
            let docs: Vec<CorpusDoc> = serde_json::from_str(&std::fs::read_to_string(&sidecar)?)?;
            if index.len() != docs.len() {
                anyhow::bail!(
                    "corpus index ({} vectors) and doc sidecar ({} docs) are out of sync at {}",
                    index.len(),
                    docs.len(),
                    index_path.display()
                );
            }
            return Ok(Self {
                docs,
                index,
                embedder,
            });
        }

        Self::build(corpus_dir, index_path, embedder).await
    }

    /// Builds the index from scratch and persists it to `index_path` (plus
    /// its doc sidecar), overwriting whatever was there before. Used both
    /// by `load_or_build`'s cold-start path and by the standalone
    /// `corpus_indexer` binary for an explicit rebuild.
    pub async fn build(
        corpus_dir: impl AsRef<Path>,
        index_path: impl AsRef<Path>,
        embedder: Embedder,
    ) -> anyhow::Result<Self> {
        let docs = read_corpus_dir(corpus_dir.as_ref())?;

        let mut index = TurboQuantIndex::new(EMBEDDING_DIM, INDEX_BIT_WIDTH)
            .map_err(|e| anyhow::anyhow!("failed to construct TurboVec index: {e}"))?;
        for doc in &docs {
            let vector = embedder.embed(&doc.text).await?;
            index.add(&vector);
        }

        let index_path = index_path.as_ref();
        if let Some(parent) = index_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        index
            .write(index_path)
            .map_err(|e| anyhow::anyhow!("failed to write TurboVec index: {e}"))?;
        std::fs::write(sidecar_path(index_path), serde_json::to_string(&docs)?)?;

        Ok(Self {
            docs,
            index,
            embedder,
        })
    }

    /// Number of documents currently indexed.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Embeds `query` and returns the `top_k` closest documents, each with
    /// its source filename and cosine-similarity score.
    pub async fn search(&self, query: &str, top_k: usize) -> anyhow::Result<Vec<RegulatoryMatch>> {
        let query_vec = self.embedder.embed(query).await?;
        debug_assert_eq!(query_vec.len(), EMBEDDING_DIM);

        let results = self.index.search(&query_vec, top_k);
        let scores = results.scores_for_query(0);
        let indices = results.indices_for_query(0);

        Ok(scores
            .iter()
            .zip(indices.iter())
            .filter_map(|(score, idx)| {
                self.docs.get(*idx as usize).map(|doc| RegulatoryMatch {
                    source: doc.source.clone(),
                    text: doc.text.clone(),
                    score: *score,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_corpus_dir_sorts_by_filename_and_rejects_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("02-b.md"), "second").unwrap();
        std::fs::write(dir.path().join("01-a.md"), "first").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored, not markdown").unwrap();

        let docs = read_corpus_dir(dir.path()).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].source, "01-a.md");
        assert_eq!(docs[0].text, "first");
        assert_eq!(docs[1].source, "02-b.md");

        let empty_dir = tempfile::tempdir().unwrap();
        assert!(read_corpus_dir(empty_dir.path()).is_err());
    }

    #[test]
    fn sidecar_path_appends_suffix_without_losing_extension() {
        let path = sidecar_path(Path::new("data/corpus.tv"));
        assert_eq!(path, PathBuf::from("data/corpus.tv.docs.json"));
    }
}
