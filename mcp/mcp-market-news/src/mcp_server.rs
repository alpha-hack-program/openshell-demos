//! `mcp-market-news` — MCP server exposing `get_relevant_news`.
//!
//! On startup this loads the whole news corpus (`data/news.jsonl`) into RAM
//! and the persisted TurboVec embedding index (`data/news.tv`) once; the
//! hot query path never touches disk again. Both files are produced ahead
//! of time by the separate `news_generator` batch binary (run it first —
//! see the README) — in Kubernetes, `data/` is meant to be a PVC shared
//! between this service and a `news_generator` CronJob so a pod restart
//! doesn't lose the corpus.
//!
//! Unlike the sibling MCP servers in this demo family, this server applies
//! NO per-caller data isolation — market news is public data. `called_by`
//! and `roles` are still attached to every response for consistency.

use std::sync::Arc;

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

#[derive(Clone)]
pub struct MarketNewsServer {
    tool_router: ToolRouter<Self>,
    service: Arc<NewsService>,
}

#[tool_router]
impl MarketNewsServer {
    pub fn new(service: Arc<NewsService>) -> Self {
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

        let news: Vec<NewsItem> = match self
            .service
            .get_relevant_news(&params.tickers, &params.sectors)
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
    let service = Arc::new(NewsService::load(&jsonl_path, &tv_path)?);
    tracing::info!("Market news corpus and index loaded");

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
