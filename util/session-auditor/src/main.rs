use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

// Baked in at compile time from plain-text files in the repo, never a
// runtime path/flag/env var — the whole point is that no `sandbox exec`
// argument or `--env` override can redirect them. The prompt shouldn't be
// swappable for a friendlier one; the OTLP endpoint shouldn't be
// redirectable or removable (denial of the audit signal) or pointed
// somewhere else.
const CLASSIFICATION_PROMPT: &str = include_str!("../prompt.txt");
const OTLP_ENDPOINT: &str = include_str!("../otlp-endpoint.txt");

/// Invoked as a `Stop` hook by either Claude Code (see
/// demos/keycloak-oidc/images/claude-audit/managed-settings.json) or Codex
/// (see demos/keycloak-oidc/images/codex-audit/requirements.toml) — one
/// process per turn, not a long-running server. Both agents' Stop hooks
/// share the same core stdin fields (session_id, transcript_path),
/// confirmed live; only the transcript's own on-disk format differs,
/// handled by the two parsers below and auto-selected from the
/// transcript_path shape. The whole point of an audit hook is to observe,
/// not to gate the user's turn — the classify+push work happens in a
/// detached background worker (see `--worker` below) so the visible hook
/// invocation only pays the cost of spawning a process, not the LLM
/// round-trip. Never blocks the user's turn on failure either way: every
/// error path logs and exits 0.
#[derive(Parser, Clone)]
#[command(
    about = "Claude Code / Codex Stop-hook that classifies a session transcript for compliance/tenant-violation risk and pushes the verdict via OTLP"
)]
struct Args {
    /// Internal: re-invokes this same binary as a detached background
    /// worker reading the staged hook event from this path, instead of the
    /// normal stdin-reading hook entry point. Not meant to be set by hand.
    #[arg(long, hide = true)]
    worker: Option<String>,

    /// Which wire format the classification backend speaks — mirrors this
    /// demo's own `byo-claude`/`byo-codex` split (one credential env
    /// name/auth style per agent's native API), except here it's per
    /// *classification backend*, independent of which agent (Claude Code
    /// or Codex) actually runs in the sandbox: today's default is
    /// Anthropic-Messages-API-compatible, but the classifier could just as
    /// well be a plain OpenAI-Chat-Completions-compatible endpoint.
    #[arg(long, env = "AUDITOR_API_STYLE", value_enum, default_value_t = ApiStyle::Anthropic)]
    api_style: ApiStyle,

    // Deliberately not ANTHROPIC_API_KEY/ANTHROPIC_BASE_URL/ANTHROPIC_MODEL
    // (or OPENAI_*): a sandbox that attaches both a Claude Code/Codex LLM
    // provider and the session-auditor provider needs the two to inject
    // different values under different names — confirmed live, the
    // gateway rejects attaching two providers that both claim the same
    // credential env key.
    #[arg(long, env = "AUDITOR_ANTHROPIC_API_KEY")]
    anthropic_api_key: Option<String>,

    #[arg(long, env = "AUDITOR_ANTHROPIC_MODEL")]
    anthropic_model: Option<String>,

    #[arg(long, env = "AUDITOR_OPENAI_API_KEY")]
    openai_api_key: Option<String>,

    #[arg(long, env = "AUDITOR_OPENAI_MODEL")]
    openai_model: Option<String>,

    /// Base URL for whichever backend `api_style` selects. No default here
    /// on purpose — the right default depends on `api_style`, resolved in
    /// `Config::resolve`.
    #[arg(long, env = "AUDITOR_LLM_BASE_URL")]
    llm_base_url: Option<String>,

