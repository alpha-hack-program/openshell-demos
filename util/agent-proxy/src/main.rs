use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::process::Command;
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(about = "OpenAI-compatible proxy that shells out to an AI agent CLI")]
struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Agent command to run. The user prompt is appended as the last argument.
    /// Codex needs exec mode with sandbox bypass (OpenShell provides the real
    /// sandbox) and --skip-git-repo-check (/sandbox is not a git repo).
    #[arg(
        long,
        env = "AGENT_COMMAND",
        default_value = "codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox"
    )]
    agent_command: String,

    /// Flag used to tell the agent CLI to write its final response to a
    /// file (e.g. codex's `-o`/`--output-last-message`), passed as
    /// `<flag> <tmpfile>` before the prompt. Needed because codex requires
    /// stdin/stdout/stderr to all be a real TTY (this sandbox can't
    /// allocate one itself — see util/agent-proxy comments below) — so
    /// stdout must be inherited rather than redirected, and the response
    /// is read back from this file instead. Set to an empty string to
    /// disable and capture stdout directly instead (e.g. for Claude Code's
    /// `-p` mode, which doesn't require a TTY).
    #[arg(long, env = "OUTPUT_FILE_FLAG", default_value = "-o")]
    output_file_flag: String,
}

// --- Request types (OpenAI chat completions subset) ---

#[derive(Deserialize)]
struct ChatRequest {
    messages: Vec<Message>,
    #[serde(default = "default_model")]
    model: String,
}

fn default_model() -> String {
    "agent-proxy".into()
}

#[derive(Deserialize)]
struct Message {
    role: String,
    content: String,
}

// --- Response types ---

#[derive(Serialize)]
struct ChatResponse {
    id: String,
    object: &'static str,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Serialize)]
struct Choice {
    index: u32,
    message: ResponseMessage,
    finish_reason: &'static str,
}

#[derive(Serialize)]
struct ResponseMessage {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    message: String,
    r#type: &'static str,
}

// --- Handlers ---

async fn health() -> &'static str {
    "ok"
}

async fn chat_completions(
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, Json<ErrorBody>)> {
    let prompt = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "no user message found"))?;

    let agent_command = std::env::var("AGENT_COMMAND").unwrap_or_else(|_| "codex".into());
    let output_file_flag = std::env::var("OUTPUT_FILE_FLAG").unwrap_or_else(|_| "-o".into());

    let parts: Vec<&str> = agent_command.split_whitespace().collect();
    let (program, base_args) = parts
        .split_first()
        .ok_or_else(|| api_error(StatusCode::INTERNAL_SERVER_ERROR, "empty AGENT_COMMAND"))?;

    // Codex's `exec` subcommand refuses to run non-interactively unless
    // stdin, stdout AND stderr all look like a real terminal (isatty) and
    // TERM isn't "dumb". This sandbox can't allocate its own pty (`script`
    // fails with "Permission denied" opening /dev/pts), so agent-proxy
    // can't interpose its own pty to capture stdout while still presenting
    // one to the child. Instead: agent-proxy must be started via a
    // foreground `openshell sandbox exec --tty` (not backgrounded with
    // `nohup &`, which tears down the pty) so its own stdin/stdout/stderr
    // are a genuine, inheritable pty; inherit them all here (and inherit
    // TERM — don't strip it); and have the agent write its answer to a
    // file (`-o`/`--output-last-message` for codex) instead of relying on
    // captured stdout.
    let (status, content) = tokio::task::spawn_blocking({
        let program = program.to_string();
        let base_args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
        let prompt = prompt.to_string();
        move || -> std::io::Result<(std::process::ExitStatus, String)> {
            let out_path =
                std::env::temp_dir().join(format!("agent-proxy-{}.out", uuid::Uuid::new_v4()));

            let mut cmd = Command::new(&program);
            cmd.args(&base_args);

            let status = if output_file_flag.is_empty() {
                let stdout_file = std::fs::File::create(&out_path)?;
                cmd.arg(&prompt)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::from(stdout_file))
                    .stderr(std::process::Stdio::null())
                    .status()?
            } else {
                cmd.arg(&output_file_flag)
                    .arg(&out_path)
                    .arg(&prompt)
                    .stdin(std::process::Stdio::inherit())
                    .stdout(std::process::Stdio::inherit())
                    .stderr(std::process::Stdio::inherit())
                    .status()?
            };

            let raw = std::fs::read_to_string(&out_path).unwrap_or_default();
            let _ = std::fs::remove_file(&out_path);
            Ok((status, strip_ansi(&raw)))
        }
    })
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if !status.success() {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("agent exited {status}: {content}"),
        ));
    }

    Ok(Json(ChatResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion",
        model: req.model,
        choices: vec![Choice {
            index: 0,
            message: ResponseMessage {
                role: "assistant",
                content,
            },
            finish_reason: "stop",
        }],
        usage: Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        },
    }))
}

// Codex may still emit ANSI color codes in non-interactive output —
// strip CSI/OSC escape sequences so the response is plain text.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c == '\u{7}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn api_error(status: StatusCode, msg: &str) -> (StatusCode, Json<ErrorBody>) {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                message: msg.to_string(),
                r#type: "proxy_error",
            },
        }),
    )
}

// --- Main ---

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v1/chat/completions", post(chat_completions));

    let addr = format!("{}:{}", args.host, args.port);
    eprintln!(
        "agent-proxy listening on {addr} (agent: {})",
        args.agent_command
    );

    let listener = TcpListener::bind(&addr).await.expect("failed to bind");
    axum::serve(listener, app).await.expect("server error");
}
