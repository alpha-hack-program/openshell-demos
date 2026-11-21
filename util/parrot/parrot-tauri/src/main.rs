// SPDX-License-Identifier: Apache-2.0

//! parrot-tauri's backend: a thin Tauri shell around `parrot-core`, the same
//! crate `parrot-tui` and `parrot-gpui` render from.
//!
//! Unlike `parrot-gpui` (whose own executor doesn't drive Tokio, forcing a
//! hand-rolled `Runtime` + channel bridge — see `parrot-gpui/src/app.rs`'s
//! module doc), Tauri commands are natively `async fn` and Tauri's command
//! dispatcher already runs on a Tokio-backed runtime. We still keep an
//! explicit `Arc<Runtime>` (used to `block_on` the initial connect before
//! `tauri::Builder::run()` takes the main thread, same as `parrot-gpui`'s
//! `main.rs`), but turns themselves are just `runtime.spawn`ed tasks that
//! stream back to the webview over a `tauri::ipc::Channel` — no manual
//! executor bridging needed on the UI side, since the UI is a webview, not
//! a Rust render loop.
//!
//! `parrot-core`'s `SessionState`/`AgentEvent`/`LogEntry`/`RunStatus` derive
//! `Serialize` (see their doc comments) specifically so this crate can ship
//! them to the webview as JSON unchanged, instead of re-implementing
//! Claude Code/Codex JSONL classification in JavaScript.

mod cli;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use parrot_core::{
    detect_passthrough_environment, Agent, AgentEvent, LogEntry, ParrotClient, RunStatus,
    SandboxPhase, SessionState, StreamSource, StreamedExecOptions, ThemeMode, TurnOptions,
};
use tauri::ipc::Channel;
use tauri::State;
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

use cli::Cli;

fn agent_str(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude",
        Agent::Codex => "codex",
    }
}

fn theme_str(theme: ThemeMode) -> &'static str {
    match theme {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::System => "system",
    }
}

/// `SandboxPhase` is `#[non_exhaustive]` upstream (new phases can appear
/// with a gateway upgrade) and derives neither `Display` nor `Serialize`,
/// hence the wildcard arm and the manual mapping here instead of in
/// `parrot-core`, which stays free of a webview-flavored string format.
fn sandbox_phase_str(phase: SandboxPhase) -> &'static str {
    match phase {
        SandboxPhase::Unspecified => "unspecified",
        SandboxPhase::Provisioning => "provisioning",
        SandboxPhase::Ready => "ready",
        SandboxPhase::Error => "error",
        SandboxPhase::Deleting => "deleting",
        SandboxPhase::Unknown => "unknown",
        SandboxPhase::Stopping => "stopping",
        SandboxPhase::Stopped => "stopped",
        SandboxPhase::Starting => "starting",
        SandboxPhase::Completed => "completed",
        SandboxPhase::Suspending => "suspending",
        SandboxPhase::Suspended => "suspended",
        _ => "unknown",
    }
}

struct AppState {
    client: Arc<ParrotClient>,
    runtime: Arc<Runtime>,
    sandbox: String,
    workspace: Option<String>,
    agent: Agent,
    mcp_config: Option<String>,
    environment: HashMap<String, String>,
    theme: ThemeMode,
    no_splash: bool,
    prompt: Option<String>,
    identity_greeting: String,
    /// Fetched once at startup (see `main`'s connect block) so the splash
    /// screen reports it before the user submits a first turn — previously
    /// the only way to learn the sandbox wasn't `Ready` was an exec failing.
    sandbox_phase: SandboxPhase,
    session: Arc<Mutex<SessionState>>,
    turn_running: Arc<AtomicBool>,
}

/// Everything the webview needs to render its header/splash on load —
/// mirrors what `parrot-gpui`'s `ParrotWindow::new` reads off `Cli`/
/// `ParrotClient` at startup.
#[derive(serde::Serialize)]
struct Greeting {
    identity_greeting: String,
    gateway_name: String,
    gateway_endpoint: String,
    sandbox: String,
    sandbox_phase: String,
    agent: String,
    workspace: Option<String>,
    theme: String,
    no_splash: bool,
    prompt: Option<String>,
}

#[tauri::command]
fn greeting(state: State<'_, AppState>) -> Greeting {
    Greeting {
        identity_greeting: state.identity_greeting.clone(),
        gateway_name: state.client.gateway.name.clone(),
        gateway_endpoint: state.client.gateway.endpoint.clone(),
        sandbox: state.sandbox.clone(),
        sandbox_phase: sandbox_phase_str(state.sandbox_phase).to_string(),
        agent: agent_str(state.agent).to_string(),
        workspace: state.workspace.clone(),
        theme: theme_str(state.theme).to_string(),
        no_splash: state.no_splash,
        prompt: state.prompt.clone(),
    }
}

/// A point-in-time view of the session, sent to the webview after every
/// stdout/stderr line and again on completion — simplest correct thing for
/// a demo tool's log sizes; see this crate's `AGENTS`-adjacent notes if log
/// volume ever makes whole-snapshot resends worth replacing with deltas.
#[derive(Clone, serde::Serialize)]
struct SessionSnapshot {
    log: Vec<LogEntry>,
    status: RunStatus,
}

fn send_snapshot(session: &Mutex<SessionState>, on_event: &Channel<SessionSnapshot>) {
    let (log, status) = {
        let session = session.lock().expect("session mutex poisoned");
        (session.log.clone(), session.status)
    };
    let _ = on_event.send(SessionSnapshot { log, status });
}

