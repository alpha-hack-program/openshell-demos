// SPDX-License-Identifier: Apache-2.0

//! Main dashboard: an interactive multi-turn session by default (type a
//! prompt, watch it stream, type the next one), or a single one-shot turn
//! when `--prompt` is given. Live-tails each turn's JSONL as it arrives.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use parrot_core::{
    tool_result_text, Agent, AgentEvent, LogEntry, ParrotClient, RunStatus, SessionState,
    StreamSource, StreamedExecOptions, TurnOptions,
};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use ratatui::Terminal;
use tokio_stream::StreamExt;

use crate::theme_detect::TuiPalette;

const TICK: Duration = Duration::from_millis(50);

/// Everything needed to run turns, independent of one-shot vs interactive.
pub struct RunOptions {
    pub agent: Agent,
    /// Workspace the sandbox lives in — `None` falls back to the SDK's
    /// unscoped ("default workspace") sandbox lookup.
    pub workspace: Option<String>,
    pub mcp_config: Option<String>,
    pub environment: HashMap<String, String>,
    /// `Some(prompt)` runs exactly that one turn then exits. `None` opens
    /// the interactive multi-turn input loop.
    pub one_shot_prompt: Option<String>,
}

pub async fn run_dashboard<B: Backend>(
    terminal: &mut Terminal<B>,
    client: &ParrotClient,
    sandbox: &str,
    options: RunOptions,
    palette: &TuiPalette,
) -> io::Result<()> {
    let mut state = SessionState::new();
    let ctx = TurnContext {
        sandbox,
        workspace: options.workspace.as_deref(),
        agent: options.agent,
        mcp_config: options.mcp_config.as_deref(),
        environment: &options.environment,
    };

    let mut view = LogView::default();

    if let Some(prompt) = &options.one_shot_prompt {
        run_turn(
            terminal, client, &ctx, prompt, &mut state, palette, &mut view,
        )
        .await?;
        return wait_for_keypress(&mut state, palette, &mut view, terminal, sandbox);
    }

    let mut input = String::new();
    loop {
        terminal.draw(|frame| {
            draw_dashboard(
                frame,
                palette,
                &state,
                sandbox,
                Some(&input),
                &mut view.scroll,
            )
        })?;

        if let Event::Key(key) = event::read()? {
            if is_copy_log_shortcut(&key) {
                copy_log_to_clipboard(&mut state, palette, &mut view.clipboard);
                continue;
            }
            if handle_scroll_key(key.code, &mut view.scroll) {
                continue;
            }
            match key.code {
                KeyCode::Enter => {
                    let prompt = input.trim().to_string();
                    if prompt.is_empty() {
                        continue;
                    }
                    input.clear();
                    let keep_going = run_turn(
                        terminal, client, &ctx, &prompt, &mut state, palette, &mut view,
                    )
                    .await?;
                    if !keep_going {
                        return Ok(());
                    }
                }
                KeyCode::Esc => return Ok(()),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
        }
    }
}

/// Log-pane UI state that outlives any single turn: how far the user has
/// scrolled up from the tail, and the clipboard handle `Ctrl+Y` copies into.
///
/// `clipboard` is `None` until the first copy, then kept open for the rest
/// of the session — see the comment on `copy_log_to_clipboard` for why.
#[derive(Default)]
struct LogView {
    scroll: u16,
    clipboard: Option<arboard::Clipboard>,
}

/// Turn-invariant settings, bundled so `run_turn` doesn't grow a parameter
/// per new option.
struct TurnContext<'a> {
    sandbox: &'a str,
    workspace: Option<&'a str>,
    agent: Agent,
    mcp_config: Option<&'a str>,
    environment: &'a HashMap<String, String>,
}

