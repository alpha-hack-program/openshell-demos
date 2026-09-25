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
    div, prelude::*, rgb, App, ClipboardItem, Context, FocusHandle, FontWeight, HighlightStyle,
    KeyDownEvent, Render, ScrollHandle, SharedString, StyledText, Window,
};
use parrot_core::{
    render_inline_markdown, tool_result_text, Agent, AgentEvent, ExecOutcome, InlineStyle,
    LogEntry, Palette, ParrotClient, Rgb, RunStatus, SessionState, StreamSource,
    StreamedExecOptions, TurnOptions,
};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

use crate::cli::Cli;

/// Matches `parrot-tui`'s splash `AUTO_DISMISS`.
const SPLASH_DURATION: Duration = Duration::from_millis(1300);

/// Per-OS candidates for [`resolve_monospace_font`], most-preferred
/// first. Each entry must be a real, literal installed font family name,
/// not a CSS-style generic keyword: gpui's Linux backend
/// (`font-kit`/`fontdb`) resolves `Font.family` by exact string match
/// against installed family names (see `platform/linux/text_system.rs`'s
/// `load_family`) — it does not go through fontconfig's generic-alias
/// layer, so a value like `"monospace"` silently matches nothing and
/// falls back to the default font with no error at all. `Font.fallbacks`
/// doesn't rescue this either — on this backend it's only consulted for
/// glyphs missing from an otherwise-valid family (e.g. CJK/emoji), not
/// for resolving a missing family name in the first place — hence trying
/// several literal candidates ourselves instead of naming just one.
/// "Liberation Mono"/"DejaVu Sans Mono" are confirmed present via
/// `fc-list` on Fedora/RHEL; the macOS and Windows lists are inferred
/// from what those OSes bundle by default, not yet checked live on
/// either.
#[cfg(target_os = "macos")]
const MONOSPACE_FONT_CANDIDATES: &[&str] = &["Menlo", "SF Mono", "Monaco"];
#[cfg(target_os = "windows")]
const MONOSPACE_FONT_CANDIDATES: &[&str] = &["Consolas", "Cascadia Mono", "Lucida Console"];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const MONOSPACE_FONT_CANDIDATES: &[&str] = &[
    "Liberation Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Ubuntu Mono",
    "Fira Mono",
];