    /// Timeout for the classification LLM call — a hung call must not hang
    /// the user's turn indefinitely.
    #[arg(long, env = "CLASSIFY_TIMEOUT_SECS", default_value_t = 20)]
    classify_timeout_secs: u64,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum ApiStyle {
    Anthropic,
    Openai,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Verdict {
    risk_level: String,
    score: u8,
    evidence: String,
}

/// Fully-resolved configuration — constructing one is the only place a
/// missing/misconfigured env var is allowed to short-circuit the hook.
struct Config {
    api_style: ApiStyle,
    api_key: String,
    base_url: String,
    model: String,
    classify_timeout_secs: u64,
}

impl Config {
    fn resolve(args: &Args) -> Option<Self> {
        let (api_key, model, default_base_url) = match args.api_style {
            ApiStyle::Anthropic => (
                args.anthropic_api_key.clone()?,
                args.anthropic_model.clone()?,
                "https://api.anthropic.com",
            ),
            ApiStyle::Openai => (
                args.openai_api_key.clone()?,
                args.openai_model.clone()?,
                "https://api.openai.com",
            ),
        };
        Some(Config {
            api_style: args.api_style,
            api_key,
            base_url: args
                .llm_base_url
                .clone()
                .unwrap_or_else(|| default_base_url.to_string()),
            model,
            classify_timeout_secs: args.classify_timeout_secs,
        })
    }
}

fn main() {
    let args = Args::parse();

    let Some(worker_file) = &args.worker else {
        spawn_worker_and_exit();
        return;
    };

    run_worker(&args, worker_file);
    let _ = std::fs::remove_file(worker_file);
}

/// The visible hook entry point: stages the event JSON to a file (so the
/// background worker doesn't depend on the hook's own stdin pipe, which
/// closes the moment this process exits) and spawns a detached re-exec of
/// this same binary to do the actual work, then returns immediately.
/// Deliberately doesn't inherit stdin/stdout/stderr for the child: if the
/// invoking agent waits for this hook's own stdout to reach EOF before
/// considering it done, an inherited fd would keep it waiting for the
/// background worker too.
fn spawn_worker_and_exit() {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        log(&format!("failed to read hook stdin: {e}"));
        return;
    }

    let staged_path = std::env::temp_dir().join(format!(
        "session-auditor-hook-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    if let Err(e) = std::fs::write(&staged_path, &input) {
        log(&format!("failed to stage hook event: {e}"));
        return;
    }

    let exe = std::env::current_exe().unwrap_or_else(|_| "/usr/local/bin/session-auditor".into());

    // Not `.output()`/`.wait()` on purpose — this child must outlive us.
    // No need to re-pass anthropic_base_url/classify_timeout_secs
    // explicitly — Command inherits the environment by default, and
    // clap re-reads the same env vars (or the same defaults) in the child.
    let spawned = Command::new(exe)
        .arg("--worker")
        .arg(&staged_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    if let Err(e) = spawned {
        log(&format!("failed to spawn background worker: {e}"));
        let _ = std::fs::remove_file(&staged_path);
    }
}

/// Does the actual (slow) work: classify + push. Runs detached from the
/// agent's own process tree, after the visible hook has already exited.
/// Dispatches on `hook_event_name`, present in every hook's stdin JSON for
/// both agents. `Stop` is the only path that calls an LLM — `SessionStart`
/// and `UserPromptSubmit` are deliberately just a heartbeat (agent is
/// alive / a turn is happening), no classification, no LLM round-trip.
fn run_worker(args: &Args, staged_path: &str) {
    let input = match std::fs::read_to_string(staged_path) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("failed to read staged hook event: {e}"));
            return;
        }
    };

    let event: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            log(&format!("invalid hook JSON in staged event: {e}"));
            return;
        }
    };

    let session_id = event
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let hook_event_name = event
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match hook_event_name {
        "SessionStart" => handle_heartbeat("agent_session_started", &session_id),
        "UserPromptSubmit" => handle_heartbeat("agent_turn_heartbeat", &session_id),
        "Stop" => handle_stop(args, &session_id, &event),
        other => log(&format!(
            "session {session_id}: unhandled hook event {other:?}, ignoring"
        )),
    }
}