/// Run a single turn to completion, streaming and rendering its output.
/// Returns `Ok(false)` if the user quit mid-turn (`q`/`Esc`), `Ok(true)`
/// to keep the session going (return to the prompt, or exit for one-shot).
async fn run_turn<B: Backend>(
    terminal: &mut Terminal<B>,
    client: &ParrotClient,
    ctx: &TurnContext<'_>,
    prompt: &str,
    state: &mut SessionState,
    palette: &TuiPalette,
    view: &mut LogView,
) -> io::Result<bool> {
    let turn_opts = TurnOptions {
        prompt,
        mcp_config: ctx.mcp_config.or_else(|| ctx.agent.default_mcp_config()),
        session_id: if ctx.agent.supports_resume() {
            state.session_id.as_deref()
        } else {
            None
        },
    };
    let command = ctx.agent.build_turn(&turn_opts);
    let exec_opts = StreamedExecOptions {
        environment: ctx.environment.clone(),
        ..Default::default()
    };

    state.begin_turn();
    state.push_user_prompt(prompt);

    let mut exec = match client
        .exec_streamed(ctx.sandbox, ctx.workspace, &command, exec_opts)
        .await
    {
        Ok(exec) => exec,
        Err(err) => {
            state.log.push(LogEntry {
                source: StreamSource::Stderr,
                event: AgentEvent::Error(err.to_string()),
            });
            state.status = RunStatus::Crashed;
            return Ok(true);
        }
    };

    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut outcome_done = false;
    let mut ticker = tokio::time::interval(TICK);

    loop {
        tokio::select! {
            biased;
            maybe_line = exec.stdout.next(), if !stdout_done => {
                match maybe_line {
                    Some(line) => state.push_stdout(&line),
                    None => stdout_done = true,
                }
            }
            maybe_line = exec.stderr.next(), if !stderr_done => {
                match maybe_line {
                    Some(line) => state.push_stderr(&line),
                    None => stderr_done = true,
                }
            }
            outcome = &mut exec.outcome, if !outcome_done => {
                if let Ok(outcome) = outcome {
                    state.finish(&outcome);
                }
                outcome_done = true;
            }
            _ = ticker.tick() => {}
        }

        if event::poll(Duration::from_millis(0))? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    return Ok(false);
                }
                if is_copy_log_shortcut(&key) {
                    copy_log_to_clipboard(state, palette, &mut view.clipboard);
                } else {
                    handle_scroll_key(key.code, &mut view.scroll);
                }
            }
        }

        terminal.draw(|frame| {
            draw_dashboard(frame, palette, state, ctx.sandbox, None, &mut view.scroll)
        })?;

        if stdout_done && stderr_done && outcome_done {
            return Ok(true);
        }
    }
}

/// After a one-shot turn finishes, let the user scroll the transcript and
/// copy it before exiting on the first key that isn't a scroll/copy command.
fn wait_for_keypress<B: Backend>(
    state: &mut SessionState,
    palette: &TuiPalette,
    view: &mut LogView,
    terminal: &mut Terminal<B>,
    sandbox: &str,
) -> io::Result<()> {
    loop {
        terminal
            .draw(|frame| draw_dashboard(frame, palette, state, sandbox, None, &mut view.scroll))?;
        if let Event::Key(key) = event::read()? {
            if is_copy_log_shortcut(&key) {
                copy_log_to_clipboard(state, palette, &mut view.clipboard);
                continue;
            }
            if handle_scroll_key(key.code, &mut view.scroll) {
                continue;
            }
            return Ok(());
        }
    }
}

fn draw_dashboard(
    frame: &mut Frame,
    palette: &TuiPalette,
    state: &SessionState,
    sandbox: &str,
    composing_input: Option<&str>,
    scroll_from_bottom: &mut u16,
) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(palette.background)),
        area,
    );
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(header_line(palette, state, sandbox), chunks[0]);

    // Inner width/height available for text once the block's border is
    // subtracted, so wrapping and scroll-to-bottom line up with what's
    // actually drawn.
    let inner_width = chunks[1].width.saturating_sub(2);
    let inner_height = chunks[1].height.saturating_sub(2);

    let lines: Vec<Line<'static>> = state
        .log
        .iter()
        .flat_map(|entry| render_log_entry(entry, palette, inner_width))
        .collect();

    // `scroll_from_bottom` counts lines scrolled up from the tail; 0 always
    // means "following the latest output". Clamped here (not at the point
    // the user pressed a key) since the max depends on the log length and
    // viewport height, both of which are only known once we're drawing.
    let total_lines = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let max_offset = total_lines.saturating_sub(inner_height);
    *scroll_from_bottom = (*scroll_from_bottom).min(max_offset);
    let scroll_top = max_offset - *scroll_from_bottom;

    let log = Paragraph::new(Text::from(lines))
        .scroll((scroll_top, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("agent output")
                .border_style(Style::default().fg(palette.muted)),
        );
    frame.render_widget(log, chunks[1]);

    let footer = match composing_input {
        Some(input) => Line::from(vec![
            Span::styled("> ", Style::default().fg(palette.accent)),
            Span::styled(input.to_string(), Style::default().fg(palette.foreground)),
        ]),
        None => Line::from("[q] quit  [\u{2191}/\u{2193}/PgUp/PgDn/Home/End] scroll  [^Y] copy")
            .style(Style::default().fg(palette.muted)),
    };
    frame.render_widget(Paragraph::new(footer), chunks[2]);
}

