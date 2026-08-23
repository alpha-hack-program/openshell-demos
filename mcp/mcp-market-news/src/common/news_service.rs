//! `get_relevant_news` business logic: a two-stage filter over the market
//! news corpus.
//!
//! Stage 1 is a cheap, exact-match filter over ticker/sector within a
//! freshness window. Stage 2 only runs when stage 1 comes up short, and
//! uses a semantic (embedding) search over the TurboVec index so that news
//! which never mentions a ticker by name (e.g. "a new port regulation hits
//! the logistics sector") can still surface for a portfolio holding
//! logistics names.
//!
//! Unlike the sibling MCP servers in this demo (`mcp-portfolio`,
//! `mcp-kyc-compliance`, `mcp-crm-calendar`), this server does NOT apply
//! per-caller data isolation: market news is public data, not
//! client-specific data. `called_by`/`roles` are still attached to every
//! tool response for consistency with the rest of the demo family.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use turbovec::TurboQuantIndex;

use super::embedder::{Embedder, EMBEDDING_DIM};

/// How far back "relevant" news is allowed to be. Deliberately a relative
/// window (not an absolute cutoff date) so a corpus generated at any point
/// in the past still demos correctly, as long as it's regenerated within
/// this window of "now".
pub const FRESHNESS_WINDOW_HOURS: i64 = 48;

/// Bit width used for the TurboVec index. 4 bits/coordinate gives the best
/// recall of the supported widths ({2,3,4}) at a corpus size this small
/// (tens of items) where index size is a non-issue.
pub const INDEX_BIT_WIDTH: usize = 4;

/// Cosine-similarity floor for a stage-2 (semantic) hit to be considered
/// relevant enough to return.
pub const SIMILARITY_THRESHOLD: f32 = 0.6;

/// Max number of stage-2 semantic candidates considered before the
/// similarity threshold is applied.
pub const SEMANTIC_TOP_K: usize = 5;

/// Minimum number of stage-1 exact-match hits below which stage 2 kicks in.
pub const MIN_STAGE1_HITS: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewsItem {
    pub id: String,
    pub headline: String,
    pub body: String,
    /// `None` for sector-level items that don't call out a specific ticker
    /// (e.g. the generic-logistics seeded item used to exercise semantic
    /// filtering).
    #[serde(default)]
    pub ticker: Option<String>,
    pub sector: String,
    pub sentiment: String,
    pub published_at: DateTime<Utc>,
}

impl NewsItem {
    /// Text embedded for semantic search: headline carries the most
    /// signal, body adds context.
    pub fn embedding_text(&self) -> String {
        format!("{} {}", self.headline, self.body)
    }

    fn matches_ticker(&self, tickers: &[String]) -> bool {
        let Some(item_ticker) = &self.ticker else {
            return false;
        };
        tickers.iter().any(|t| t.eq_ignore_ascii_case(item_ticker))
    }

    fn matches_sector(&self, sectors: &[String]) -> bool {
        sectors.iter().any(|s| s.eq_ignore_ascii_case(&self.sector))
    }

    fn is_fresh(&self, now: DateTime<Utc>, window_hours: i64) -> bool {
        now.signed_duration_since(self.published_at) <= Duration::hours(window_hours)
    }
}

/// Holds the in-RAM corpus and the loaded vector index. Built once at
/// process start (`NewsService::load`) and reused for every tool call —
/// the hot query path never touches disk again.
pub struct NewsService {
    items: Vec<NewsItem>,
    index: TurboQuantIndex,
    embedder: Embedder,
}