/// `SessionStart`/`UserPromptSubmit` handler: no LLM call, just a presence
/// signal — the agent identity comes from which managed hook config file
/// is baked into this image, not from parsing the event (more reliable
/// than assuming what fields a given event happens to include).
fn handle_heartbeat(metric_name: &str, session_id: &str) {
    let agent = detect_agent();
    let identity = detect_sandbox_identity();
    let mut attrs = vec![("session_id", session_id), ("agent", agent)];
    if let Some((workspace, sandbox)) = identity.as_ref() {
        attrs.push(("workspace", workspace.as_str()));
        attrs.push(("sandbox", sandbox.as_str()));
    }
    if let Err(e) = push_gauge_metric(metric_name, 1.0, &attrs) {
        log(&format!(
            "session {session_id}: failed to push {metric_name}: {e}"
        ));
    }
}

fn detect_agent() -> &'static str {
    if std::path::Path::new("/etc/claude-code").exists() {
        "claude"
    } else if std::path::Path::new("/etc/codex").exists() {
        "codex"
    } else {
        "unknown"
    }
}

/// Best-effort sandbox-owner attribution. `/etc/hostname` on this OpenShell
/// version is set to `<workspace>--<sandbox-name>` — confirmed live
/// (2026-09-02), but this is an internal naming convention, not a
/// documented public API, so a future OpenShell version could change the
/// format; if the `--` separator isn't found, this just omits both
/// attributes rather than guessing. Deliberately reads `/etc/hostname`
/// rather than trusting an env var: confirmed live that a non-root sandbox
/// process can change neither the kernel hostname (`hostname newname` →
/// "must be root to change the host name") nor the `/etc/hostname` file
/// itself (`Permission denied`) — unlike any env var, which the sandbox
/// occupant can freely override on their own `sandbox exec` calls. Two
/// alternatives were tried and rejected first: the demo's own
/// `$USER_ACCESS_TOKEN` is a network-layer resolve placeholder, never a
/// real token inside the sandbox, so there's nothing to decode locally;
/// and `sandbox upload`/`--upload` at creation time can't write to a
/// root-owned path like `/etc/` either (same Landlock +
/// plain-Unix-permissions wall the binary itself would hit), so there's no
/// tamper-resistant channel for admin-supplied *per-instance* data at all
/// today — only the hostname, which the platform sets itself.
fn detect_sandbox_identity() -> Option<(String, String)> {
    let hostname = std::fs::read_to_string("/etc/hostname").ok()?;
    let (workspace, sandbox) = hostname.trim().split_once("--")?;
    (!workspace.is_empty() && !sandbox.is_empty())
        .then(|| (workspace.to_string(), sandbox.to_string()))
}

/// `Stop` handler: the only path that reads a transcript and calls an LLM.
fn handle_stop(args: &Args, session_id: &str, event: &Value) {
    let Some(config) = Config::resolve(args) else {
        log(&format!(
            "missing required configuration for {:?} classification (matching API key/model not set), skipping this turn",
            args.api_style
        ));
        return;
    };

    let Some(transcript_path) = event.get("transcript_path").and_then(|v| v.as_str()) else {
        log("no transcript_path in hook event, nothing to classify");
        return;
    };

    let content = match std::fs::read_to_string(transcript_path) {
        Ok(c) => c,
        Err(e) => {
            log(&format!("could not read transcript {transcript_path}: {e}"));
            return;
        }
    };

    let transcript = extract_transcript(transcript_path, &content);
    if transcript.trim().is_empty() {
        log(&format!("session {session_id}: empty transcript, skipping"));
        return;
    }

    let verdict = match classify(&config, &transcript) {
        Ok(v) => v,
        Err(e) => {
            log(&format!("session {session_id}: classification failed: {e}"));
            return;
        }
    };

    log(&format!("session {session_id}: verdict {verdict:?}"));

    let identity = detect_sandbox_identity();
    let mut attrs = vec![
        ("session_id", session_id),
        ("risk_level", verdict.risk_level.as_str()),
    ];
    if let Some((workspace, sandbox)) = identity.as_ref() {
        attrs.push(("workspace", workspace.as_str()));
        attrs.push(("sandbox", sandbox.as_str()));
    }
    if let Err(e) = push_gauge_metric(
        "session_compliance_risk_score",
        verdict.score as f64,
        &attrs,
    ) {
        log(&format!("session {session_id}: failed to push metric: {e}"));
    }
}

