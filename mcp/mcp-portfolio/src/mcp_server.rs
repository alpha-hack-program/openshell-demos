use rmcp::transport::{
    streamable_http_server::{session::local::LocalSessionManager, StreamableHttpService},
    StreamableHttpServerConfig,
};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod common;
use common::portfolio_service::PortfolioService;

const BIND_ADDRESS: &str = "127.0.0.1:8001";

/// Schema is applied once for the whole demo by the mcp-servers Helm chart's
/// post-install/post-upgrade hook Job (see
/// demos/keycloak-oidc/mcp-servers/templates/schema-init-job.yaml), not by
/// this binary. This just fails fast with a clear message if that hook
/// hasn't run yet, instead of surfacing a confusing "relation does not
/// exist" error from the first tool call.
async fn assert_schema_ready(pool: &sqlx::PgPool, tables: &[&str]) -> anyhow::Result<()> {
    for table in tables {
        let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(format!("public.{table}"))
            .fetch_one(pool)
            .await?;
        if exists.is_none() {
            anyhow::bail!(
                "required table `{table}` not found in the database — has the \
                 mcp-servers-schema-init Helm hook run? (`helm upgrade --install` applies it \
                 automatically)"
            );
        }
    }
    Ok(())
}

fn streamable_http_config() -> StreamableHttpServerConfig {
    let disable_check = std::env::var("MCP_DISABLE_HOST_CHECK")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false);

    let stateful_mode = std::env::var("MCP_STATEFUL_MODE")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL debe apuntar al servicio PostgreSQL compartido");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    assert_schema_ready(&pool, &["clients", "positions", "performance_snapshots"]).await?;
    tracing::info!("database schema verified");

    let bind_address = std::env::var("BIND_ADDRESS").unwrap_or_else(|_| BIND_ADDRESS.to_string());
    tracing::info!(
        "iniciando mcp-portfolio (streamable-http) en {}",
        bind_address
    );

    let service = StreamableHttpService::new(
        move || Ok(PortfolioService::new(pool.clone())),
        LocalSessionManager::default().into(),
        streamable_http_config(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind(bind_address).await?;
    axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
        .await?;
    Ok(())
}
