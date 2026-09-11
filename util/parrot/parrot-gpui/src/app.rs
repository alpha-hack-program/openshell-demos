// SPDX-License-Identifier: Apache-2.0

//! The parrot-gpui dashboard: welcome splash, header, scrollable agent
//! output, and a bottom composer — the same information the TUI shows,
//! laid out as a native window instead of a terminal frame.
//!
//! `parrot-core`'s exec/agent plumbing (`exec_streamed`, `Agent::build_turn`,
//! `SessionState`) is built on Tokio, which gpui's own executor doesn't
//! drive. Each submitted turn is therefore handed to a dedicated
//! [`tokio::runtime::Runtime`] (owned by `main` and shared here via `Arc`),
//! and its stdout/stderr/outcome are relayed back to the gpui entity over a
//! plain `tokio::sync::mpsc` channel — polling that channel's `recv()` from
//! gpui's own `cx.spawn` task does not itself require a Tokio context, only
//! sending into it does.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, rgb, Context, FocusHandle, KeyDownEvent, Render, ScrollHandle, SharedString,
    Window,
};
use parrot_core::{
    tool_result_text, Agent, AgentEvent, ExecOutcome, LogEntry, Palette, ParrotClient, Rgb,
    RunStatus, SessionState, StreamSource, StreamedExecOptions, TurnOptions,
};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

use crate::cli::Cli;

/// Matches `parrot-tui`'s splash `AUTO_DISMISS`.
const SPLASH_DURATION: Duration = Duration::from_millis(1300);

fn rgb_u32(color: Rgb) -> u32 {
    (u32::from(color.0) << 16) | (u32::from(color.1) << 8) | u32::from(color.2)
}

pub struct ParrotWindow {
    runtime: Arc<Runtime>,
    client: Arc<ParrotClient>,
    sandbox: String,
    workspace: Option<String>,
    agent: Agent,
    mcp_config: Option<String>,
    environment: HashMap<String, String>,
    palette: Palette,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    state: SessionState,
    composing: String,
    turn_running: bool,
    show_splash: bool,
    identity_greeting: String,
    gateway_name: String,
    gateway_endpoint: String,
}

