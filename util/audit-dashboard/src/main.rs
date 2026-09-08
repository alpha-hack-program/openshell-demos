use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{Json, Router, extract::State, routing::get};
use clap::Parser;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;

/// Queries openshift-user-workload-monitoring's Thanos-querier from inside
/// the cluster (via this pod's own ServiceAccount token) for the three
/// metrics util/session-auditor pushes, and serves a small JSON snapshot
/// plus a static Preact frontend that polls it. See
/// demos/keycloak-oidc/docs/prometheus-scraping.md for the metrics this
/// reads, and this crate's own README for the RBAC this needs
/// (`[VERIFY]` against a live cluster — no prior confirmed pattern for a
/// workload calling thanos-querier in this repo).
#[derive(Parser, Clone)]
#[command(
    about = "Live users -> sandbox -> MCP server risk/heartbeat graph, sourced from session-auditor metrics"
)]
struct Config {
    /// Namespace to scope every PromQL query to — this dashboard only
    /// ever shows its own demo's sandboxes, never the whole cluster.
    #[arg(long, env = "OPENSHELL_NAMESPACE")]
    namespace: String,

    /// In-cluster Thanos-querier base URL. The tenancy-aware front door
    /// for openshift-user-workload-monitoring — enforces namespace-scoped
    /// RBAC via this pod's bound ServiceAccount token rather than trusting
    /// the query's own namespace label. `[VERIFY]` exact host/port on your
    /// cluster before relying on this default.
    #[arg(
        long,
        env = "THANOS_QUERIER_URL",
        default_value = "https://thanos-querier.openshift-monitoring.svc:9091"
    )]
    thanos_url: String,

    /// How often to re-poll Thanos-querier and rebuild the in-memory graph.
    #[arg(long, env = "REFRESH_INTERVAL_SECS", default_value_t = 5)]
    refresh_interval_secs: u64,

    /// A sandbox with no agent_session_started/agent_turn_heartbeat sample
    /// within this many seconds is rendered dimmed/offline in the
    /// frontend, regardless of its last known risk color.
    #[arg(long, env = "HEARTBEAT_STALE_SECS", default_value_t = 90)]
    heartbeat_stale_secs: u64,

    #[arg(long, env = "PORT", default_value_t = 8080)]
    port: u16,
}

#[derive(Clone, Serialize, Default, Debug)]
struct SandboxNode {
    workspace: String,
    sandbox: String,
    agent: String,
    last_seen_unix: Option<i64>,
    risk_score: Option<f64>,
    risk_level: Option<String>,
    mcp_servers: Vec<String>,
}

#[derive(Clone, Serialize, Default)]
struct Graph {
    generated_at_unix: i64,
    heartbeat_stale_secs: u64,
    sandboxes: Vec<SandboxNode>,
}

struct AppState {
    config: Config,
    http: reqwest::Client,
    graph: RwLock<Graph>,
}

#[derive(Deserialize)]
struct PromResponse {
    status: String,
    data: Option<PromData>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct PromData {
    result: Vec<PromSample>,
}

#[derive(Deserialize)]
struct PromSample {
    metric: HashMap<String, String>,
    // Prometheus's own wire shape: [<query-eval-time>, "<value-as-string>"].
    // For a plain metric query the string is the metric's value; for a
    // `timestamp(...)`-wrapped query the string is the wrapped sample's
    // own timestamp (in seconds) — either way, the number we actually
    // want is always the second element.
    value: (f64, String),
}

#[tokio::main]
async fn main() {
    let config = Config::parse();
    let port = config.port;
    let http = build_http_client();
    let state = Arc::new(AppState {
        config,
        http,
        graph: RwLock::new(Graph::default()),
    });

    let refresh_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = refresh_graph(&refresh_state).await {
                eprintln!("audit-dashboard: refresh failed: {e}");
            }
            tokio::time::sleep(Duration::from_secs(
                refresh_state.config.refresh_interval_secs,
            ))
            .await;
        }
    });

    let app = Router::new()
        .route("/api/graph", get(get_graph))
        .fallback_service(ServeDir::new("dist"))
        .with_state(state);

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("failed to bind listener");
    println!("audit-dashboard listening on :{port}");
    axum::serve(listener, app).await.expect("server error");
}

async fn get_graph(State(state): State<Arc<AppState>>) -> Json<Graph> {
    Json(state.graph.read().await.clone())
}

/// Trusts the CAs mounted into every pod so a plain `reqwest::Client` can
/// verify Thanos-querier's TLS certificate. Loaded once at startup, not
/// per-request — the SA *token* is re-read fresh on every request (see
/// `read_token`) since projected tokens rotate, but these CAs don't change
/// nearly as often.
///
/// Confirmed live: `ca.crt` (the Kubernetes API server's CA) is NOT the
/// right one here and fails TLS verification against Thanos-querier
/// ("self-signed certificate in certificate chain") — OpenShift signs
/// in-cluster *service* serving certificates (like Thanos-querier's) with
/// a separate internal service-ca, whose bundle OpenShift auto-projects
/// into every pod as `service-ca.crt` alongside the regular `ca.crt`, not
/// documented anywhere obvious but present by default. Trust both rather
/// than picking one, since which CA actually signs a given in-cluster
/// endpoint isn't something this binary should have to assume correctly
/// on every OpenShift version.
fn build_http_client() -> reqwest::Client {
    let mut builder = reqwest::Client::builder();
    for path in [
        "/var/run/secrets/kubernetes.io/serviceaccount/service-ca.crt",
        "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt",
    ] {
        if let Ok(ca) = std::fs::read(path)
            && let Ok(cert) = reqwest::Certificate::from_pem(&ca)
        {
            builder = builder.add_root_certificate(cert);
        }
    }
    builder.build().expect("failed to build http client")
}

