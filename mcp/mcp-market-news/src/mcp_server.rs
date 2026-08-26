//! `mcp-market-news` — MCP server exposing `get_relevant_news`.
//!
//! On startup this loads the whole news corpus (`data/news.jsonl`) into RAM
//! and the persisted TurboVec embedding index (`data/news.tv`) once; the
//! hot query path never touches disk again. Both files are produced ahead
//! of time by the separate `news_generator` batch binary (run it first —
//! see the README) — in Kubernetes, `data/` is meant to be a PVC shared
//! between this service and a `news_generator` CronJob (or a
//! `GENERATION_MODE=loop` sidecar) so a pod restart doesn't lose the
//! corpus.
//!
//! A background task reloads `news.jsonl`/`news.tv` from disk every
//! `NEWS_RELOAD_INTERVAL_MINUTES` (default 5; `0` disables it) and swaps
//! the result in atomically, so a `news_generator` sidecar's ongoing
//! drip-feed writes actually reach traffic without restarting this
//! process. The `Embedder` itself is loaded once and reused across
//! reloads (see `NewsService::load_with_embedder`) — only the corpus and
//! index are re-read.
//!
//! Unlike the sibling MCP servers in this demo family, this server applies
//! NO per-caller data isolation — market news is public data. `called_by`
//! and `roles` are still attached to every response for consistency.

use std::sync::{Arc, RwLock};

use rmcp::transport::{
    streamable_http_server::{session::local::LocalSessionManager, StreamableHttpService},
    StreamableHttpServerConfig,
};
use rmcp::{
    handler::server::common::Extension,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::{Deserialize, Serialize};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use mcp_market_news::common::auth::extract_caller_from_parts;
use mcp_market_news::common::news_service::{NewsItem, NewsService};

const BIND_ADDRESS: &str = "127.0.0.1:8002";
const DEFAULT_JSONL_PATH: &str = "data/news.jsonl";
const DEFAULT_TV_PATH: &str = "data/news.tv";
const DEFAULT_RELOAD_INTERVAL_MINUTES: u64 = 5;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRelevantNewsParams {
    #[schemars(
        description = "Tickers to match exactly (case-insensitive), e.g. from a client's resolved portfolio positions via mcp-portfolio."
    )]
    #[serde(default)]
    pub tickers: Vec<String>,

    #[schemars(
        description = "Sectors to match exactly (stage 1) and/or semantically (stage 2 fallback), e.g. 'logistics'."
    )]
    #[serde(default)]
    pub sectors: Vec<String>,
}

/// Uniform response envelope: the narrowed news list, plus who called the
/// tool and their roles — same convention as every other MCP server in
/// this demo family, even though this one doesn't gate on identity.
#[derive(Debug, Serialize)]
struct ToolResponse<T> {
    output: T,
    called_by: String,
    roles: Vec<String>,
}

fn streamable_http_config() -> StreamableHttpServerConfig {
    let disable_check = std::env::var("MCP_DISABLE_HOST_CHECK")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);

    let stateful_mode = std::env::var("MCP_STATEFUL_MODE")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);

    if disable_check {
        return StreamableHttpServerConfig::default()
            .disable_allowed_hosts()
            .with_sse_retry(None)
            .with_stateful_mode(stateful_mode)
            .with_json_response(true);
    }

    let mut cfg = StreamableHttpServerConfig::default()
        .with_sse_retry(None)
        .with_stateful_mode(stateful_mode)
        .with_json_response(true);
    if let Ok(extra) = std::env::var("MCP_ALLOWED_HOSTS") {
        let extra_hosts: Vec<String> = extra
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !extra_hosts.is_empty() {
            let merged: Vec<String> = cfg
                .allowed_hosts
                .iter()
                .cloned()
                .chain(extra_hosts)
                .collect();
            cfg = cfg.with_allowed_hosts(merged);
        }
    }
    cfg
}

/// The inner `Arc<NewsService>` is swapped out wholesale by the periodic
/// reload task in `main` — readers clone it out from under the `RwLock`
/// and never hold the lock across a query, so a slow query can't block a
/// reload (or vice versa).
type SharedNewsService = Arc<RwLock<Arc<NewsService>>>;

#[derive(Clone)]
pub struct MarketNewsServer {
    tool_router: ToolRouter<Self>,
    service: SharedNewsService,
}

#[tool_router]
impl MarketNewsServer {
    pub fn new(service: SharedNewsService) -> Self {
        Self {
            tool_router: Self::tool_router(),
            service,
        }
    }

