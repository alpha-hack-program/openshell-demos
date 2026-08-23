//! Builds a small local test fixture (`data/news.jsonl` + `data/news.tv`)
//! using the *real* Embedder (downloads `sentence-transformers/all-MiniLM-L6-v2`
//! from the Hugging Face Hub on first run) and the *real* TurboVec index —
//! the same code paths `news_generator` uses — without requiring a live
//! Postgres database or `ANTHROPIC_API_KEY`.
//!
//! This exists because `news_generator`'s own corpus (Postgres +
//! Anthropic API) could not be exercised in the environment this project
//! was built in. The headline/body text below is hand-authored to mirror
//! the shape `news_generator`'s prompts ask the LLM for (see README.md):
//! a handful of background-noise items, a seeded exact-ticker hit for
//! `NDFR`, and a seeded ticker-agnostic "logistics sector" item that only
//! stage-2 semantic search should find.
//!
//! Network-gated and slow (downloads ~90MB on first run), so it's
//! `#[ignore]`d by default:
//!
//!   cargo test --release --test fixture -- --ignored --nocapture

use chrono::Utc;
use mcp_market_news::common::embedder::{Embedder, EMBEDDING_DIM};
use mcp_market_news::common::news_service::{NewsItem, INDEX_BIT_WIDTH};
use turbovec::TurboQuantIndex;

fn item(id: &str, headline: &str, body: &str, ticker: Option<&str>, sector: &str) -> NewsItem {
    NewsItem {
        id: id.to_string(),
        headline: headline.to_string(),
        body: body.to_string(),
        ticker: ticker.map(String::from),
        sector: sector.to_string(),
        sentiment: "neutral".to_string(),
        published_at: Utc::now(),
    }
}

#[test]
#[ignore]
fn build_local_fixture_corpus() {
    let items = vec![
        // --- background noise (a handful, not the full 35) ---
        item(
            "noise-1",
            "Regional bank posts steady quarterly earnings in line with forecasts",
            "The bank's net interest margin held flat quarter over quarter. Analysts described the results as unremarkable.",
            Some("RBNK"),
            "banking",
        ),
        item(
            "noise-2",
            "Semiconductor maker announces minor supply chain adjustment",
            "The company shifted a portion of its wafer orders between two existing suppliers. No impact to full-year guidance is expected.",
            Some("CHIP"),
            "technology",
        ),
        item(
            "noise-3",
            "Retail chain opens three new stores in suburban markets",
            "The openings are part of a previously announced expansion plan. Same-store sales trends were not affected.",
            Some("SHOP"),
            "retail",
        ),
        item(
            "noise-4",
            "Utility company completes routine maintenance at regional plant",
            "The maintenance window was scheduled months in advance. Power supply was not interrupted.",
            None,
            "utilities",
        ),
        // --- seeded item 1: guaranteed exact-ticker hit ---
        item(
            "seed-ndfr",
            "NDFR shares slide after surprise tariff hike disrupts cross-border freight",
            "Logistics operator NDFR said a newly imposed tariff on cross-border freight will materially raise costs on its busiest trade lane. \
             The company is reviewing contingency routing and warned of a potential hit to next quarter's margins.",
            Some("NDFR"),
            "logistics",
        ),
        // --- seeded item 2: ticker-agnostic, semantic-only hit ---
        item(
            "seed-logistics-generic",
            "Port authorities tighten customs inspection rules for freight carriers",
            "A new regulatory framework requires longer inspection windows for freight moving through several major ports. \
             Industry groups expect the change to add delays across the logistics sector broadly, not any single carrier.",
            None,
            "logistics",
        ),
    ];

    let embedder = Embedder::load().expect("embedder should load from HF hub cache");

    let mut index = TurboQuantIndex::new(EMBEDDING_DIM, INDEX_BIT_WIDTH)
        .expect("index construction should succeed for a valid dim/bit_width");

    for it in &items {
        let vector = embedder
            .embed(&it.embedding_text())
            .expect("embedding should succeed");
        index.add(&vector);
    }

    std::fs::create_dir_all("data").expect("data dir should be creatable");

    let mut jsonl = String::new();
    for it in &items {
        jsonl.push_str(&serde_json::to_string(it).unwrap());
        jsonl.push('\n');
    }
    std::fs::write("data/news.jsonl", jsonl).expect("news.jsonl should be writable");

    index
        .write("data/news.tv")
        .expect("news.tv should be writable");

    eprintln!(
        "Wrote {} fixture items to data/news.jsonl and data/news.tv",
        items.len()
    );
}