#[tauri::command]
async fn submit_turn(
    prompt: String,
    on_event: Channel<SessionSnapshot>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("empty prompt".to_string());
    }
    if state.turn_running.swap(true, Ordering::SeqCst) {
        return Err("a turn is already running".to_string());
    }

    let session_id = {
        let mut session = state.session.lock().expect("session mutex poisoned");
        session.begin_turn();
        session.push_user_prompt(&prompt);
        session.session_id.clone()
    };
    send_snapshot(&state.session, &on_event);

    let client = state.client.clone();
    let runtime = state.runtime.clone();
    let sandbox = state.sandbox.clone();
    let workspace = state.workspace.clone();
    let agent = state.agent;
    let mcp_config = state
        .mcp_config
        .clone()
        .or_else(|| agent.default_mcp_config().map(str::to_string));
    let environment = state.environment.clone();
    let session = state.session.clone();
    let turn_running = state.turn_running.clone();
    let resume_session_id = if agent.supports_resume() {
        session_id
    } else {
        None
    };

    runtime.spawn(async move {
        let turn_opts = TurnOptions {
            prompt: &prompt,
            mcp_config: mcp_config.as_deref(),
            session_id: resume_session_id.as_deref(),
        };
        let command = agent.build_turn(&turn_opts);
        let exec_opts = StreamedExecOptions {
            environment,
            ..Default::default()
        };

        let mut exec = match client
            .exec_streamed(&sandbox, workspace.as_deref(), &command, exec_opts)
            .await
        {
            Ok(exec) => exec,
            Err(err) => {
                // Mirrors parrot-gpui's `TurnEvent::Failed` handling: the
                // exec never started (connect/RPC-open failure), so push
                // the error into the session itself rather than inventing
                // a second wire-message shape alongside `SessionSnapshot`.
                {
                    let mut session = session.lock().expect("session mutex poisoned");
                    session.log.push(LogEntry {
                        source: StreamSource::Stderr,
                        event: AgentEvent::Error(err.to_string()),
                    });
                    session.status = RunStatus::Crashed;
                }
                send_snapshot(&session, &on_event);
                turn_running.store(false, Ordering::SeqCst);
                return;
            }
        };

        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut outcome_done = false;

        loop {
            tokio::select! {
                biased;
                line = exec.stdout.next(), if !stdout_done => match line {
                    Some(line) => {
                        session.lock().expect("session mutex poisoned").push_stdout(&line);
                        send_snapshot(&session, &on_event);
                    }
                    None => stdout_done = true,
                },
                line = exec.stderr.next(), if !stderr_done => match line {
                    Some(line) => {
                        session.lock().expect("session mutex poisoned").push_stderr(&line);
                        send_snapshot(&session, &on_event);
                    }
                    None => stderr_done = true,
                },
                outcome = &mut exec.outcome, if !outcome_done => {
                    if let Ok(outcome) = outcome {
                        session.lock().expect("session mutex poisoned").finish(&outcome);
                        send_snapshot(&session, &on_event);
                    }
                    outcome_done = true;
                }
            }

            if stdout_done && stderr_done && outcome_done {
                break;
            }
        }

        turn_running.store(false, Ordering::SeqCst);
    });

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    // Same fail-fast-before-any-window behavior as parrot-tui/parrot-gpui:
    // a banker with no active gateway or no sandbox access gets a message
    // on the terminal, never a blank window.
    let runtime = Arc::new(Runtime::new().expect("failed to start tokio runtime"));
    let (client, workspace, sandbox_phase) = runtime
        .block_on(async {
            let client =
                ParrotClient::connect(cli.gateway.as_deref(), cli.gateway_endpoint.as_deref())
                    .await?;
            let workspace = client.resolve_workspace(cli.workspace.as_deref()).await?;
            let sandbox_phase = client
                .sandbox_phase(&cli.sandbox, workspace.as_deref())
                .await?;
            Ok::<_, parrot_core::ParrotError>((client, workspace, sandbox_phase))
        })
        .unwrap_or_else(|err| {
            eprintln!("parrot-tauri: {err}");
            std::process::exit(1);
        });
    eprintln!(
        "parrot-tauri: sandbox '{}' is {}",
        cli.sandbox,
        sandbox_phase_str(sandbox_phase)
    );

    let mut environment = detect_passthrough_environment();
    environment.extend(cli.env.iter().cloned());
    if environment.is_empty() {
        eprintln!(
            "parrot-tauri: no environment variables passed through (none auto-detected, no --env given)"
        );
    } else {
        let mut keys: Vec<_> = environment.keys().cloned().collect();
        keys.sort();
        eprintln!("parrot-tauri: passing environment: {}", keys.join(", "));
    }

    let identity_greeting = client.identity.greeting();

    let state = AppState {
        client: Arc::new(client),
        runtime,
        sandbox: cli.sandbox.clone(),
        workspace,
        agent: cli.agent,
        mcp_config: cli.mcp_config.clone(),
        environment,
        theme: cli.theme,
        no_splash: cli.no_splash,
        prompt: cli.prompt.clone(),
        identity_greeting,
        sandbox_phase,
        session: Arc::new(Mutex::new(SessionState::new())),
        turn_running: Arc::new(AtomicBool::new(false)),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![greeting, submit_turn])
        .run(tauri::generate_context!())
        .expect("error while running parrot-tauri");
}