fn read_token() -> Result<String, String> {
    std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token")
        .map_err(|e| format!("failed to read service account token: {e}"))
}

async fn query_instant(
    http: &reqwest::Client,
    base: &str,
    promql: &str,
) -> Result<Vec<PromSample>, String> {
    let token = read_token()?;
    let resp = http
        .get(format!("{base}/api/v1/query"))
        .bearer_auth(token)
        .query(&[("query", promql)])
        .send()
        .await
        // reqwest's own Display impl for a send error omits the actual
        // underlying cause (e.g. a TLS verification failure) — confirmed
        // live this cost real debugging time (the log just said "error
        // sending request for url (...)" with no hint it was a CA
        // mismatch). Walk the source chain so the next failure is
        // diagnosable from the log alone.
        .map_err(|e| {
            let mut msg = format!("request to thanos-querier failed: {e}");
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                msg.push_str(&format!(" -- caused by: {s}"));
                source = s.source();
            }
            msg
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("thanos-querier returned {status}: {body}"));
    }

    let body: PromResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON from thanos-querier: {e}"))?;

    if body.status != "success" {
        return Err(format!(
            "thanos-querier query failed: {}",
            body.error.unwrap_or_default()
        ));
    }

    Ok(body.data.map(|d| d.result).unwrap_or_default())
}

/// Rebuilds the whole graph from three PromQL queries every tick — cheap
/// enough at this demo's scale, and much simpler than incrementally
/// patching state from a diff. `timestamp(...)` on the two heartbeat
/// metrics recovers each series' own last-sample time (an instant query's
/// own returned timestamp is just "now," not when the sample actually
/// landed — see `PromSample::value`'s doc comment).
async fn refresh_graph(state: &AppState) -> Result<(), String> {
    let ns = &state.config.namespace;
    let base = state.config.thanos_url.trim_end_matches('/');

    let session_started = query_instant(
        &state.http,
        base,
        &format!(r#"timestamp(agent_session_started{{namespace="{ns}"}})"#),
    )
    .await?;
    let turn_heartbeat = query_instant(
        &state.http,
        base,
        &format!(r#"timestamp(agent_turn_heartbeat{{namespace="{ns}"}})"#),
    )
    .await?;
    let risk = query_instant(
        &state.http,
        base,
        &format!(r#"session_compliance_risk_score{{namespace="{ns}"}}"#),
    )
    .await?;

    let mut nodes: HashMap<(String, String), SandboxNode> = HashMap::new();

    for sample in session_started.iter().chain(turn_heartbeat.iter()) {
        let (Some(workspace), Some(sandbox)) =
            (sample.metric.get("workspace"), sample.metric.get("sandbox"))
        else {
            continue;
        };
        let agent = sample.metric.get("agent").cloned().unwrap_or_default();
        let Ok(ts) = sample.value.1.parse::<f64>() else {
            continue;
        };
        let ts = ts as i64;

        let entry = nodes
            .entry((workspace.clone(), sandbox.clone()))
            .or_insert_with(|| SandboxNode {
                workspace: workspace.clone(),
                sandbox: sandbox.clone(),
                ..Default::default()
            });
        if entry.agent.is_empty() {
            entry.agent = agent;
        }
        entry.last_seen_unix = Some(entry.last_seen_unix.map_or(ts, |prev| prev.max(ts)));
    }

    for sample in &risk {
        let (Some(workspace), Some(sandbox)) =
            (sample.metric.get("workspace"), sample.metric.get("sandbox"))
        else {
            continue;
        };
        let entry = nodes
            .entry((workspace.clone(), sandbox.clone()))
            .or_insert_with(|| SandboxNode {
                workspace: workspace.clone(),
                sandbox: sandbox.clone(),
                ..Default::default()
            });
        entry.risk_score = sample.value.1.parse::<f64>().ok();
        entry.risk_level = sample.metric.get("risk_level").cloned();
        entry.mcp_servers = sample
            .metric
            .get("mcp_servers")
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
    }

    let mut sandboxes: Vec<SandboxNode> = nodes.into_values().collect();
    sandboxes.sort_by(|a, b| {
        (a.workspace.as_str(), a.sandbox.as_str()).cmp(&(b.workspace.as_str(), b.sandbox.as_str()))
    });

    let graph = Graph {
        generated_at_unix: now_unix(),
        heartbeat_stale_secs: state.config.heartbeat_stale_secs,
        sandboxes,
    };
    *state.graph.write().await = graph;
    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