impl ParrotWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cli: &Cli,
        client: Arc<ParrotClient>,
        workspace: Option<String>,
        environment: HashMap<String, String>,
        runtime: Arc<Runtime>,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);

        if !cli.no_splash {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SPLASH_DURATION).await;
                this.update(cx, |state, cx| {
                    state.show_splash = false;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        let identity_greeting = client.identity.greeting();
        let gateway_name = client.gateway.name.clone();
        let gateway_endpoint = client.gateway.endpoint.clone();

        Self {
            runtime,
            client,
            sandbox: cli.sandbox.clone(),
            workspace,
            agent: cli.agent,
            mcp_config: cli.mcp_config.clone(),
            environment,
            palette,
            focus_handle,
            scroll_handle: ScrollHandle::new(),
            state: SessionState::new(),
            composing: String::new(),
            turn_running: false,
            show_splash: !cli.no_splash,
            identity_greeting,
            gateway_name,
            gateway_endpoint,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.show_splash {
            self.show_splash = false;
            cx.notify();
            return;
        }

        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "enter" => self.submit_turn(cx),
            "backspace" => {
                self.composing.pop();
                cx.notify();
            }
            _ => {
                let modifiers = &keystroke.modifiers;
                if modifiers.control || modifiers.platform || modifiers.function {
                    return;
                }
                if let Some(key_char) = &keystroke.key_char {
                    self.composing.push_str(key_char);
                    cx.notify();
                }
            }
        }
    }

    /// Submit the composed prompt as one turn, mirroring `parrot-tui`'s
    /// `run_turn`: build the agent's argv, open the streaming exec, and
    /// relay stdout/stderr/outcome into `state` as they arrive. Always
    /// interactive — there is no one-shot mode here, that's `parrot-tui`.
    fn submit_turn(&mut self, cx: &mut Context<Self>) {
        if self.turn_running {
            return;
        }
        let prompt = self.composing.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        self.composing.clear();
        self.turn_running = true;
        self.state.begin_turn();
        self.scroll_handle.scroll_to_bottom();
        cx.notify();

        let client = self.client.clone();
        let runtime = self.runtime.clone();
        let sandbox = self.sandbox.clone();
        let workspace = self.workspace.clone();
        let agent = self.agent;
        let mcp_config = self
            .mcp_config
            .clone()
            .or_else(|| agent.default_mcp_config().map(str::to_string));
        let environment = self.environment.clone();
        let session_id = self.state.session_id.clone();

        cx.spawn(async move |this, cx| {
            let turn_opts = TurnOptions {
                prompt: &prompt,
                mcp_config: mcp_config.as_deref(),
                session_id: if agent.supports_resume() {
                    session_id.as_deref()
                } else {
                    None
                },
            };
            let command = agent.build_turn(&turn_opts);
            let exec_opts = StreamedExecOptions {
                environment,
                ..Default::default()
            };

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
            runtime.spawn(async move {
                drive_turn(
                    &client,
                    &sandbox,
                    workspace.as_deref(),
                    &command,
                    exec_opts,
                    tx,
                )
                .await;
            });

            while let Some(event) = rx.recv().await {
                let delivered = this.update(cx, |state, cx| {
                    apply_turn_event(state, event);
                    state.scroll_handle.scroll_to_bottom();
                    cx.notify();
                });
                if delivered.is_err() {
                    // Window closed mid-turn; nothing left to update.
                    return;
                }
            }

            this.update(cx, |state, cx| {
                state.turn_running = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_splash(&self) -> impl IntoElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .text_3xl()
                    .text_color(rgb(rgb_u32(self.palette.accent)))
                    .child("Meridian Inc."),
            )
            .child(
                div()
                    .text_color(rgb(rgb_u32(self.palette.foreground)))
                    .child(self.identity_greeting.clone()),
            )
            .child(
                div()
                    .text_color(rgb(rgb_u32(self.palette.muted)))
                    .child(format!(
                        "Gateway: {} ({})",
                        self.gateway_name, self.gateway_endpoint
                    )),
            )
    }

    fn render_header(&self) -> impl IntoElement {
        let (status_text, status_color) = match self.state.status {
            RunStatus::Running => ("running".to_string(), self.palette.accent),
            RunStatus::Completed { exit_code: 0 } => {
                ("completed".to_string(), self.palette.tool_result)
            }
            RunStatus::Completed { exit_code } => {
                (format!("completed (exit {exit_code})"), self.palette.error)
            }
            RunStatus::Crashed => ("crashed".to_string(), self.palette.error),
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(rgb(rgb_u32(self.palette.muted)))
            .child(format!("parrot — sandbox {} — ", self.sandbox))
            .child(
                div()
                    .text_color(rgb(rgb_u32(status_color)))
                    .child(status_text),
            )
    }

    fn render_log(&self) -> impl IntoElement {
        div()
            .id("log")
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .children(self.state.log.iter().map(|entry| {
                let (text, color) = render_log_entry(entry, &self.palette);
                div().text_color(rgb(rgb_u32(color))).child(text)
            }))
    }

    fn render_composer(&self) -> impl IntoElement {
        let hint: SharedString = if self.turn_running {
            "running…".into()
        } else {
            "".into()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(rgb(rgb_u32(self.palette.muted)))
            .child(
                div()
                    .text_color(rgb(rgb_u32(self.palette.accent)))
                    .child("> "),
            )
            .child(div().flex_1().child(self.composing.clone()))
            .child(
                div()
                    .text_color(rgb(rgb_u32(self.palette.muted)))
                    .child(hint),
            )
    }
}

impl Render for ParrotWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div()
            .key_context("ParrotWindow")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(rgb_u32(self.palette.background)))
            .text_color(rgb(rgb_u32(self.palette.foreground)));

        if self.show_splash {
            root.child(self.render_splash())
        } else {
            root.child(self.render_header())
                .child(self.render_log())
                .child(self.render_composer())
        }
    }
}

/// Messages relayed from the Tokio-side [`drive_turn`] task to the gpui
/// entity, one per stdout/stderr line plus a final outcome (or a
/// connect/stream failure).
enum TurnEvent {
    Stdout(String),
    Stderr(String),
    Finished(ExecOutcome),
    Failed(String),
}

fn apply_turn_event(window: &mut ParrotWindow, event: TurnEvent) {
    match event {
        TurnEvent::Stdout(line) => window.state.push_stdout(&line),
        TurnEvent::Stderr(line) => window.state.push_stderr(&line),
        TurnEvent::Finished(outcome) => window.state.finish(&outcome),
        TurnEvent::Failed(message) => {
            window.state.log.push(LogEntry {
                source: StreamSource::Stderr,
                event: AgentEvent::Error(message),
            });
            window.state.status = RunStatus::Crashed;
        }
    }
}

/// Runs entirely on the Tokio runtime: open the streaming exec and forward
/// every line/outcome over `tx` as it arrives. Mirrors `parrot-tui`'s
/// `run_turn` streaming loop, minus the terminal drawing and keypress
/// polling (gpui redraws reactively via `cx.notify()` instead).
async fn drive_turn(
    client: &ParrotClient,
    sandbox: &str,
    workspace: Option<&str>,
    command: &[String],
    exec_opts: StreamedExecOptions,
    tx: tokio::sync::mpsc::UnboundedSender<TurnEvent>,
) {
    let mut exec = match client
        .exec_streamed(sandbox, workspace, command, exec_opts)
        .await
    {
        Ok(exec) => exec,
        Err(err) => {
            let _ = tx.send(TurnEvent::Failed(err.to_string()));
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
                Some(line) => { let _ = tx.send(TurnEvent::Stdout(line)); }
                None => stdout_done = true,
            },
            line = exec.stderr.next(), if !stderr_done => match line {
                Some(line) => { let _ = tx.send(TurnEvent::Stderr(line)); }
                None => stderr_done = true,
            },
            outcome = &mut exec.outcome, if !outcome_done => {
                if let Ok(outcome) = outcome {
                    let _ = tx.send(TurnEvent::Finished(outcome));
                }
                outcome_done = true;
            }
        }

        if stdout_done && stderr_done && outcome_done {
            return;
        }
    }
}

/// Classify one log entry into its display text and semantic palette
/// color. Mirrors `parrot-tui`'s `render_log_entry`, but returns a single
/// string instead of pre-wrapped `Line`s — gpui's text layout wraps (and
/// breaks on embedded newlines) on its own, so there's no ratatui-style
/// manual wrapping to do here.
fn render_log_entry(entry: &LogEntry, palette: &Palette) -> (String, Rgb) {
    match &entry.event {
        AgentEvent::AssistantText(text) => (format!("» {text}"), palette.assistant_text),
        AgentEvent::ToolCall { name, .. } => (format!("→ tool call: {name}"), palette.tool_call),
        AgentEvent::ToolResult {
            is_error: true,
            content,
        } => {
            let text = tool_result_text(content);
            let text = if text.is_empty() {
                "tool result: error".to_string()
            } else {
                format!("tool result: error: {text}")
            };
            (format!("✗ {text}"), palette.error)
        }
        AgentEvent::ToolResult {
            is_error: false, ..
        } => ("✓ tool result".to_string(), palette.tool_result),
        AgentEvent::Thinking(text) => (format!("… {text}"), palette.muted),
        AgentEvent::ThinkingTokens(estimated_tokens) => (
            format!("… thinking… ({estimated_tokens} tokens)"),
            palette.muted,
        ),
        AgentEvent::Result(_) => ("● final result".to_string(), palette.result),
        AgentEvent::Error(message) => (format!("! {message}"), palette.error),
        AgentEvent::Info(message) => (format!("ℹ {message}"), palette.muted),
        AgentEvent::Unknown(value) => (
            format!("· unrecognized event: {}", unknown_label(value)),
            palette.muted,
        ),
        AgentEvent::Unparseable(line) => (format!("  {line}"), palette.muted),
    }
}

/// Same fallback labeling `parrot-tui` uses for genuinely unclassified
/// events — Claude Code nests its discriminator as a sibling `subtype`;
/// Codex nests it one level down as `item.type`.
fn unknown_label(value: &serde_json::Value) -> String {
    let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("?");
    let nested = value
        .get("subtype")
        .or_else(|| value.get("item").and_then(|item| item.get("type")))
        .and_then(|v| v.as_str());
    match nested {
        Some(nested) => format!("{ty}/{nested}"),
        None => ty.to_string(),
    }
}
