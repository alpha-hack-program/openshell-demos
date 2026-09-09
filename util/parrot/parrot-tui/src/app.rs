// SPDX-License-Identifier: Apache-2.0

//! Main dashboard: an interactive multi-turn session by default (type a
//! prompt, watch it stream, type the next one), or a single one-shot turn
//! when `--prompt` is given. Live-tails each turn's JSONL as it arrives.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode};
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

    if let Some(prompt) = &options.one_shot_prompt {
        run_turn(terminal, client, &ctx, prompt, &mut state, palette).await?;
        return wait_for_keypress();
    }

    let mut input = String::new();
    loop {
        terminal.draw(|frame| draw_dashboard(frame, palette, &state, sandbox, Some(&input)))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    let prompt = input.trim().to_string();
                    if prompt.is_empty() {
                        continue;
                    }
                    input.clear();
                    let keep_going =
                        run_turn(terminal, client, &ctx, &prompt, &mut state, palette).await?;
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
            }
        }

        terminal.draw(|frame| draw_dashboard(frame, palette, state, ctx.sandbox, None))?;

        if stdout_done && stderr_done && outcome_done {
            return Ok(true);
        }
    }
}

fn wait_for_keypress() -> io::Result<()> {
    loop {
        if let Event::Key(_) = event::read()? {
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
    let scroll = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(inner_height);

    let log = Paragraph::new(Text::from(lines)).scroll((scroll, 0)).block(
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
        None => Line::from("[q] quit").style(Style::default().fg(palette.muted)),
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

fn render_log_entry(entry: &LogEntry, palette: &TuiPalette, width: u16) -> Vec<Line<'static>> {
    let (prefix, text, color) = match &entry.event {
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
    };
    wrap_entry(prefix, &text, width, color)
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
}