/// Picks the first of [`MONOSPACE_FONT_CANDIDATES`] that's actually
/// installed (queried live via gpui's own font system — see the
/// candidates list's doc comment for why a single hardcoded name isn't
/// safe), falling back to gpui's own UI-font marker (`.SystemUIFont`,
/// proportional) if genuinely none of them are present. That fallback is
/// deliberately not silent — better than repeating the exact "requested
/// a font that doesn't exist, got the wrong one with no explanation"
/// failure mode this function exists to avoid.
fn resolve_monospace_font(cx: &App) -> SharedString {
    let installed = cx.text_system().all_font_names();
    MONOSPACE_FONT_CANDIDATES
        .iter()
        .find(|candidate| {
            installed
                .iter()
                .any(|name| name.eq_ignore_ascii_case(candidate))
        })
        .map(|&name| SharedString::from(name))
        .unwrap_or_else(|| {
            eprintln!(
                "parrot-gpui: none of {MONOSPACE_FONT_CANDIDATES:?} are installed \
                 — falling back to the proportional UI font"
            );
            SharedString::from(".SystemUIFont")
        })
}

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
    /// Byte offset into `composing`, always on a char boundary.
    cursor: usize,
    turn_running: bool,
    show_splash: bool,
    identity_greeting: String,
    gateway_name: String,
    gateway_endpoint: String,
    monospace_font: SharedString,
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
        let monospace_font = resolve_monospace_font(cx);

        let mut this = Self {
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
            cursor: 0,
            turn_running: false,
            show_splash: !cli.no_splash,
            identity_greeting,
            gateway_name,
            gateway_endpoint,
            monospace_font,
        };

        // --prompt: submit immediately rather than waiting for a keypress —
        // runs concurrently with the splash timer above (submit_turn
        // doesn't check show_splash), so the turn is already streaming by
        // the time the splash dismisses.
        if let Some(prompt) = &cli.prompt {
            this.composing = prompt.clone();
            this.submit_turn(cx);
        }

        this
    }

    /// Enter submits (see `submit_turn`); Shift+Enter or Alt+Enter inserts a
    /// literal newline instead, so a prompt can span multiple lines.
    /// Left/Right/Up/Down/Home/End move the cursor (Up/Down move between
    /// `\n`-delimited lines, not wrapped visual rows, preserving column);
    /// Ctrl/Cmd+C/X/V copy, cut, and paste the whole buffer (there's no
    /// selection range — only a single cursor position).
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.show_splash {
            self.show_splash = false;
            cx.notify();
            return;
        }

        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        let ctrl_or_cmd = modifiers.control || modifiers.platform;

        match keystroke.key.as_str() {
            "enter" if modifiers.shift || modifiers.alt => self.insert_at_cursor("\n", cx),
            "enter" => self.submit_turn(cx),
            "backspace" => self.backspace(cx),
            "delete" => self.delete_forward(cx),
            "left" => self.move_left(cx),
            "right" => self.move_right(cx),
            "up" => self.move_up(cx),
            "down" => self.move_down(cx),
            "home" => self.move_home(cx),
            "end" => self.move_end(cx),
            "c" if ctrl_or_cmd => self.copy_to_clipboard(cx),
            "x" if ctrl_or_cmd => self.cut_to_clipboard(cx),
            "v" if ctrl_or_cmd => self.paste_from_clipboard(cx),
            _ => {
                if modifiers.control || modifiers.platform || modifiers.function {
                    return;
                }
                if let Some(key_char) = keystroke.key_char.clone() {
                    self.insert_at_cursor(&key_char, cx);
                }
            }
        }
    }

    fn insert_at_cursor(&mut self, text: &str, cx: &mut Context<Self>) {
        self.composing.insert_str(self.cursor, text);
        self.cursor += text.len();
        cx.notify();
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.cursor == 0 {
            return;
        }
        let start = prev_char_boundary(&self.composing, self.cursor);
        self.composing.replace_range(start..self.cursor, "");
        self.cursor = start;
        cx.notify();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.cursor >= self.composing.len() {
            return;
        }
        let end = next_char_boundary(&self.composing, self.cursor);
        self.composing.replace_range(self.cursor..end, "");
        cx.notify();
    }

    fn move_left(&mut self, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            self.cursor = prev_char_boundary(&self.composing, self.cursor);
            cx.notify();
        }
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        if self.cursor < self.composing.len() {
            self.cursor = next_char_boundary(&self.composing, self.cursor);
            cx.notify();
        }
    }

    fn move_up(&mut self, cx: &mut Context<Self>) {
        let start = line_start(&self.composing, self.cursor);
        if start == 0 {
            return;
        }
        let column = column_chars(&self.composing, self.cursor);
        let prev_end = start - 1;
        let prev_start = line_start(&self.composing, prev_end);
        self.cursor = offset_at_column(&self.composing, prev_start, prev_end, column);
        cx.notify();
    }

    fn move_down(&mut self, cx: &mut Context<Self>) {
        let end = line_end(&self.composing, self.cursor);
        if end == self.composing.len() {
            return;
        }
        let column = column_chars(&self.composing, self.cursor);
        let next_start = end + 1;
        let next_end = line_end(&self.composing, next_start);
        self.cursor = offset_at_column(&self.composing, next_start, next_end, column);
        cx.notify();
    }

    fn move_home(&mut self, cx: &mut Context<Self>) {
        self.cursor = line_start(&self.composing, self.cursor);
        cx.notify();
    }

    fn move_end(&mut self, cx: &mut Context<Self>) {
        self.cursor = line_end(&self.composing, self.cursor);
        cx.notify();
    }

    fn copy_to_clipboard(&mut self, cx: &mut Context<Self>) {
        if self.composing.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.composing.clone()));
    }

    fn cut_to_clipboard(&mut self, cx: &mut Context<Self>) {
        if self.composing.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(std::mem::take(
            &mut self.composing,
        )));
        self.cursor = 0;
        cx.notify();
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert_at_cursor(&text, cx);
        }
    }

    /// Submit the composed prompt as one turn, mirroring `parrot-tui`'s
    /// `run_turn`: build the agent's argv, open the streaming exec, and
    /// relay stdout/stderr/outcome into `state` as they arrive. Called
    /// both from a real Enter keypress and, for `--prompt`, once
    /// automatically from `ParrotWindow::new` — either way the window
    /// stays open afterward for further typed turns.
    fn submit_turn(&mut self, cx: &mut Context<Self>) {
        if self.turn_running {
            return;
        }
        let prompt = self.composing.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        self.composing.clear();
        self.cursor = 0;
        self.turn_running = true;
        self.state.begin_turn();
        self.state.push_user_prompt(&prompt);
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
                // Markdown syntax is only meaningful in the assistant's own
                // prose (its actual answer) — running it over this app's
                // own synthetic log lines (tool-call/tool-result markers,
                // thinking, errors, the user's echoed prompt, ...) would
                // misrender any stray `*`/`` ` ``/`#` that shows up
                // incidentally in a tool name or error message.
                let (display, runs) = if matches!(entry.event, AgentEvent::AssistantText(_)) {
                    render_inline_markdown(&text)
                } else {
                    (text, Vec::new())
                };
                let highlights = runs
                    .into_iter()
                    .map(|run| (run.range, inline_highlight_style(run.style, &self.palette)));
                div()
                    .text_color(rgb(rgb_u32(color)))
                    .child(StyledText::new(display).with_highlights(highlights))
            }))
    }

    fn render_composer(&self) -> impl IntoElement {
        let hint: SharedString = if self.turn_running {
            "running…".into()
        } else {
            "".into()
        };

        // No custom text-layout element/cursor overlay needed: splicing a
        // caret glyph directly into the displayed string lets gpui's
        // existing word-wrap and embedded-newline handling (already used
        // for the log) place it correctly even across wrapped/multiple
        // lines, at the cost of the caret being un-styled (same color as
        // the surrounding text) and not blinking.
        let (before, after) = self.composing.split_at(self.cursor);
        let display = format!("{before}│{after}");

        div()
            .flex()
            .flex_row()
            .items_start()
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
            .child(div().flex_1().child(display))
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
            .font_family(self.monospace_font.clone())
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
        AgentEvent::UserPrompt(text) => (format!("> {text}"), palette.accent),
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

/// Maps a backend-agnostic [`InlineStyle`] (from `parrot-core`'s markdown
/// pass) onto gpui's own `HighlightStyle`. Bold/Header both bold the run —
/// Header additionally recolors it to `accent`, matching the weight the
/// rest of the app already uses for "notice this" (see `render_header`'s
/// status line). Code gets no font swap (the whole app is already
/// monospace — see `resolve_monospace_font`), just a distinguishing color.
fn inline_highlight_style(style: InlineStyle, palette: &Palette) -> HighlightStyle {
    match style {
        InlineStyle::Bold => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        },
        InlineStyle::Header => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            color: Some(rgb(rgb_u32(palette.accent)).into()),
            ..Default::default()
        },
        InlineStyle::Code => HighlightStyle {
            color: Some(rgb(rgb_u32(palette.tool_call)).into()),
            ..Default::default()
        },
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

// The composer's cursor-movement arithmetic, factored out as pure
// functions over `&str` so it's testable without a `ParrotWindow`/`cx`.

fn prev_char_boundary(s: &str, pos: usize) -> usize {
    s[..pos]
        .chars()
        .next_back()
        .map_or(0, |c| pos - c.len_utf8())
}

fn next_char_boundary(s: &str, pos: usize) -> usize {
    s[pos..].chars().next().map_or(pos, |c| pos + c.len_utf8())
}

/// Byte offset of the start of the `\n`-delimited logical line containing
/// `pos`.
fn line_start(s: &str, pos: usize) -> usize {
    s[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Byte offset of the end of the `\n`-delimited logical line containing
/// `pos` (the offset of the `\n` itself, or `s.len()` on the last line).
fn line_end(s: &str, pos: usize) -> usize {
    s[pos..].find('\n').map_or(s.len(), |i| pos + i)
}

/// How many chars into its logical line `pos` is — used to preserve the
/// visual column across Up/Down.
fn column_chars(s: &str, pos: usize) -> usize {
    s[line_start(s, pos)..pos].chars().count()
}

/// The byte offset `column` chars into the line spanning
/// `line_start..line_end`, clamped to the line's length for short lines.
fn offset_at_column(s: &str, line_start: usize, line_end: usize, column: usize) -> usize {
    let line = &s[line_start..line_end];
    line.char_indices()
        .nth(column)
        .map_or(line_end, |(i, _)| line_start + i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prev_char_boundary_steps_back_one_multibyte_char() {
        assert_eq!(prev_char_boundary("hé", "hé".len()), "h".len());
    }

    #[test]
    fn prev_char_boundary_at_start_stays_zero() {
        assert_eq!(prev_char_boundary("abc", 0), 0);
    }

    #[test]
    fn next_char_boundary_steps_forward_one_multibyte_char() {
        assert_eq!(next_char_boundary("hé", "h".len()), "hé".len());
    }

    #[test]
    fn next_char_boundary_at_end_stays_put() {
        assert_eq!(next_char_boundary("abc", 3), 3);
    }

    #[test]
    fn line_start_and_end_find_current_logical_line() {
        let s = "one\ntwo\nthree";
        let pos = s.find("wo").unwrap();
        assert_eq!(line_start(s, pos), 4);
        assert_eq!(line_end(s, pos), 7);
    }

    #[test]
    fn line_start_on_first_line_is_zero() {
        assert_eq!(line_start("one\ntwo", 1), 0);
    }

    #[test]
    fn line_end_on_last_line_is_len() {
        let s = "one\ntwo";
        assert_eq!(line_end(s, 5), s.len());
    }

    #[test]
    fn column_chars_counts_from_line_start() {
        let s = "abc\nde";
        assert_eq!(column_chars(s, s.len()), 2);
    }

    #[test]
    fn offset_at_column_clamps_to_line_end_for_short_lines() {
        let s = "abcdef\nde";
        let (start, end) = (7, 9);
        assert_eq!(offset_at_column(s, start, end, 5), end);
    }

    #[test]
    fn offset_at_column_finds_exact_position() {
        let s = "abcdef";
        assert_eq!(offset_at_column(s, 0, s.len(), 2), 2);
    }
}