fn header_line<'a>(palette: &TuiPalette, state: &SessionState, sandbox: &'a str) -> Paragraph<'a> {
    let (status_text, status_color): (String, Color) = match state.status {
        RunStatus::Running => ("running".to_string(), palette.accent),
        RunStatus::Completed { exit_code: 0 } => ("completed".to_string(), palette.tool_result),
        RunStatus::Completed { exit_code } => {
            (format!("completed (exit {exit_code})"), palette.error)
        }
        RunStatus::Crashed => ("crashed".to_string(), palette.error),
    };
    let line = Line::from(vec![
        Span::styled(
            format!("parrot — sandbox {sandbox} — "),
            Style::default().fg(palette.foreground),
        ),
        Span::styled(status_text, Style::default().fg(status_color)),
    ]);
    Paragraph::new(line)
}

/// Move the log viewport by `code`, if it's a recognized scroll key.
/// Returns `false` (and leaves `scroll_from_bottom` untouched) for any
/// other key, so callers can fall through to their own handling.
fn handle_scroll_key(code: KeyCode, scroll_from_bottom: &mut u16) -> bool {
    const PAGE: u16 = 10;
    match code {
        KeyCode::Up => *scroll_from_bottom = scroll_from_bottom.saturating_add(1),
        KeyCode::Down => *scroll_from_bottom = scroll_from_bottom.saturating_sub(1),
        KeyCode::PageUp => *scroll_from_bottom = scroll_from_bottom.saturating_add(PAGE),
        KeyCode::PageDown => *scroll_from_bottom = scroll_from_bottom.saturating_sub(PAGE),
        // Clamped against the real max in `draw_dashboard`, once the log
        // length is known.
        KeyCode::Home => *scroll_from_bottom = u16::MAX,
        KeyCode::End => *scroll_from_bottom = 0,
        _ => return false,
    }
    true
}

fn is_copy_log_shortcut(key: &crossterm::event::KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('y')
}

/// Copies the whole transcript so far (not just what's currently visible)
/// to the system clipboard as plain text, and drops a status line in the
/// log reporting success or failure.
///
/// Uses `arboard` (the native OS clipboard) rather than the OSC 52 terminal
/// escape sequence: OSC 52 needs no extra dependency, but is unsupported in
/// GNOME Terminal/VTE-based terminals and in stock macOS Terminal.app.
/// `arboard` talks to the OS clipboard directly and works out of the box on
/// macOS, on Linux/X11, and on Linux/Wayland desktops via the XWayland
/// fallback that GNOME/KDE ship by default.
///
/// `clipboard` is reused across calls and kept alive for the rest of the
/// session (not opened-and-dropped per call): on X11, and on Wayland's
/// data-control protocol, the process that sets the selection has to stay
/// around to answer paste requests, since neither has OS-level clipboard
/// storage of its own. Dropping the handle right after `set_text` meant
/// nothing was left to answer a paste even moments later while parrot was
/// still running.
fn copy_log_to_clipboard(
    state: &mut SessionState,
    palette: &TuiPalette,
    clipboard: &mut Option<arboard::Clipboard>,
) {
    let text = state
        .log
        .iter()
        .map(|entry| plain_log_line(entry, palette))
        .collect::<Vec<_>>()
        .join("\n");
    let line_count = state.log.len();

    let result: Result<(), arboard::Error> = (|| {
        if clipboard.is_none() {
            *clipboard = Some(arboard::Clipboard::new()?);
        }
        clipboard
            .as_mut()
            .expect("just initialized above")
            .set_text(text)
    })();

    let message = match result {
        Ok(()) => format!("copied {line_count} log entries to clipboard"),
        Err(err) => format!("clipboard error: {err}"),
    };
    state.log.push(LogEntry {
        source: StreamSource::Stdout,
        event: AgentEvent::Info(message),
    });
}

