//! `news_generator` — batch job that (re)builds the market news corpus.
//!
//! Run this separately, before each demo session (or as a Kubernetes
//! `CronJob` against the same PVC the `mcp_server` binary reads from) — it
//! is NOT started by the interactive service.
//!
//! Pipeline:
//! 1. Read distinct `(ticker, sector)` pairs from the shared Postgres
//!    `positions` table (`DATABASE_URL`).
//! 2. Ask Claude (`ANTHROPIC_API_KEY`, model `claude-sonnet-4-6`) for a
//!    batch of background-noise headlines covering those tickers/sectors,
//!    plus two hand-guided "seeded" items that guarantee the demo always
//!    has something to find (see the prompts below, reproduced verbatim in
//!    the README).
//! 3. Embed every item (`headline + body`) with the pure-Rust
//!    [`mcp_market_news::common::embedder::Embedder`] and add the vectors
//!    to a `turbovec::TurboQuantIndex`.
//! 4. Write `data/news.jsonl` (source of truth) and `data/news.tv`
//!    (persisted vector index) — both consumed read-only by `mcp_server`.

use chrono::Utc;
use mcp_market_news::common::embedder::{Embedder, EMBEDDING_DIM};
use mcp_market_news::common::news_service::{NewsItem, INDEX_BIT_WIDTH};
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use turbovec::TurboQuantIndex;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const ANTHROPIC_VERSION: &str = "2023-06-01";

const DEFAULT_JSONL_PATH: &str = "data/news.jsonl";
const DEFAULT_TV_PATH: &str = "data/news.tv";

/// Shape of one item as returned by the LLM, before we assign an `id` and
/// a real generation timestamp.
#[derive(Debug, Deserialize)]
struct GeneratedNewsItem {
    headline: String,
    body: String,
    #[serde(default)]
    ticker: Option<String>,
    sector: String,
    sentiment: String,
}

/// Batch 1 prompt — background noise. `{tickers_and_sectors}` is filled in
/// from `SELECT DISTINCT ticker, sector FROM positions` against the shared
/// portfolio database. Reproduced verbatim in README.md.
fn batch1_prompt(tickers_and_sectors: &str) -> String {
    format!(
        "Generate 35 short fictional financial news headlines for these tickers/sectors: \
         {tickers_and_sectors}. Each item: `headline` (one sentence), `body` (2-3 sentences), \
         `ticker` (can be null if sector-level), `sector`, `sentiment` (positive/negative/neutral). \
         Most should be normal, low-impact market noise, not extraordinary events. Return only \
         a JSON array, no extra text."
    )
}

/// Seeded item #1 — guaranteed exact-ticker hit. Reproduced verbatim in
/// README.md.
const SEED_PROMPT_1: &str = "Generate 1 fictional financial news item explicitly mentioning ticker `NDFR` (logistics sector) describing a clear high-impact event (tariff change, operational incident, etc). Same format as above.";

/// Seeded item #2 — guaranteed semantic-only hit (no ticker mentioned).
/// Reproduced verbatim in README.md.
const SEED_PROMPT_2: &str = "Generate 1 fictional financial news item that does NOT mention any ticker by name, but describes an event generically affecting the \"logistics\" sector (e.g. a port regulatory change). This item tests semantic filtering, not exact ticker matching.";

/// Reads distinct `(ticker, sector)` pairs from the shared portfolio
/// database and renders them as a comma-separated list for the batch-1
/// prompt.
async fn load_tickers_and_sectors(database_url: &str) -> anyhow::Result<String> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await?;

    #[derive(sqlx::FromRow)]
    struct TickerSector {
        ticker: Option<String>,
        sector: Option<String>,
    }

    let rows: Vec<TickerSector> = sqlx::query_as("SELECT DISTINCT ticker, sector FROM positions")
        .fetch_all(&pool)
        .await?;

    let rendered: Vec<String> = rows
        .into_iter()
        .filter_map(|r| match (r.ticker, r.sector) {
            (Some(t), Some(s)) => Some(format!("{t} ({s})")),
            (Some(t), None) => Some(t),
            (None, Some(s)) => Some(s),
            (None, None) => None,
        })
        .collect();

    Ok(rendered.join(", "))
}

/// Calls the Anthropic Messages API with a single user-turn prompt and
/// returns the raw text of the first content block.
async fn call_anthropic(
    client: &reqwest::Client,
    api_key: &str,
    prompt: &str,
) -> anyhow::Result<String> {
    let body = serde_json::json!({
        "model": ANTHROPIC_MODEL,
        "max_tokens": 16000,
        "messages": [
            { "role": "user", "content": prompt }
        ]
    });

    let resp = client
        .post(ANTHROPIC_API_URL)
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let payload: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("Anthropic API error ({status}): {payload}");
    }

    payload["content"][0]["text"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("unexpected Anthropic response shape: {payload}"))
}

