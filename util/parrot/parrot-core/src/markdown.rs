// SPDX-License-Identifier: Apache-2.0

//! A deliberately small, non-CommonMark-spec-compliant markdown-to-styled-
//! runs pass for streamed agent text — headers, bullets, inline bold, and
//! inline code only, nothing else (no links, tables, nested emphasis,
//! block quotes). Backend-agnostic (no gpui/ratatui dependency, matching
//! `theme.rs`'s own reason for staying backend-agnostic) — callers map
//! [`InlineStyle`] onto their own styling primitives.
//!
//! Deliberately not built on a real CommonMark parser: agent text arrives
//! streamed, a token/chunk at a time, and a real parser building a
//! document tree tends to mis-render or flicker on a still-open marker
//! (an unterminated code fence, a lone `**`) that hasn't closed yet. This
//! pass is line-oriented and only recognizes a *closed* marker as styling
//! at all — an unclosed one is left as literal, unstyled text, which is
//! always well-formed by construction.

use std::ops::Range;

/// How one run of text (see [`InlineRun`]) should be styled. Deliberately
/// small — no italic, strikethrough, or link support; add variants here,
/// not a different mechanism, if the scope grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineStyle {
    /// A `#`..`######` header line — the rest of that line, marker
    /// stripped.
    Header,
    /// `**bold**` or `__bold__`, marker stripped.
    Bold,
    /// `` `code` ``, marker stripped.
    Code,
}

/// One styled run — a byte range into the string [`render_inline_markdown`]
/// returned (not the original input, which is a different length once
/// markers are stripped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRun {
    pub range: Range<usize>,
    pub style: InlineStyle,
}

/// Rewrites `text` into a display string with markdown markers stripped,
/// plus the runs that should be styled. See the module doc for exactly
/// what's recognized.
pub fn render_inline_markdown(text: &str) -> (String, Vec<InlineRun>) {
    let mut out = String::with_capacity(text.len());
    let mut runs = Vec::new();

    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        push_line(line, &mut out, &mut runs);
    }

    (out, runs)
}

/// Handles one line's leading header/bullet marker (if any), then hands
/// the remainder to [`scan_inline`].
fn push_line(line: &str, out: &mut String, runs: &mut Vec<InlineRun>) {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        if let Some(rest) = trimmed[hashes..].strip_prefix(' ') {
            out.push_str(indent);
            let start = out.len();
            scan_inline(rest, out, runs);
            runs.push(InlineRun {
                range: start..out.len(),
                style: InlineStyle::Header,
            });
            return;
        }
    }

    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        out.push_str(indent);
        out.push_str("• ");
        scan_inline(rest, out, runs);
        return;
    }

    out.push_str(indent);
    scan_inline(trimmed, out, runs);
}

/// Scans one line (already stripped of any header/bullet marker) for
/// `**bold**`/`__bold__` and `` `code` ``, appending the destyled text to
/// `out` and recording runs. Not recursive — a bold span is not itself
/// scanned for inline code — matching this pass's cheap, non-spec-
/// compliant scope.
fn scan_inline(line: &str, out: &mut String, runs: &mut Vec<InlineRun>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((marker, marker_len)) = match_marker(bytes, i) {
            if let Some(close) = line[i + marker_len..].find(marker) {
                let close = i + marker_len + close;
                let inner = &line[i + marker_len..close];
                let start = out.len();
                out.push_str(inner);
                runs.push(InlineRun {
                    range: start..out.len(),
                    style: if marker == "`" {
                        InlineStyle::Code
                    } else {
                        InlineStyle::Bold
                    },
                });
                i = close + marker_len;
                continue;
            }
        }
        // Not a recognized/closed marker — copy one char literally,
        // unstyled. Covers both "not a marker at all" and "an opening
        // marker with no closing pair yet" (still-streaming text).
        let ch = line[i..]
            .chars()
            .next()
            .expect("i < bytes.len() implies a char starts here");
        out.push(ch);
        i += ch.len_utf8();
    }
}

/// Recognizes `**`, `__`, or `` ` `` starting at byte offset `i`.
fn match_marker(bytes: &[u8], i: usize) -> Option<(&'static str, usize)> {
    if bytes[i..].starts_with(b"**") {
        Some(("**", 2))
    } else if bytes[i..].starts_with(b"__") {
        Some(("__", 2))
    } else if bytes[i..].starts_with(b"`") {
        Some(("`", 1))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged_with_no_runs() {
        let (text, runs) = render_inline_markdown("just plain text");
        assert_eq!(text, "just plain text");
        assert!(runs.is_empty());
    }

    #[test]
    fn strips_bold_and_records_a_run() {
        let (text, runs) = render_inline_markdown("say **hello** now");
        assert_eq!(text, "say hello now");
        assert_eq!(
            runs,
            vec![InlineRun {
                range: 4..9,
                style: InlineStyle::Bold,
            }]
        );
    }

    #[test]
    fn strips_underscore_bold_too() {
        let (text, runs) = render_inline_markdown("__bold__");
        assert_eq!(text, "bold");
        assert_eq!(runs[0].style, InlineStyle::Bold);
    }

    #[test]
    fn strips_inline_code_and_records_a_run() {
        let (text, runs) = render_inline_markdown("run `cargo test` please");
        assert_eq!(text, "run cargo test please");
        assert_eq!(
            runs,
            vec![InlineRun {
                range: 4..14,
                style: InlineStyle::Code,
            }]
        );
    }

    #[test]
    fn header_line_strips_marker_and_styles_whole_line() {
        let (text, runs) = render_inline_markdown("# Title here\nbody");
        assert_eq!(text, "Title here\nbody");
        assert_eq!(
            runs,
            vec![InlineRun {
                range: 0..10,
                style: InlineStyle::Header,
            }]
        );
    }

    #[test]
    fn bullet_line_normalizes_marker_with_no_style_run() {
        let (text, runs) = render_inline_markdown("- one\n* two\n+ three");
        assert_eq!(text, "• one\n• two\n• three");
        assert!(runs.is_empty());
    }

    #[test]
    fn unclosed_marker_is_left_literal_and_unstyled() {
        let (text, runs) = render_inline_markdown("still **typing");
        assert_eq!(text, "still **typing");
        assert!(runs.is_empty());
    }

    #[test]
    fn multiple_runs_on_one_line_both_recorded() {
        let (text, runs) = render_inline_markdown("**a** and `b`");
        assert_eq!(text, "a and b");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].style, InlineStyle::Bold);
        assert_eq!(runs[1].style, InlineStyle::Code);
    }

    #[test]
    fn seven_hashes_is_not_a_header() {
        let (text, runs) = render_inline_markdown("####### not a header");
        assert_eq!(text, "####### not a header");
        assert!(runs.is_empty());
    }
}
