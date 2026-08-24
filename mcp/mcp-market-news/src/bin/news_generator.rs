//! `news_generator` — batch job that (re)builds the market news corpus.
//!
//! Run this separately, before each demo session (or as a Kubernetes
//! `CronJob` against the same PVC the `mcp_server` binary reads from) — it
//! is NOT started by the interactive service.
//!
//! Pipeline:
//! 1. Read distinct `(ticker, sector)` pairs from the shared Postgres
//!    `positions` table (`DATABASE_URL`).
//! 2. Ask an OpenAI-compatible chat-completions endpoint (`OPENAI_BASE_URL`,
//!    `OPENAI_API_KEY`, `OPENAI_MODEL`) for a batch of background-noise
//!    headlines covering those tickers/sectors, plus two hand-guided
//!    "seeded" items that guarantee the demo always has something to find
//!    (see the prompts below, reproduced verbatim in the README).
//! 3. Embed every item (`headline + body`) with the pure-Rust
//!    [`mcp_market_news::common::embedder::Embedder`] and add the vectors
//!    to a `turbovec::TurboQuantIndex`.
//! 4. Write `data/news.jsonl` (source of truth) and `data/news.tv`
//!    (persisted vector index) — both consumed read-only by `mcp_server`.

use chrono::Utc;
use mcp_market_news::common::embedder::{Embedder, EMBEDDING_DIM};
use mcp_market_news::common::news_service::{load_jsonl, NewsItem, INDEX_BIT_WIDTH};
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use turbovec::TurboQuantIndex;

const DEFAULT_GENERATION_INTERVAL_MINUTES: u64 = 5;
const DEFAULT_GENERATION_BATCH_SIZE: usize = 5;

/// Default base URL when `OPENAI_BASE_URL` isn't set — plain OpenAI. Points
/// at an OpenAI-*compatible* endpoint (e.g. a self-hosted vLLM route) by
/// overriding `OPENAI_BASE_URL`; does not include a trailing `/v1`, same
/// convention as `docs/inference-api-compatibility.md`.
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

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

/// Drip-feed prompt used by `GENERATION_MODE=loop` — same shape as
/// [`batch1_prompt`] but with a caller-supplied item count instead of a
/// fixed 35, so `GENERATION_BATCH_SIZE` controls how much "new" news shows
/// up each cycle.
fn topup_prompt(count: usize, tickers_and_sectors: &str) -> String {
    format!(
        "Generate {count} short fictional financial news headlines for these tickers/sectors: \
         {tickers_and_sectors}. Each item: `headline` (one sentence), `body` (2-3 sentences), \
         `ticker` (can be null if sector-level), `sector`, `sentiment` (positive/negative/neutral). \
         Most should be normal, low-impact market noise, not extraordinary events. Return only \
         a JSON array, no extra text."
    )
}

/// Seeded item #1 — guaranteed exact-ticker hit. Reproduced verbatim in
/// README.md.
///
/// Self-contained on purpose: each `call_openai` request is a separate,
/// stateless API call with no shared conversation history, so an earlier
/// version of this prompt saying "same format as above" referred to
/// nothing the model could actually see — observed in production (2026-08-23)
/// causing DeepSeek to return a markdown news article instead of JSON,
/// which crashed `news_generator` via `parse_generated_items`'s error path.
const SEED_PROMPT_1: &str = "Generate 1 fictional financial news item explicitly mentioning ticker `NDFR` (logistics sector) describing a clear high-impact event (tariff change, operational incident, etc). Fields: `headline` (one sentence), `body` (2-3 sentences), `ticker` (must be \"NDFR\"), `sector` (must be \"logistics\"), `sentiment` (positive/negative/neutral). Return only a single JSON object with those five fields, no extra text, no markdown formatting.";