/// The model is asked to "return only a JSON array, no extra text", but we
/// parse defensively: extract the first top-level JSON array or object out
/// of whatever text came back, rather than trusting it verbatim.
fn parse_generated_items(text: &str) -> anyhow::Result<Vec<GeneratedNewsItem>> {
    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        if end > start {
            let candidate = &text[start..=end];
            if let Ok(items) = serde_json::from_str::<Vec<GeneratedNewsItem>>(candidate) {
                return Ok(items);
            }
        }
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if end > start {
            let candidate = &text[start..=end];
            if let Ok(item) = serde_json::from_str::<GeneratedNewsItem>(candidate) {
                return Ok(vec![item]);
            }
        }
    }
    anyhow::bail!("could not find a JSON array or object in LLM response: {text}")
}

fn to_news_items(generated: Vec<GeneratedNewsItem>) -> Vec<NewsItem> {
    let now = Utc::now();
    generated
        .into_iter()
        .map(|g| NewsItem {
            id: uuid::Uuid::new_v4().to_string(),
            headline: g.headline,
            body: g.body,
            ticker: g.ticker,
            sector: g.sector,
            sentiment: g.sentiment,
            published_at: now,
        })
        .collect()
}

fn write_jsonl(items: &[NewsItem], path: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    for item in items {
        writeln!(file, "{}", serde_json::to_string(item)?)?;
    }
    Ok(())
}

fn build_and_write_index(
    embedder: &Embedder,
    items: &[NewsItem],
    path: &str,
) -> anyhow::Result<()> {
    let mut index = TurboQuantIndex::new(EMBEDDING_DIM, INDEX_BIT_WIDTH)
        .map_err(|e| anyhow::anyhow!("failed to construct TurboVec index: {e:?}"))?;

    for item in items {
        let vector = embedder.embed(&item.embedding_text())?;
        index.add(&vector);
    }

    index
        .write(path)
        .map_err(|e| anyhow::anyhow!("failed to write TurboVec index to {path}: {e}"))?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL is not set — cannot read positions table"))?;
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY is not set — cannot call Claude"))?;

    let jsonl_path =
        std::env::var("NEWS_JSONL_PATH").unwrap_or_else(|_| DEFAULT_JSONL_PATH.to_string());
    let tv_path = std::env::var("NEWS_TV_PATH").unwrap_or_else(|_| DEFAULT_TV_PATH.to_string());

    tracing::info!("Reading distinct ticker/sector pairs from positions table");
    let tickers_and_sectors = load_tickers_and_sectors(&database_url).await?;
    tracing::info!(tickers_and_sectors, "Loaded tickers/sectors");

    let client = reqwest::Client::new();

    tracing::info!("Requesting batch 1 (background noise) from Claude");
    let batch1_text =
        call_anthropic(&client, &api_key, &batch1_prompt(&tickers_and_sectors)).await?;
    let batch1 = to_news_items(parse_generated_items(&batch1_text)?);
    tracing::info!(count = batch1.len(), "Batch 1 generated");

    tracing::info!("Requesting seeded item 1 (NDFR exact-ticker hit) from Claude");
    let seed1_text = call_anthropic(&client, &api_key, SEED_PROMPT_1).await?;
    let seed1 = to_news_items(parse_generated_items(&seed1_text)?);

    tracing::info!("Requesting seeded item 2 (generic logistics, semantic-only hit) from Claude");
    let seed2_text = call_anthropic(&client, &api_key, SEED_PROMPT_2).await?;
    let seed2 = to_news_items(parse_generated_items(&seed2_text)?);

    let mut all_items = batch1;
    all_items.extend(seed1);
    all_items.extend(seed2);
    tracing::info!(total = all_items.len(), "All items generated");

    write_jsonl(&all_items, &jsonl_path)?;
    tracing::info!(path = jsonl_path, "Wrote news.jsonl");

    tracing::info!("Loading embedder (downloads model weights on first run)");
    let embedder = Embedder::load()?;

    tracing::info!("Embedding items and building TurboVec index");
    build_and_write_index(&embedder, &all_items, &tv_path)?;
    tracing::info!(path = tv_path, "Wrote news.tv");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json_array() {
        let text = r#"[{"headline":"h","body":"b","ticker":"NDFR","sector":"logistics","sentiment":"neutral"}]"#;
        let items = parse_generated_items(text).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].ticker.as_deref(), Some("NDFR"));
    }

    #[test]
    fn parses_json_array_with_surrounding_prose() {
        let text = "Sure, here you go:\n```json\n[{\"headline\":\"h\",\"body\":\"b\",\"sector\":\"logistics\",\"sentiment\":\"neutral\"}]\n```\nHope that helps!";
        let items = parse_generated_items(text).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].ticker, None);
    }

    #[test]
    fn parses_single_object_for_seed_prompts() {
        let text = r#"{"headline":"h","body":"b","ticker":"NDFR","sector":"logistics","sentiment":"negative"}"#;
        let items = parse_generated_items(text).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn to_news_items_assigns_id_and_timestamp() {
        let generated = vec![GeneratedNewsItem {
            headline: "h".to_string(),
            body: "b".to_string(),
            ticker: None,
            sector: "logistics".to_string(),
            sentiment: "neutral".to_string(),
        }];
        let items = to_news_items(generated);
        assert_eq!(items.len(), 1);
        assert!(!items[0].id.is_empty());
    }
}
