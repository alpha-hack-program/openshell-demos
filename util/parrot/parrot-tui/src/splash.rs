// SPDX-License-Identifier: Apache-2.0

//! Startup splash: wordmark, identity greeting, active gateway — dismissed
//! automatically after a short delay or on any keypress.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use figlet_rs::FIGfont;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui::Terminal;

use crate::theme_detect::TuiPalette;

const AUTO_DISMISS: Duration = Duration::from_millis(1300);
const POLL_STEP: Duration = Duration::from_millis(50);

pub struct SplashInfo<'a> {
    pub greeting: &'a str,
    pub gateway_name: &'a str,
    pub gateway_endpoint: &'a str,
}

/// Show the splash screen until `AUTO_DISMISS` elapses or the user presses
/// any key.
pub fn run_splash<B: Backend>(
    terminal: &mut Terminal<B>,
    palette: &TuiPalette,
    info: &SplashInfo<'_>,
) -> io::Result<()> {
    let wordmark = figlet_wordmark("Meridian Inc.");
    let start = Instant::now();

    loop {
        terminal.draw(|frame| draw_splash(frame, palette, &wordmark, info))?;

        let elapsed = start.elapsed();
        if elapsed >= AUTO_DISMISS {
            return Ok(());
        }
        let wait = (AUTO_DISMISS - elapsed).min(POLL_STEP);
        if event::poll(wait)? {
            if let Event::Key(_) = event::read()? {
                return Ok(());
            }
        }
    }
}

fn figlet_wordmark(text: &str) -> String {
    match FIGfont::standard() {
        Ok(font) => font
            .convert(text)
            .map(|figure| figure.to_string())
            .unwrap_or_else(|| text.to_string()),
        Err(_) => text.to_string(),
    }
}

fn draw_splash(frame: &mut Frame, palette: &TuiPalette, wordmark: &str, info: &SplashInfo<'_>) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(palette.background)),
        area,
    );

    let mut lines: Vec<Line> = wordmark
        .lines()
        .map(|l| Line::from(l.to_string()).style(Style::default().fg(palette.accent)))
        .collect();
    lines.push(Line::from(""));
    lines
        .push(Line::from(info.greeting.to_string()).style(Style::default().fg(palette.foreground)));
    lines.push(
        Line::from(format!(
            "Gateway: {} ({})",
            info.gateway_name, info.gateway_endpoint
        ))
        .style(Style::default().fg(palette.muted)),
    );

    let paragraph = Paragraph::new(Text::from(lines)).alignment(Alignment::Center);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Min(0),
            Constraint::Percentage(30),
        ])
        .split(area);

    frame.render_widget(paragraph, vertical[1]);
}
