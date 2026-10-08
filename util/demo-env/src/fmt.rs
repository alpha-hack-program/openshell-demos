// SPDX-License-Identifier: Apache-2.0

//! Terminal formatting: timestamps, sizes, durations, colour.

use std::sync::OnceLock;
use std::time::SystemTime;

use chrono::{DateTime, Local, SecondsFormat, Utc};

static COLOR: OnceLock<bool> = OnceLock::new();

/// Decide once whether to emit ANSI. Honours `NO_COLOR` and `--no-color`;
/// output is also plain when stdout isn't a terminal, so piping into a
/// file or a log stays readable.
pub fn init_color(forced_off: bool) {
    let on = !forced_off && std::env::var_os("NO_COLOR").is_none() && stdout_is_tty();
    let _ = COLOR.set(on);
}

fn stdout_is_tty() -> bool {
    // Avoid a dependency for one call.
    unsafe { libc_isatty(1) }
}

// `isatty` is a three-line extern rather than a crate: `demo-env` is a
// local developer tool and this is its only libc need.
unsafe fn libc_isatty(fd: i32) -> bool {
    unsafe extern "C" {
        fn isatty(fd: i32) -> i32;
    }
    unsafe { isatty(fd) == 1 }
}

fn color_on() -> bool {
    *COLOR.get().unwrap_or(&false)
}

pub fn green(s: &str) -> String {
    paint(s, "32")
}

pub fn yellow(s: &str) -> String {
    paint(s, "33")
}

pub fn red(s: &str) -> String {
    paint(s, "31")
}

pub fn cyan(s: &str) -> String {
    paint(s, "36")
}

pub fn bold(s: &str) -> String {
    paint(s, "1")
}

pub fn dim(s: &str) -> String {
    paint(s, "2")
}

fn paint(s: &str, code: &str) -> String {
    if color_on() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn rfc3339(t: SystemTime) -> String {
    DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Compact, sortable stamp for auto-generated slot names.
pub fn now_slot_stamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Render a stored RFC3339 stamp in the user's local timezone, falling
/// back to the raw string if it won't parse.
pub fn local_time(stamp: &str) -> String {
    match DateTime::parse_from_rfc3339(stamp) {
        Ok(dt) => dt
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => stamp.to_string(),
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// "2d 3h", "47m", "12s" — coarse on purpose, this is for "does this
/// expire before the demo ends", not for billing.
pub fn human_duration(seconds: i64) -> String {
    let s = seconds.abs();
    if s >= 86_400 {
        let days = s / 86_400;
        let hours = (s % 86_400) / 3_600;
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if s >= 3_600 {
        let hours = s / 3_600;
        let mins = (s % 3_600) / 60;
        if mins > 0 {
            format!("{hours}h {mins}m")
        } else {
            format!("{hours}h")
        }
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// Seconds between now and a unix timestamp; negative once it's past.
pub fn seconds_until(unix: i64) -> i64 {
    unix - Utc::now().timestamp()
}

/// Shorten `$HOME/...` to `~/...` so path columns stay narrow.
pub fn tilde(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_coarsely() {
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(600), "10m");
        assert_eq!(human_duration(3_600), "1h");
        assert_eq!(human_duration(5_400), "1h 30m");
        assert_eq!(human_duration(90_000), "1d 1h");
        assert_eq!(human_duration(172_800), "2d");
        assert_eq!(human_duration(-600), "10m");
    }

    #[test]
    fn bytes_stay_readable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
    }
}