fn plain_log_line(entry: &LogEntry, palette: &TuiPalette) -> String {
    let (prefix, text, _color) = classify_entry(entry, palette);
    format!("{prefix}{text}")
}

fn render_log_entry(entry: &LogEntry, palette: &TuiPalette, width: u16) -> Vec<Line<'static>> {
    let (prefix, text, color) = classify_entry(entry, palette);
    wrap_entry(prefix, &text, width, color)
}

fn classify_entry(entry: &LogEntry, palette: &TuiPalette) -> (&'static str, String, Color) {
    match &entry.event {
        AgentEvent::UserPrompt(text) => ("> ", text.clone(), palette.accent),
        AgentEvent::AssistantText(text) => ("» ", text.clone(), palette.assistant_text),
        AgentEvent::ToolCall { name, .. } => {
            ("→ ", format!("tool call: {name}"), palette.tool_call)
        }
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
            ("✗ ", text, palette.error)
        }
        AgentEvent::ToolResult {
            is_error: false, ..
        } => ("✓ ", "tool result".to_string(), palette.tool_result),
        AgentEvent::Thinking(text) => ("… ", text.clone(), palette.muted),
        AgentEvent::ThinkingTokens(estimated_tokens) => (
            "… ",
            format!("thinking… ({estimated_tokens} tokens)"),
            palette.muted,
        ),
        AgentEvent::Result(_) => ("● ", "final result".to_string(), palette.result),
        AgentEvent::Error(message) => ("! ", message.clone(), palette.error),
        AgentEvent::Info(message) => ("ℹ ", message.clone(), palette.muted),
        AgentEvent::Unknown(value) => (
            "· ",
            format!("unrecognized event: {}", unknown_label(value)),
            palette.muted,
        ),
        AgentEvent::Unparseable(line) => ("  ", line.clone(), palette.muted),
    }
}

/// Best-effort label for a genuinely unclassified event, so the fallback
/// itself is informative rather than a dead end — pulls whatever `type`
/// (and `subtype`, if present) the JSON carries.
fn unknown_label(value: &serde_json::Value) -> String {
    let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("?");
    // Claude Code nests its discriminator as a sibling `subtype`; Codex
    // nests it one level down as `item.type` (e.g. `item.completed` with
    // an `item` of type `mcp_tool_call`). Check both.
    let nested = value
        .get("subtype")
        .or_else(|| value.get("item").and_then(|item| item.get("type")))
        .and_then(|v| v.as_str());
    match nested {
        Some(nested) => format!("{ty}/{nested}"),
        None => ty.to_string(),
    }
}

/// Word-wrap `text` to `width` columns, prefixing the first output line
/// with `prefix` and indenting every other line (both from embedded `\n`s
/// and from wrapping) to align under it.
fn wrap_entry(prefix: &str, text: &str, width: u16, color: Color) -> Vec<Line<'static>> {
    let prefix_width = prefix.chars().count();
    let content_width = usize::from(width).saturating_sub(prefix_width).max(1);
    let indent = " ".repeat(prefix_width);

    let mut rows: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        rows.extend(wrap_paragraph(paragraph, content_width));
    }

    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            let content = if i == 0 {
                format!("{prefix}{row}")
            } else {
                format!("{indent}{row}")
            };
            Line::from(content).style(Style::default().fg(color))
        })
        .collect()
}

