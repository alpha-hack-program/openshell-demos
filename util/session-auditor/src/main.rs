use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{io::AsyncWriteExt, net::TcpListener, process::Command, sync::RwLock};

// Baked in at compile time from a plain-text file in the repo, never a
// runtime path/flag — the whole point is that no `sandbox exec` argument
// or `--env` override can point this at a different, friendlier prompt.
const CLASSIFICATION_PROMPT: &str = include_str!("../prompt.txt");

const RISK_LEVELS: [&str; 4] = [
    "none",
    "self_refused",
    "blocked_attempt",
    "complied_or_fabricated",
];

#[derive(Parser, Clone)]
#[command(
    about = "Classifies the latest Claude Code session transcript for compliance/tenant-violation risk via an LLM call, and serves the verdict as a Prometheus /metrics endpoint"
)]
struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Directory to search recursively for the newest *.jsonl session transcript.
    #[arg(
        long,
        env = "TRANSCRIPTS_DIR",
        default_value = "/sandbox/.claude/projects"
    )]
    transcripts_dir: String,

    /// How often to check whether the newest transcript changed. The LLM is
    /// only called when it actually has — not on every tick.
    #[arg(long, env = "POLL_INTERVAL_SECS", default_value_t = 30)]
    poll_interval_secs: u64,

    #[arg(long, env = "ANTHROPIC_API_KEY")]
    anthropic_api_key: String,

    #[arg(
        long,
        env = "ANTHROPIC_BASE_URL",
        default_value = "https://api.anthropic.com"
    )]
    anthropic_base_url: String,

    #[arg(long, env = "ANTHROPIC_MODEL")]
    anthropic_model: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Verdict {
    risk_level: String,
    score: u8,
    evidence: String,
}

impl Default for Verdict {
    fn default() -> Self {
        Verdict {
            risk_level: "none".into(),
            score: 0,
            evidence: "no session analyzed yet".into(),
        }
    }
}

struct AppState {
    verdict: RwLock<Verdict>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let state = Arc::new(AppState {
        verdict: RwLock::new(Verdict::default()),
    });

    tokio::spawn(poll_loop(state.clone(), args.clone()));

    let app = Router::new()
        .route("/metrics", get(metrics))
        .with_state(state);
    let addr = format!("{}:{}", args.host, args.port);
    eprintln!(
        "session-auditor listening on {addr} (transcripts: {}, poll interval: {}s)",
        args.transcripts_dir, args.poll_interval_secs
    );

    let listener = TcpListener::bind(&addr).await.expect("failed to bind");
    axum::serve(listener, app).await.expect("server error");
}

async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let v = state.verdict.read().await.clone();

    let mut body = String::new();
    body.push_str(
        "# HELP session_compliance_risk_score Compliance/tenant-violation risk score for the most recently analyzed agent session (0=none 1=self_refused 2=blocked_attempt 3=complied_or_fabricated)\n",
    );
    body.push_str("# TYPE session_compliance_risk_score gauge\n");
    body.push_str(&format!("session_compliance_risk_score {}\n", v.score));

    body.push_str("# HELP session_compliance_risk_level One-hot indicator of the current risk level classification\n");
    body.push_str("# TYPE session_compliance_risk_level gauge\n");
    for level in RISK_LEVELS {
        let val = if level == v.risk_level { 1 } else { 0 };
        body.push_str(&format!(
            "session_compliance_risk_level{{level=\"{level}\"}} {val}\n"
        ));
    }

    (
        StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        body,
    )
}

async fn poll_loop(state: Arc<AppState>, args: Args) {
    let mut last_mtime: Option<SystemTime> = None;
    let mut interval = tokio::time::interval(Duration::from_secs(args.poll_interval_secs));

    loop {
        interval.tick().await;

        let Some(path) = find_newest_jsonl(Path::new(&args.transcripts_dir)) else {
            continue;
        };
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        if Some(mtime) == last_mtime {
            continue; // no change since last check — skip the LLM call
        }

        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let transcript = extract_transcript(&content);
        if transcript.trim().is_empty() {
            last_mtime = Some(mtime);
            continue;
        }

        match classify(&args, &transcript).await {
            Ok(verdict) => {
                eprintln!("session-auditor: new verdict: {verdict:?}");
                *state.verdict.write().await = verdict;
                last_mtime = Some(mtime);
            }
            Err(e) => {
                eprintln!("session-auditor: classification failed, will retry next tick: {e}");
            }
        }
    }
}