impl NewsService {
    /// Loads `news.jsonl` (one JSON `NewsItem` per line) fully into RAM and
    /// the persisted TurboVec index from `news.tv`, plus the embedder
    /// (needed to embed stage-2 queries). All disk/model I/O happens here,
    /// not on the request path.
    pub fn load(
        jsonl_path: impl AsRef<std::path::Path>,
        tv_path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<Self> {
        let items = load_jsonl(jsonl_path)?;
        let index = TurboQuantIndex::load(tv_path)
            .map_err(|e| anyhow::anyhow!("failed to load TurboVec index: {e}"))?;
        let embedder = Embedder::load()?;

        if index.len() != items.len() {
            tracing::warn!(
                items = items.len(),
                index_len = index.len(),
                "news.jsonl and news.tv are out of sync — did news_generator run to completion?"
            );
        }

        Ok(Self {
            items,
            index,
            embedder,
        })
    }

    /// Construct directly from parts, for tests / the generator's own
    /// verification step (no HF download needed if an `Embedder` is
    /// already on hand).
    pub fn from_parts(items: Vec<NewsItem>, index: TurboQuantIndex, embedder: Embedder) -> Self {
        Self {
            items,
            index,
            embedder,
        }
    }

    /// Two-stage relevant-news lookup. Never returns the full feed — only
    /// the narrowed stage-1 and/or stage-2 hits, deduplicated by id.
    pub fn get_relevant_news(
        &self,
        tickers: &[String],
        sectors: &[String],
    ) -> anyhow::Result<Vec<NewsItem>> {
        let now = Utc::now();

        let stage1: Vec<&NewsItem> = self
            .items
            .iter()
            .filter(|item| item.is_fresh(now, FRESHNESS_WINDOW_HOURS))
            .filter(|item| item.matches_ticker(tickers) || item.matches_sector(sectors))
            .collect();

        let mut results: Vec<NewsItem> = stage1.iter().map(|i| (*i).clone()).collect();

        if stage1.len() < MIN_STAGE1_HITS && !sectors.is_empty() {
            // Wrapping the raw sector words in a short natural-language sentence
            // measurably improves separation from unrelated boilerplate
            // financial headlines under MiniLM-L6 (empirically ~0.08 wider
            // gap between the true match and the nearest false positive vs.
            // embedding the bare sector words) — see README "Open risks" for
            // the numbers this was tuned against.
            let query_text = format!("News affecting the {} sector", sectors.join(" and "));
            let query_vec = self.embedder.embed(&query_text)?;
            debug_assert_eq!(query_vec.len(), EMBEDDING_DIM);

            let search_results = self.index.search(&query_vec, SEMANTIC_TOP_K);
            let scores = search_results.scores_for_query(0);
            let indices = search_results.indices_for_query(0);

            for (score, idx) in scores.iter().zip(indices.iter()) {
                if *score <= SIMILARITY_THRESHOLD {
                    continue;
                }
                let Some(item) = self.items.get(*idx as usize) else {
                    continue;
                };
                if !item.is_fresh(now, FRESHNESS_WINDOW_HOURS) {
                    continue;
                }
                if !results.iter().any(|r| r.id == item.id) {
                    results.push(item.clone());
                }
            }
        }

        Ok(results)
    }

    #[cfg(test)]
    pub fn items(&self) -> &[NewsItem] {
        &self.items
    }
}

fn load_jsonl(path: impl AsRef<std::path::Path>) -> anyhow::Result<Vec<NewsItem>> {
    let content = std::fs::read_to_string(path)?;
    let mut items = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let item: NewsItem = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("news.jsonl line {}: {e}", line_no + 1))?;
        items.push(item);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, ticker: Option<&str>, sector: &str, hours_ago: i64) -> NewsItem {
        NewsItem {
            id: id.to_string(),
            headline: format!("Headline for {id}"),
            body: "Body text.".to_string(),
            ticker: ticker.map(String::from),
            sector: sector.to_string(),
            sentiment: "neutral".to_string(),
            published_at: Utc::now() - Duration::hours(hours_ago),
        }
    }

    #[test]
    fn stage1_matches_exact_ticker_within_window() {
        let i = item("1", Some("NDFR"), "logistics", 1);
        assert!(i.matches_ticker(&["NDFR".to_string()]));
        assert!(i.is_fresh(Utc::now(), FRESHNESS_WINDOW_HOURS));
    }

    #[test]
    fn stage1_ticker_match_is_case_insensitive() {
        let i = item("1", Some("ndfr"), "logistics", 1);
        assert!(i.matches_ticker(&["NDFR".to_string()]));
    }

    #[test]
    fn stage1_rejects_stale_items() {
        let i = item("1", Some("NDFR"), "logistics", 72);
        assert!(!i.is_fresh(Utc::now(), FRESHNESS_WINDOW_HOURS));
    }

    #[test]
    fn sector_only_item_never_matches_ticker() {
        let i = item("1", None, "logistics", 1);
        assert!(!i.matches_ticker(&["NDFR".to_string()]));
        assert!(i.matches_sector(&["logistics".to_string()]));
    }

    #[test]
    fn load_jsonl_parses_one_item_per_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("news.jsonl");
        let item = item("1", Some("NDFR"), "logistics", 1);
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&item).unwrap()),
        )
        .unwrap();

        let loaded = load_jsonl(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "1");
    }
}
