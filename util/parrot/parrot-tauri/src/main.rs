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
mod client_snapshot;
mod mcp;
mod meeting_brief;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use parrot_core::{
    detect_passthrough_environment, Agent, AgentEvent, LogEntry, McpAuthConfig, ParrotClient,
    RunStatus, SandboxPhase, SessionState, StreamSource, StreamedExecOptions, ThemeMode,
    TurnOptions,
};
use tauri::ipc::Channel;
use tauri::State;
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

use cli::Cli;
use mcp::McpState;

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
    /// `None` until a sandbox is attached — either resolved eagerly at
    /// startup (`--sandbox` was given) or via the `connect_sandbox` command
    /// (the splash screen's Connect button).
    sandbox: Mutex<Option<String>>,
    /// Prefix used by `connect_sandbox` to compute
    /// `{sandbox_prefix}{agent}-{workspace}`. Unused when `--sandbox` was
    /// given explicitly.
    sandbox_prefix: String,
    workspace: Option<String>,
    agent: Agent,
    mcp_config: Option<String>,
    environment: HashMap<String, String>,
    theme: ThemeMode,
    no_splash: bool,
    prompt: Option<String>,
    identity_greeting: String,
    /// Mirrors `sandbox`: `None` until a sandbox is attached. Re-fetched by
    /// `connect_sandbox`/`sandbox_status` so the splash/header can report it
    /// without waiting for an exec to fail first.
    sandbox_phase: Mutex<Option<SandboxPhase>>,
    session: Arc<Mutex<SessionState>>,
    turn_running: Arc<AtomicBool>,
}

/// Everything the webview needs to render its header/splash on load —
/// mirrors what `parrot-gpui`'s `ParrotWindow::new` reads off `Cli`/
/// `ParrotClient` at startup.
#[derive(serde::Serialize)]
struct Greeting {
    identity_greeting: String,
    display_name: Option<String>,
    gateway_name: String,
    gateway_endpoint: String,
    /// `null` until a sandbox is attached — the webview shows the splash's
    /// Connect button instead of auto-dismissing in that case.
    sandbox: Option<String>,
    sandbox_phase: Option<String>,
    agent: String,
    workspace: Option<String>,
    theme: String,
    no_splash: bool,
    prompt: Option<String>,
    /// Whether `--mcp-gateway-url`/`--keycloak-host` were both given — the
    /// webview uses this to decide whether to show the toolbar's "Connect"
    /// affordance at all, versus a launch that only ever wanted the chat
    /// pane.
    mcp_available: bool,
}

#[tauri::command]
fn greeting(state: State<'_, AppState>, mcp_state: State<'_, McpState>) -> Greeting {
    let sandbox = state.sandbox.lock().expect("sandbox mutex poisoned").clone();
    let sandbox_phase = state
        .sandbox_phase
        .lock()
        .expect("sandbox_phase mutex poisoned")
        .map(sandbox_phase_str)
        .map(str::to_string);
    Greeting {
        identity_greeting: state.identity_greeting.clone(),
        display_name: state.client.identity.display_name.clone(),
        gateway_name: state.client.gateway.name.clone(),
        gateway_endpoint: state.client.gateway.endpoint.clone(),
        sandbox,
        sandbox_phase,
        agent: agent_str(state.agent).to_string(),
        workspace: state.workspace.clone(),
        theme: theme_str(state.theme).to_string(),
        no_splash: state.no_splash,
        prompt: state.prompt.clone(),
        mcp_available: mcp_state.configured(),
    }
}

/// What `connect_sandbox`/`sandbox_status` report back to the webview.
#[derive(serde::Serialize)]
struct SandboxStatus {
    sandbox: String,
    sandbox_phase: String,
}

/// Resolves and verifies `{sandbox_prefix}{agent}-{user}` against an
/// already-provisioned sandbox — the splash screen's Connect button.
///
/// `user` is the MCP Gateway login's `preferred_username` (the frontend
/// logs into MCP first and passes it here) — the stable Keycloak login
/// name, not `ParrotClient::identity`'s `display_name` (which prefers the
/// human-readable `name` claim, e.g. "Alice Meridian" instead of "alice",
/// and would produce a wrong/malformed sandbox name). Falls back to
/// `state.workspace` (the primary gateway's own resolved workspace) when
/// `user` is omitted — e.g. MCP isn't configured for this launch at all.
/// The primary `ParrotClient`/its workspace scoping for the actual RPC
/// call are unchanged either way; only the *name being looked up* can
/// come from the MCP identity.
///
/// Per this demo's self-service-onboarding invariant (see
/// `demos/keycloak-oidc/docs/self-service-onboarding.md`), this only ever
/// *reads* sandbox state; it never creates one. If the computed name isn't
/// provisioned, the error tells the user to ask an admin rather than
/// failing generically.
#[tauri::command]
async fn connect_sandbox(user: Option<String>, state: State<'_, AppState>) -> Result<SandboxStatus, String> {
    {
        let sandbox = state.sandbox.lock().expect("sandbox mutex poisoned").clone();
        if let Some(name) = sandbox {
            let phase = state
                .sandbox_phase
                .lock()
                .expect("sandbox_phase mutex poisoned")
                .expect("sandbox_phase set whenever sandbox is");
            return Ok(SandboxStatus {
                sandbox: name,
                sandbox_phase: sandbox_phase_str(phase).to_string(),
            });
        }
    }

    let user = user
        .or_else(|| state.workspace.clone())
        .ok_or_else(|| "can't determine your username — ask an admin to add you to a workspace".to_string())?;
    let candidate = format!("{}{}-{}", state.sandbox_prefix, agent_str(state.agent), user);
    let phase = state
        .client
        .sandbox_phase(&candidate, state.workspace.as_deref())
        .await
        .map_err(|_| {
            format!(
                "No sandbox named '{candidate}' is provisioned for your account yet. Ask an admin to provision it (see demos/keycloak-oidc/scripts/14-/15-/16-provision-*-sandbox.sh)."
            )
        })?;

    *state.sandbox.lock().expect("sandbox mutex poisoned") = Some(candidate.clone());
    *state.sandbox_phase.lock().expect("sandbox_phase mutex poisoned") = Some(phase);

    Ok(SandboxStatus {
        sandbox: candidate,
        sandbox_phase: sandbox_phase_str(phase).to_string(),
    })
}