fn find_newest_jsonl(dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    let mut stack = vec![dir.to_path_buf()];

    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                && let Ok(meta) = entry.metadata()
                && let Ok(mtime) = meta.modified()
                && newest.as_ref().map(|(_, t)| mtime > *t).unwrap_or(true)
            {
                newest = Some((path, mtime));
            }
        }
    }

    newest.map(|(p, _)| p)
}

/// Best-effort flattening of Claude Code's JSONL transcript format into a
/// plain-text conversation the classification LLM call can read. Skips
/// "attachment"/"queue-operation" lines (tool-list deltas, etc.) — noise
/// for this purpose.
fn extract_transcript(content: &str) -> String {
    let mut out = String::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => "USER",
            Some("assistant") => "ASSISTANT",
            _ => continue,
        };
        let Some(msg_content) = v.pointer("/message/content") else {
            continue;
        };
        let mut piece = String::new();
        flatten_content(msg_content, &mut piece);
        let piece = piece.trim();
        if !piece.is_empty() {
            out.push_str(role);
            out.push_str(": ");
            out.push_str(piece);
            out.push('\n');
        }
    }
    // Cap the size sent to the LLM — most recent context matters most.
    const MAX_CHARS: usize = 12_000;
    if out.len() > MAX_CHARS {
        let start = out.len() - MAX_CHARS;
        out[start..].to_string()
    } else {
        out
    }
}

fn flatten_content(content: &Value, out: &mut String) {
    match content {
        Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Value::Array(items) => {
            for item in items {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            out.push_str(text);
                            out.push('\n');
                        }
                    }
                    Some("tool_use") => {
                        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let input = item.get("input").map(|i| i.to_string()).unwrap_or_default();
                        out.push_str(&format!("[tool_call: {name} input={input}]\n"));
                    }
                    Some("tool_result") => {
                        let is_error = item
                            .get("is_error")
                            .and_then(|e| e.as_bool())
                            .unwrap_or(false);
                        let mut result_text = String::new();
                        if let Some(c) = item.get("content") {
                            flatten_content(c, &mut result_text);
                        }
                        out.push_str(&format!(
                            "[tool_result error={is_error}: {}]\n",
                            result_text.trim()
                        ));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Shells out to `curl` rather than embedding an HTTPS client — matches
/// `agent-proxy`'s own pattern of shelling out to a CLI, and avoids needing
/// TLS/crypto crates to cross-compile for the musl target.
async fn classify(args: &Args, transcript: &str) -> Result<Verdict, String> {
    let body = serde_json::json!({
        "model": args.anthropic_model,
        "max_tokens": 300,
        "system": CLASSIFICATION_PROMPT,
        "messages": [{"role": "user", "content": transcript}],
    })
    .to_string();

    let mut child = Command::new("curl")
        .args([
            "-sS",
            "-X",
            "POST",
            &format!("{}/v1/messages", args.anthropic_base_url),
            "-H",
            &format!("x-api-key: {}", args.anthropic_api_key),
            "-H",
            "anthropic-version: 2023-06-01",
            "-H",
            "content-type: application/json",
            "--data-binary",
            "@-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| format!("failed to write request body to curl: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("failed to wait for curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "curl exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let resp_json: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        format!(
            "invalid JSON from LLM API ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })?;
    // Reasoning models (e.g. DeepSeek) emit a "thinking" block before the
    // "text" block — don't assume content[0] is the answer, find the first
    // block actually typed "text".
    let text = resp_json
        .pointer("/content")
        .and_then(|c| c.as_array())
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("no text content in LLM response: {resp_json}"))?;

    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str::<Verdict>(cleaned)
        .map_err(|e| format!("failed to parse verdict JSON ({e}): {cleaned}"))
}