fn wrap_paragraph(paragraph: &str, width: usize) -> Vec<String> {
    if paragraph.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in paragraph.split(' ') {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    lines.push(current);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wrap_entry_splits_embedded_newlines_into_separate_lines() {
        let out = wrap_entry("» ", "line one\nline two", 80, Color::White);
        assert_eq!(plain(&out), vec!["» line one", "  line two"]);
    }

    #[test]
    fn wrap_entry_word_wraps_long_paragraphs() {
        let out = wrap_entry("» ", "aaaa bbbb cccc", 10, Color::White);
        assert_eq!(plain(&out), vec!["» aaaa", "  bbbb", "  cccc"]);
    }

    #[test]
    fn unknown_label_includes_subtype_when_present() {
        let value = serde_json::json!({"type": "system", "subtype": "compact_boundary"});
        assert_eq!(unknown_label(&value), "system/compact_boundary");
    }

    #[test]
    fn unknown_label_falls_back_to_type_only() {
        let value = serde_json::json!({"type": "system"});
        assert_eq!(unknown_label(&value), "system");
    }

    #[test]
    fn unknown_label_includes_nested_codex_item_type() {
        let value = serde_json::json!({
            "type": "item.completed",
            "item": {"id": "item_2", "type": "mcp_tool_call", "server": "mcp-portfolio"}
        });
        assert_eq!(unknown_label(&value), "item.completed/mcp_tool_call");
    }

    fn test_palette() -> TuiPalette {
        TuiPalette {
            background: Color::Black,
            foreground: Color::White,
            accent: Color::White,
            muted: Color::White,
            error: Color::White,
            assistant_text: Color::White,
            tool_call: Color::White,
            tool_result: Color::White,
            result: Color::White,
        }
    }

    /// Every visible cell's symbol, concatenated — enough to check whether
    /// a given line of text is anywhere on screen without caring exactly
    /// where.
    fn rendered_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer.content().iter().map(|cell| cell.symbol()).collect()
    }

    fn log_with_numbered_lines(count: usize) -> SessionState {
        let mut state = SessionState::new();
        for i in 0..count {
            state.push_user_prompt(&format!("line-{i:03}"));
        }
        state
    }

    #[test]
    fn draw_dashboard_follows_the_tail_by_default() {
        let palette = test_palette();
        let state = log_with_numbered_lines(50);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut scroll: u16 = 0;

        terminal
            .draw(|frame| draw_dashboard(frame, &palette, &state, "sandbox", None, &mut scroll))
            .unwrap();

        let text = rendered_text(terminal.backend().buffer());
        assert!(text.contains("line-049"), "tail line missing:\n{text}");
        assert!(!text.contains("line-000"), "unexpected head line:\n{text}");
    }

    #[test]
    fn scroll_up_reveals_older_lines_and_end_returns_to_the_tail() {
        let palette = test_palette();
        let state = log_with_numbered_lines(50);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut scroll: u16 = 0;

        // Up doesn't move far enough on its own to change what's visible in
        // a 10-line viewport with wrapped single-line entries, so scroll by
        // a full page to make the shift unambiguous.
        handle_scroll_key(KeyCode::PageUp, &mut scroll);
        terminal
            .draw(|frame| draw_dashboard(frame, &palette, &state, "sandbox", None, &mut scroll))
            .unwrap();
        let scrolled = rendered_text(terminal.backend().buffer());
        assert!(
            !scrolled.contains("line-049"),
            "tail still visible after scrolling up:\n{scrolled}"
        );

        handle_scroll_key(KeyCode::End, &mut scroll);
        terminal
            .draw(|frame| draw_dashboard(frame, &palette, &state, "sandbox", None, &mut scroll))
            .unwrap();
        let followed = rendered_text(terminal.backend().buffer());
        assert!(
            followed.contains("line-049"),
            "End didn't return to the tail:\n{followed}"
        );
    }

    #[test]
    fn scroll_up_is_clamped_at_the_top_of_the_log() {
        let mut scroll: u16 = 0;
        handle_scroll_key(KeyCode::Home, &mut scroll);
        assert_eq!(scroll, u16::MAX, "Home requests max scroll pending clamp");

        let palette = test_palette();
        let state = log_with_numbered_lines(50);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| draw_dashboard(frame, &palette, &state, "sandbox", None, &mut scroll))
            .unwrap();

        // Clamped to the log's actual top instead of panicking or
        // underflowing the scroll-top subtraction in draw_dashboard.
        let text = rendered_text(terminal.backend().buffer());
        assert!(
            text.contains("line-000"),
            "top line missing after clamp:\n{text}"
        );
    }

    #[test]
    fn handle_scroll_key_ignores_non_scroll_keys() {
        let mut scroll: u16 = 3;
        assert!(!handle_scroll_key(KeyCode::Char('y'), &mut scroll));
        assert_eq!(scroll, 3);
    }
}
