// SPDX-License-Identifier: Apache-2.0

//! Terminal-specific theme resolution: OSC 11 background detection,
//! `NO_COLOR`/`CLICOLOR_FORCE`/`COLORTERM` handling, and degrading
//! `parrot-core`'s 24-bit [`Rgb`] palette to whatever the terminal actually
//! supports. Lives here rather than in `parrot-core` because it needs raw
//! terminal I/O.

use std::io::{IsTerminal, Read, Write};
use std::time::Duration;

use parrot_core::{Palette, Rgb, ThemeMode};
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorCapability {
    TrueColor,
    Palette256,
    Palette16,
    None,
}

/// Detect terminal color support, respecting `NO_COLOR` (disables color)
/// and `CLICOLOR_FORCE` (forces color even when stdout isn't a TTY).
pub fn detect_color_capability() -> ColorCapability {
    if std::env::var_os("NO_COLOR").is_some() {
        return ColorCapability::None;
    }
    let forced = std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0");
    if !forced && !std::io::stdout().is_terminal() {
        return ColorCapability::None;
    }
    if matches!(
        std::env::var("COLORTERM").as_deref(),
        Ok("truecolor") | Ok("24bit")
    ) {
        return ColorCapability::TrueColor;
    }
    match std::env::var("TERM").as_deref() {
        Ok("dumb") => ColorCapability::None,
        Ok(term) if term.contains("256color") => ColorCapability::Palette256,
        _ => ColorCapability::Palette16,
    }
}

/// Resolve `System` to a concrete `Light`/`Dark`: OSC 11 terminal query
/// first, then `COLORFGBG`, then default to `Dark`.
pub fn resolve_theme_mode(mode: ThemeMode) -> ThemeMode {
    match mode {
        ThemeMode::Light | ThemeMode::Dark => mode,
        ThemeMode::System => {
            if let Some(is_dark) = query_osc11_background() {
                return if is_dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                };
            }
            if let Some(is_dark) = colorfgbg_is_dark() {
                return if is_dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                };
            }
            ThemeMode::Dark
        }
    }
}

fn colorfgbg_is_dark() -> Option<bool> {
    let value = std::env::var("COLORFGBG").ok()?;
    let bg = value.split(';').next_back()?;
    let bg: u8 = bg.trim().parse().ok()?;
    // xterm convention: palette indices 0-6 are dark backgrounds, 7+ light.
    Some(bg < 8)
}

/// Query the terminal's background color via OSC 11. Best-effort: returns
/// `None` on any failure, non-TTY streams, or if the terminal doesn't
/// answer within the timeout, rather than blocking startup.
fn query_osc11_background() -> Option<bool> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return None;
    }

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        crossterm::terminal::enable_raw_mode().ok()?;
    }

    let response = read_osc11_response();

    if !was_raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }

    parse_osc11_response(&response?)
}

fn read_osc11_response() -> Option<Vec<u8>> {
    print!("\x1b]11;?\x07");
    std::io::stdout().flush().ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 64];
        let mut response = Vec::new();
        for _ in 0..8 {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    response.extend_from_slice(&buf[..n]);
                    if response.contains(&0x07) || response.windows(2).any(|w| w == [0x1b, b'\\']) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(response);
    });

    rx.recv_timeout(Duration::from_millis(200)).ok()
}

fn parse_osc11_response(bytes: &[u8]) -> Option<bool> {
    let text = String::from_utf8_lossy(bytes);
    let start = text.find("rgb:")? + 4;
    let rest = &text[start..];
    let end = rest.find(['\x07', '\x1b']).unwrap_or(rest.len());
    let mut channels = rest[..end].split('/');
    let r_raw = channels.next()?;
    let digits = r_raw.len().max(1);
    let max = (1u64 << (digits * 4)) - 1;

    let normalize = |raw: &str| -> Option<u32> {
        let value = u64::from_str_radix(raw, 16).ok()?;
        Some(((value * 255) / max) as u32)
    };

    let r = normalize(r_raw)?;
    let g = normalize(channels.next()?)?;
    let b = normalize(channels.next()?)?;

    // Perceived luminance (ITU-R BT.601); below ~50% reads as a dark background.
    let luminance = (r * 299 + g * 587 + b * 114) / 1000;
    Some(luminance < 128)
}

/// Convert a [`Rgb`] color to whatever `capability` actually supports.
pub fn ratatui_color(color: Rgb, capability: ColorCapability) -> Color {
    match capability {
        ColorCapability::None => Color::Reset,
        ColorCapability::TrueColor => Color::Rgb(color.0, color.1, color.2),
        ColorCapability::Palette256 => nearest_ansi256(color),
        ColorCapability::Palette16 => nearest_ansi16(color),
    }
}

fn nearest_ansi256(Rgb(r, g, b): Rgb) -> Color {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let to_cube = |v: u8| -> u8 {
        STEPS
            .iter()
            .enumerate()
            .min_by_key(|(_, &step)| (i32::from(step) - i32::from(v)).abs())
            .map(|(i, _)| i as u8)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (to_cube(r), to_cube(g), to_cube(b));
    Color::Indexed(16 + 36 * ri + 6 * gi + bi)
}

fn nearest_ansi16(color: Rgb) -> Color {
    const PALETTE: [(Color, (u8, u8, u8)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (205, 0, 0)),
        (Color::Green, (0, 205, 0)),
        (Color::Yellow, (205, 205, 0)),
        (Color::Blue, (0, 0, 238)),
        (Color::Magenta, (205, 0, 205)),
        (Color::Cyan, (0, 205, 205)),
        (Color::Gray, (229, 229, 229)),
        (Color::DarkGray, (127, 127, 127)),
        (Color::LightRed, (255, 0, 0)),
        (Color::LightGreen, (0, 255, 0)),
        (Color::LightYellow, (255, 255, 0)),
        (Color::LightBlue, (92, 92, 255)),
        (Color::LightMagenta, (255, 0, 255)),
        (Color::LightCyan, (0, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    let Rgb(r, g, b) = color;
    PALETTE
        .iter()
        .min_by_key(|(_, (pr, pg, pb))| {
            let dr = i32::from(*pr) - i32::from(r);
            let dg = i32::from(*pg) - i32::from(g);
            let db = i32::from(*pb) - i32::from(b);
            dr * dr + dg * dg + db * db
        })
        .map(|(color, _)| *color)
        .unwrap_or(Color::White)
}

/// [`parrot_core::Palette`], pre-converted into ratatui colors for a
/// detected [`ColorCapability`], so drawing code never touches `Rgb`
/// directly.
pub struct TuiPalette {
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    pub muted: Color,
    pub error: Color,
    pub assistant_text: Color,
    pub tool_call: Color,
    pub tool_result: Color,
    pub result: Color,
}

impl TuiPalette {
    pub fn new(palette: Palette, capability: ColorCapability) -> Self {
        Self {
            background: ratatui_color(palette.background, capability),
            foreground: ratatui_color(palette.foreground, capability),
            accent: ratatui_color(palette.accent, capability),
            muted: ratatui_color(palette.muted, capability),
            error: ratatui_color(palette.error, capability),
            assistant_text: ratatui_color(palette.assistant_text, capability),
            tool_call: ratatui_color(palette.tool_call, capability),
            tool_result: ratatui_color(palette.tool_result, capability),
            result: ratatui_color(palette.result, capability),
        }
    }
}