/// Command hooks' stderr isn't reliably surfaced back to the terminal in
/// every Claude Code surface — write to a fixed, human-inspectable file too.
fn log(msg: &str) {
    eprintln!("session-auditor: {msg}");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/session-auditor-hook.log")
    {
        let _ = writeln!(f, "session-auditor: {msg}");
    }
}

/// Dispatches to the right parser based on the transcript path shape —
/// confirmed live that Claude Code and Codex use distinct, incompatible
/// on-disk JSONL schemas even though their Stop-hook stdin payloads share
/// the same core field names.
fn extract_transcript(transcript_path: &str, content: &str) -> String {
    let raw = if transcript_path.contains("/.codex/sessions/") {
        extract_transcript_codex(content)
    } else {
        extract_transcript_claude(content)
    };
    truncate_transcript(raw)
}

// Cap the size sent to the LLM — most recent context matters most.
fn truncate_transcript(out: String) -> String {
    const MAX_CHARS: usize = 12_000;
    if out.len() > MAX_CHARS {
        let start = out.len() - MAX_CHARS;
        out[start..].to_string()
    } else {
        out
    }
}

/// Best-effort flattening of Claude Code's JSONL transcript format into a
/// plain-text conversation the classification LLM call can read. Skips
/// "attachment"/"queue-operation" lines (tool-list deltas, etc.) — noise
/// for this purpose.
fn extract_transcript_claude(content: &str) -> String {
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
    out
}

/// Codex's JSONL transcript format — confirmed live (2026-08-31,
/// codex-cli 0.146.0). Structurally different from Claude Code's: plain
/// messages are flat `event_msg` lines (`payload.type`:
/// `user_message`/`agent_message`, `payload.message` a plain string), and
/// tool calls are separate top-level `response_item` lines
/// (`payload.type`: `function_call`/`function_call_output`), not nested
/// inside a message's content array.
fn extract_transcript_codex(content: &str) -> String {
    let mut out = String::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(payload) = v.get("payload") else {
            continue;
        };
        match payload.get("type").and_then(|t| t.as_str()) {
            Some("user_message") => {
                if let Some(msg) = payload.get("message").and_then(|m| m.as_str()) {
                    out.push_str("USER: ");
                    out.push_str(msg);
                    out.push('\n');
                }
            }
            Some("agent_message") => {
                if let Some(msg) = payload.get("message").and_then(|m| m.as_str()) {
                    out.push_str("ASSISTANT: ");
                    out.push_str(msg);
                    out.push('\n');
                }
            }
            Some("function_call") => {
                let name = payload.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let arguments = payload
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .unwrap_or("");
                out.push_str(&format!("[tool_call: {name} input={arguments}]\n"));
            }
            Some("function_call_output") => {
                let output = payload.get("output").and_then(|o| o.as_str()).unwrap_or("");
                out.push_str(&format!("[tool_result: {output}]\n"));
            }
            _ => {}
        }
    }
    out
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

/// Dispatches to the wire format `Config::resolve` picked, then parses the
/// resulting text the same way regardless of backend — both APIs are asked
/// for the identical JSON-shaped answer, just wrapped differently.
fn classify(config: &Config, transcript: &str) -> Result<Verdict, String> {
    let text = match config.api_style {
        ApiStyle::Anthropic => classify_anthropic(config, transcript)?,
        ApiStyle::Openai => classify_openai(config, transcript)?,
    };

    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str::<Verdict>(cleaned)
        .map_err(|e| format!("failed to parse verdict JSON ({e}): {cleaned}"))
}

