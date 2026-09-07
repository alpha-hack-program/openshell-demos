// SPDX-License-Identifier: Apache-2.0

//! Shared theme/palette types.
//!
//! No UI-backend dependency on purpose: `parrot-tui` (ratatui) and
//! `parrot-gpui` (gpui) each convert [`Rgb`] into their own color type, but
//! both read the same semantic slots here so "error" or "muted" never
//! drifts between frontends.

/// Light/dark preference. Actual terminal-background *detection* for
/// `System` (OSC 11 query, `COLORFGBG`, `NO_COLOR`) is a `parrot-tui`
/// concern — it resolves to a concrete mode before rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}

/// A plain 24-bit color, independent of any rendering backend's color type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Semantic color slots shared by every parrot frontend.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    pub accent: Rgb,
    pub muted: Rgb,
    pub error: Rgb,
    pub assistant_text: Rgb,
    pub tool_call: Rgb,
    pub tool_result: Rgb,
    pub result: Rgb,
}

impl Palette {
    pub const fn dark() -> Self {
        Self {
            background: Rgb(0x1e, 0x1e, 0x2e),
            foreground: Rgb(0xcd, 0xd6, 0xf4),
            accent: Rgb(0x89, 0xb4, 0xfa),
            muted: Rgb(0x6c, 0x70, 0x86),
            error: Rgb(0xf3, 0x8b, 0xa8),
            assistant_text: Rgb(0xcd, 0xd6, 0xf4),
            tool_call: Rgb(0xf9, 0xe2, 0xaf),
            tool_result: Rgb(0xa6, 0xe3, 0xa1),
            result: Rgb(0x94, 0xe2, 0xd5),
        }
    }

    pub const fn light() -> Self {
        Self {
            background: Rgb(0xef, 0xf1, 0xf5),
            foreground: Rgb(0x4c, 0x4f, 0x69),
            accent: Rgb(0x1e, 0x66, 0xf5),
            muted: Rgb(0x8c, 0x8f, 0xa1),
            error: Rgb(0xd2, 0x02, 0x53),
            assistant_text: Rgb(0x4c, 0x4f, 0x69),
            tool_call: Rgb(0xdf, 0x8e, 0x1d),
            tool_result: Rgb(0x40, 0xa0, 0x2b),
            result: Rgb(0x17, 0x92, 0x8f),
        }
    }

    /// Resolve a mode into a concrete palette. `System` falls back to
    /// `dark` here — frontends that can actually detect the terminal/OS
    /// background should resolve `System` to `Light`/`Dark` themselves
    /// before calling this.
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark | ThemeMode::System => Self::dark(),
        }
    }
}