/// Seeded item #2 — guaranteed semantic-only hit (no ticker mentioned).
/// Reproduced verbatim in README.md. Self-contained for the same reason as
/// `SEED_PROMPT_1` above.
const SEED_PROMPT_2: &str = "Generate 1 fictional financial news item that does NOT mention any ticker by name, but describes an event generically affecting the \"logistics\" sector (e.g. a port regulatory change). This item tests semantic filtering, not exact ticker matching. Fields: `headline` (one sentence), `body` (2-3 sentences), `ticker` (must be null), `sector` (must be \"logistics\"), `sentiment` (positive/negative/neutral). Return only a single JSON object with those five fields, no extra text, no markdown formatting.";

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

/// Calls an OpenAI-compatible `/v1/chat/completions` endpoint with a single
/// user-turn prompt and returns the raw text of the first choice.
///
/// Chat Completions, not the Responses API used elsewhere in this repo for
/// Codex/namespace-tool compatibility (see
/// `docs/inference-api-compatibility.md`) — this call has no tool use at
/// all, and Chat Completions is the more broadly supported format across
/// self-hosted/third-party OpenAI-compatible endpoints (older vLLM
/// included).
async fn call_openai(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> anyhow::Result<String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 16000,
        "messages": [
            { "role": "user", "content": prompt }
        ]
    });

    let resp = client
        .post(format!("{base_url}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let payload: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("OpenAI-compatible API error ({status}): {payload}");
    }

    payload["choices"][0]["message"]["content"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("unexpected chat-completions response shape: {payload}"))
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

/// Writes to `{path}.tmp` then `rename()`s over `path` — `rename` is atomic
/// on the same filesystem, so `mcp_server`'s periodic reload (see
/// `mcp_server.rs`) never observes a partially-written file. Without this,
/// a reload racing an in-progress write could read a truncated JSONL.
fn write_jsonl(items: &[NewsItem], path: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let tmp_path = format!("{path}.tmp");
    {
        let mut file = std::fs::File::create(&tmp_path)?;
        for item in items {
            writeln!(file, "{}", serde_json::to_string(item)?)?;
        }
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

async fn build_and_write_index(
    embedder: &Embedder,
    items: &[NewsItem],
    path: &str,
) -> anyhow::Result<()> {
    let mut index = TurboQuantIndex::new(EMBEDDING_DIM, INDEX_BIT_WIDTH)
        .map_err(|e| anyhow::anyhow!("failed to construct TurboVec index: {e:?}"))?;

    for item in items {
        let vector = embedder.embed(&item.embedding_text()).await?;
        index.add(&vector);
    }

    // Same atomic write-then-rename as `write_jsonl` above, same reason.
    let tmp_path = format!("{path}.tmp");
    index
        .write(&tmp_path)
        .map_err(|e| anyhow::anyhow!("failed to write TurboVec index to {tmp_path}: {e}"))?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// `GENERATION_MODE=loop`: an alternative to running this binary as a
/// one-shot Kubernetes `CronJob` (see module docs) — instead, run it as a
/// long-lived process that generates a fresh drip-feed batch immediately on
/// startup, then again every `interval`, forever, so a demo session sees
/// "new" news arrive over time without anyone re-running the generator by
/// hand. The full corpus (existing + drip-fed items) is always rewritten to
/// `jsonl_path`/`tv_path` in place, using the exact same
/// `write_jsonl`/`build_and_write_index` path as the one-shot mode, so
/// `mcp_server` doesn't need to know which mode produced its corpus.
///
/// A failed cycle (LLM call, parsing, or write error) is logged and
/// skipped rather than crashing the process — a transient failure
/// shouldn't take down a service meant to run unattended for the length of
/// a demo session.
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    tickers_and_sectors: &str,
    jsonl_path: &str,
    tv_path: &str,
    interval: Duration,
    batch_size: usize,
) -> anyhow::Result<()> {
    let embedder = Embedder::new()?;
    let mut corpus = if std::path::Path::new(jsonl_path).exists() {
        load_jsonl(jsonl_path)?
    } else {
        Vec::new()
    };
    tracing::info!(
        existing = corpus.len(),
        interval_secs = interval.as_secs(),
        batch_size,
        "Starting continuous news generation loop"
    );

    loop {
        tracing::info!(batch_size, "Generating drip-feed batch");
        let cycle_result = call_openai(
            client,
            base_url,
            api_key,
            model,
            &topup_prompt(batch_size, tickers_and_sectors),
        )
        .await
        .and_then(|text| parse_generated_items(&text));

        match cycle_result {
            Ok(generated) => {
                let new_items = to_news_items(generated);
                tracing::info!(count = new_items.len(), "Generated new items");
                corpus.extend(new_items);

                if let Err(e) = write_jsonl(&corpus, jsonl_path) {
                    tracing::error!(error = %e, "Failed to write news.jsonl this cycle");
                } else if let Err(e) = build_and_write_index(&embedder, &corpus, tv_path).await {
                    tracing::error!(error = %e, "Failed to rebuild TurboVec index this cycle");
                } else {
                    tracing::info!(total = corpus.len(), path = jsonl_path, "Corpus updated");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Generation cycle failed, will retry next interval")
            }
        }

        tracing::info!(
            seconds = interval.as_secs(),
            "Sleeping until next generation cycle"
        );
        tokio::time::sleep(interval).await;
    }
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
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY is not set — cannot call the LLM endpoint"))?;
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_OPENAI_BASE_URL.to_string());
    let model = std::env::var("OPENAI_MODEL")
        .map_err(|_| anyhow::anyhow!("OPENAI_MODEL is not set — cannot call the LLM endpoint"))?;

    let jsonl_path =
        std::env::var("NEWS_JSONL_PATH").unwrap_or_else(|_| DEFAULT_JSONL_PATH.to_string());
    let tv_path = std::env::var("NEWS_TV_PATH").unwrap_or_else(|_| DEFAULT_TV_PATH.to_string());

    tracing::info!("Reading distinct ticker/sector pairs from positions table");
    let tickers_and_sectors = load_tickers_and_sectors(&database_url).await?;
    tracing::info!(tickers_and_sectors, "Loaded tickers/sectors");

    let client = reqwest::Client::new();

    let generation_mode = std::env::var("GENERATION_MODE").unwrap_or_else(|_| "once".to_string());
    if generation_mode == "loop" {
        let interval_minutes: u64 = std::env::var("GENERATION_INTERVAL_MINUTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_GENERATION_INTERVAL_MINUTES);
        let batch_size: usize = std::env::var("GENERATION_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_GENERATION_BATCH_SIZE);

        tracing::info!(
            interval_minutes,
            batch_size,
            "GENERATION_MODE=loop — running as a continuous drip-feed service"
        );
        return run_loop(
            &client,
            &base_url,
            &api_key,
            &model,
            &tickers_and_sectors,
            &jsonl_path,
            &tv_path,
            Duration::from_secs(interval_minutes * 60),
            batch_size,
        )
        .await;
    }

    tracing::info!(model, base_url, "Requesting batch 1 (background noise)");
    let batch1_text = call_openai(
        &client,
        &base_url,
        &api_key,
        &model,
        &batch1_prompt(&tickers_and_sectors),
    )
    .await?;
    let batch1 = to_news_items(parse_generated_items(&batch1_text)?);
    tracing::info!(count = batch1.len(), "Batch 1 generated");

    tracing::info!("Requesting seeded item 1 (NDFR exact-ticker hit)");
    let seed1_text = call_openai(&client, &base_url, &api_key, &model, SEED_PROMPT_1).await?;
    let seed1 = to_news_items(parse_generated_items(&seed1_text)?);

    tracing::info!("Requesting seeded item 2 (generic logistics, semantic-only hit)");
    let seed2_text = call_openai(&client, &base_url, &api_key, &model, SEED_PROMPT_2).await?;
    let seed2 = to_news_items(parse_generated_items(&seed2_text)?);

    let mut all_items = batch1;
    all_items.extend(seed1);
    all_items.extend(seed2);
    tracing::info!(total = all_items.len(), "All items generated");

    write_jsonl(&all_items, &jsonl_path)?;
    tracing::info!(path = jsonl_path, "Wrote news.jsonl");

    tracing::info!("Connecting to the shared embeddings service");
    let embedder = Embedder::new()?;

    tracing::info!("Embedding items and building TurboVec index");
    build_and_write_index(&embedder, &all_items, &tv_path).await?;
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