/// Shells out to `curl` rather than embedding an HTTPS client — matches
/// `agent-proxy`'s own pattern of shelling out to a CLI, and avoids needing
/// TLS/crypto crates to cross-compile for the musl target.
fn classify_anthropic(config: &Config, transcript: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "model": config.model,
        // Reasoning models (e.g. DeepSeek) spend tokens on a "thinking"
        // block before the final JSON answer — 300 was observed live to be
        // too small for longer transcripts, truncating before any answer
        // was produced at all.
        "max_tokens": 1024,
        "system": CLASSIFICATION_PROMPT,
        "messages": [{"role": "user", "content": transcript}],
    })
    .to_string();

    let output = Command::new("curl")
        .args([
            "-sS",
            "-f",
            "-m",
            &config.classify_timeout_secs.to_string(),
            "-X",
            "POST",
            &format!("{}/v1/messages", config.base_url),
            "-H",
            &format!("x-api-key: {}", config.api_key),
            "-H",
            "anthropic-version: 2023-06-01",
            "-H",
            "content-type: application/json",
            "-d",
            &body,
        ])
        .output()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

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
    resp_json
        .pointer("/content")
        .and_then(|c| c.as_array())
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no text content in LLM response: {resp_json}"))
}

/// Plain OpenAI Chat Completions — deliberately not the Responses API:
/// classification is a single non-agentic turn with no tool calls, so
/// Chat Completions is the simplest wire format that's broadly compatible
/// (see docs/inference-api-compatibility.md for why Responses API matters
/// for the *agents themselves*, which need namespace-tool support this
/// call never uses).
fn classify_openai(config: &Config, transcript: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "model": config.model,
        "max_tokens": 1024,
        "messages": [
            {"role": "system", "content": CLASSIFICATION_PROMPT},
            {"role": "user", "content": transcript},
        ],
    })
    .to_string();

    let output = Command::new("curl")
        .args([
            "-sS",
            "-f",
            "-m",
            &config.classify_timeout_secs.to_string(),
            "-X",
            "POST",
            &format!("{}/v1/chat/completions", config.base_url),
            "-H",
            &format!("Authorization: Bearer {}", config.api_key),
            "-H",
            "content-type: application/json",
            "-d",
            &body,
        ])
        .output()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

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

    resp_json
        .pointer("/choices/0/message/content")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no message content in LLM response: {resp_json}"))
}

/// Shared by all three hooks — the compliance-score gauge from `Stop` and
/// the two heartbeat gauges from `SessionStart`/`UserPromptSubmit` all push
/// through the same OTLP/HTTP JSON shape, just with a different name/value
/// and attribute set.
fn push_gauge_metric(name: &str, value: f64, attributes: &[(&str, &str)]) -> Result<(), String> {
    let now_unix_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos()
        .to_string();

    let attrs: Vec<Value> = attributes
        .iter()
        .map(|(k, v)| serde_json::json!({"key": k, "value": {"stringValue": v}}))
        .collect();

    let body = serde_json::json!({
        "resourceMetrics": [{
            "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "session-auditor"}}]},
            "scopeMetrics": [{
                "scope": {"name": "session-auditor"},
                "metrics": [{
                    "name": name,
                    "gauge": {
                        "dataPoints": [{
                            "asDouble": value,
                            "timeUnixNano": now_unix_nanos,
                            "attributes": attrs,
                        }],
                    },
                }],
            }],
        }],
    })
    .to_string();

    let output = Command::new("curl")
        .args([
            "-sS",
            "-f",
            "-m",
            "10",
            "-X",
            "POST",
            &format!("{}/v1/metrics", OTLP_ENDPOINT.trim()),
            "-H",
            "Content-Type: application/json",
            "-d",
            &body,
        ])
        .output()
        .map_err(|e| format!("failed to spawn curl: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "curl exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}
