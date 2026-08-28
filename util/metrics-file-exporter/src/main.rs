use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use clap::Parser;
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(about = "Serves a file's contents as a Prometheus /metrics endpoint")]
struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Path to the file to read on every scrape (re-read each request, not cached).
    #[arg(long, env = "METRICS_FILE", default_value = "/sandbox/metrics.prom")]
    metrics_file: String,
}

async fn metrics(path: String) -> impl IntoResponse {
    match tokio::fs::read_to_string(&path).await {
        Ok(body) => (
            StatusCode::OK,
            [("Content-Type", "text/plain; version=0.0.4")],
            body,
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("Content-Type", "text/plain; version=0.0.4")],
            format!("# error reading {path}: {e}\n"),
        ),
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let metrics_file = args.metrics_file.clone();
    let app = Router::new().route(
        "/metrics",
        get(move || metrics(metrics_file.clone())),
    );

    let addr = format!("{}:{}", args.host, args.port);
    eprintln!("metrics-file-exporter listening on {addr} (file: {})", args.metrics_file);

    let listener = TcpListener::bind(&addr).await.expect("failed to bind");
    axum::serve(listener, app).await.expect("server error");
}