    /// Returns market news relevant to the given tickers/sectors, within a
    /// 48h freshness window.
    ///
    /// Stage 1 (always run first, cheap): exact ticker/sector match against
    /// the in-RAM corpus.
    ///
    /// Stage 2 (only if stage 1 finds fewer than 2 items): a semantic
    /// search over the TurboVec index using an embedding of the requested
    /// sectors, so news that never mentions a ticker by name (e.g. a
    /// generic port-regulation story affecting "logistics") can still
    /// surface. Only hits above a cosine-similarity threshold are kept.
    ///
    /// Never returns the full feed — only the already-narrowed result of
    /// one or both stages.
    #[tool(
        description = "Returns market news relevant to the given tickers/sectors (last 48h). Call this after resolving a client's portfolio positions (e.g. via mcp-portfolio) and pass that portfolio's tickers/sectors. Exact ticker/sector matches are always returned; if fewer than 2 exact matches are found, a semantic search over sector meaning is also run so ticker-agnostic sector news (e.g. a generic logistics regulation story) isn't missed."
    )]
    pub async fn get_relevant_news(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetRelevantNewsParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller_from_parts(&parts);

        // Snapshot the current corpus without holding the lock across the
        // query — a reload can swap in a fresh `Arc<NewsService>` at any
        // time, but this handler always sees a consistent one.
        let service = self.service.read().unwrap().clone();
        let news: Vec<NewsItem> = match service
            .get_relevant_news(&params.tickers, &params.sectors)
            .await
        {
            Ok(news) => news,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Error looking up relevant news: {e}"
                ))]))
            }
        };

        let response = ToolResponse {
            output: news,
            called_by: caller.banker_id,
            roles: caller.roles,
        };

        match serde_json::to_string_pretty(&response) {
            Ok(json_str) => Ok(CallToolResult::success(vec![Content::text(json_str)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error serializing response: {e}"
            ))])),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MarketNewsServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Market News MCP server. Exposes get_relevant_news(tickers, sectors) — \
                 returns public market-news items relevant to the given tickers/sectors \
                 within the last 48h. This server does NOT apply per-client data \
                 isolation (market news is public, not client data), unlike the sibling \
                 mcp-portfolio / mcp-kyc-compliance / mcp-crm-calendar servers in this \
                 demo. Every response still carries called_by/roles for consistency.",
            )
            .with_server_info(Implementation::new(
                "mcp-market-news".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            ))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let jsonl_path =
        std::env::var("NEWS_JSONL_PATH").unwrap_or_else(|_| DEFAULT_JSONL_PATH.to_string());
    let tv_path = std::env::var("NEWS_TV_PATH").unwrap_or_else(|_| DEFAULT_TV_PATH.to_string());

    tracing::info!(jsonl_path, tv_path, "Loading market news corpus and index");
    let initial = NewsService::load(&jsonl_path, &tv_path)?;
    tracing::info!(
        items = initial.item_count(),
        "Market news corpus and index loaded"
    );
    let service: SharedNewsService = Arc::new(RwLock::new(Arc::new(initial)));

    let reload_interval_minutes: u64 = std::env::var("NEWS_RELOAD_INTERVAL_MINUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RELOAD_INTERVAL_MINUTES);
    if reload_interval_minutes > 0 {
        let reload_service = service.clone();
        let reload_jsonl_path = jsonl_path.clone();
        let reload_tv_path = tv_path.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(reload_interval_minutes * 60);
            loop {
                tokio::time::sleep(interval).await;
                // Unlike the retired candle-based Embedder, constructing a
                // fresh one is just reading two env vars — no ~90MB model to
                // avoid reloading, so NewsService::load rebuilds everything.
                match NewsService::load(&reload_jsonl_path, &reload_tv_path) {
                    Ok(fresh) => {
                        let items = fresh.item_count();
                        *reload_service.write().unwrap() = Arc::new(fresh);
                        tracing::info!(items, "Reloaded market news corpus");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to reload market news corpus, keeping previous data");
                    }
                }
            }
        });
        tracing::info!(reload_interval_minutes, "Periodic corpus reload enabled");
    } else {
        tracing::info!("NEWS_RELOAD_INTERVAL_MINUTES=0 — periodic corpus reload disabled");
    }

    let bind_address = std::env::var("BIND_ADDRESS").unwrap_or_else(|_| BIND_ADDRESS.to_string());
    tracing::info!(
        "Starting streamable-http Market News MCP server on {}",
        bind_address
    );

    let service_for_factory = service.clone();
    let http_service = StreamableHttpService::new(
        move || Ok(MarketNewsServer::new(service_for_factory.clone())),
        LocalSessionManager::default().into(),
        streamable_http_config(),
    );

    let router = axum::Router::new().nest_service("/mcp", http_service);
    let tcp_listener = tokio::net::TcpListener::bind(bind_address).await?;
    axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}