/// Re-checks the already-attached sandbox's phase — the header's heartbeat
/// poll. Errors if no sandbox is attached yet (call `connect_sandbox`
/// first).
#[tauri::command]
async fn sandbox_status(state: State<'_, AppState>) -> Result<SandboxStatus, String> {
    let sandbox = state
        .sandbox
        .lock()
        .expect("sandbox mutex poisoned")
        .clone()
        .ok_or_else(|| "not connected to a sandbox yet".to_string())?;
    let phase = state
        .client
        .sandbox_phase(&sandbox, state.workspace.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    *state.sandbox_phase.lock().expect("sandbox_phase mutex poisoned") = Some(phase);
    Ok(SandboxStatus {
        sandbox,
        sandbox_phase: sandbox_phase_str(phase).to_string(),
    })
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
    let sandbox = state
        .sandbox
        .lock()
        .expect("sandbox mutex poisoned")
        .clone()
        .ok_or_else(|| "not connected to a sandbox yet".to_string())?;
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
            no_builtin_tools: false,
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
    // a banker with no active gateway gets a message on the terminal, never
    // a blank window. Sandbox resolution is no longer part of this eager
    // chain when `--sandbox` is omitted — identity/workspace are enough to
    // open the window; the splash screen's Connect button resolves the
    // sandbox afterward (see `connect_sandbox`).
    let runtime = Arc::new(Runtime::new().expect("failed to start tokio runtime"));
    let (client, workspace) = runtime
        .block_on(async {
            let client =
                ParrotClient::connect(cli.gateway.as_deref(), cli.gateway_endpoint.as_deref())
                    .await?;
            let workspace = client.resolve_workspace(cli.workspace.as_deref()).await?;
            Ok::<_, parrot_core::ParrotError>((client, workspace))
        })
        .unwrap_or_else(|err| {
            eprintln!("parrot-tauri: {err}");
            std::process::exit(1);
        });

    let (sandbox, sandbox_phase) = match cli.sandbox.clone() {
        Some(name) => {
            let phase = runtime
                .block_on(client.sandbox_phase(&name, workspace.as_deref()))
                .unwrap_or_else(|err| {
                    eprintln!("parrot-tauri: {err}");
                    std::process::exit(1);
                });
            eprintln!("parrot-tauri: sandbox '{name}' is {}", sandbox_phase_str(phase));
            (Some(name), Some(phase))
        }
        None => {
            eprintln!("parrot-tauri: no --sandbox given; connect from the splash screen");
            (None, None)
        }
    };

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
        sandbox: Mutex::new(sandbox),
        sandbox_prefix: cli.sandbox_prefix.clone().unwrap_or_default(),
        workspace,
        agent: cli.agent,
        mcp_config: cli.mcp_config.clone(),
        environment,
        theme: cli.theme,
        no_splash: cli.no_splash,
        prompt: cli.prompt.clone(),
        identity_greeting,
        sandbox_phase: Mutex::new(sandbox_phase),
        session: Arc::new(Mutex::new(SessionState::new())),
        turn_running: Arc::new(AtomicBool::new(false)),
    };

    let auth_config = cli.keycloak_host.as_ref().map(|host| McpAuthConfig {
        issuer: format!("https://{host}/realms/{}", cli.keycloak_realm),
        client_id: cli.keycloak_client_id.clone(),
        scopes: vec!["openid".to_string(), "profile".to_string()],
        insecure: cli.mcp_insecure,
    });
    let mcp_state = McpState::new(cli.mcp_gateway_url.clone(), auth_config, cli.mcp_servers);

    tauri::Builder::default()
        .manage(state)
        .manage(mcp_state)
        .invoke_handler(tauri::generate_handler![
            greeting,
            connect_sandbox,
            sandbox_status,
            submit_turn,
            mcp::mcp_login,
            mcp::mcp_list_tools,
            mcp::mcp_call_tool,
            mcp::mcp_today,
            meeting_brief::mcp_meeting_brief,
            meeting_brief::list_saved_briefs,
            meeting_brief::load_saved_brief,
            client_snapshot::mcp_client_snapshot
        ])
        .run(tauri::generate_context!())
        .expect("error while running parrot-tauri");
}
