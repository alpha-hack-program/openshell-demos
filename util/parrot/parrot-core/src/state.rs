// SPDX-License-Identifier: Apache-2.0

//! View-model layer both frontends render from, so classification and
//! run-status logic lives once instead of being re-derived in `parrot-tui`
//! and `parrot-gpui` separately.

use crate::exec::ExecOutcome;
use crate::jsonl::{classify_line, extract_session_id, AgentEvent};

/// One rendered log line, tagged with which stream it came from and its
/// classification.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub source: StreamSource,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSource {
    Stdout,
    Stderr,
}

/// How the running exec has progressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunStatus {
    #[default]
    Running,
    Completed {
        exit_code: i32,
    },
    Crashed,
}

/// Aggregated state across one parrot session, which may span multiple
/// turns — `log` and `session_id` accumulate across turns; `status`
/// reflects the most recent one.
#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub log: Vec<LogEntry>,
    pub status: RunStatus,
    /// The agent's own conversation-session id, once known — extracted
    /// opportunistically from the stream so the next turn can pass it to
    /// e.g. Claude Code's `--resume` and continue the same conversation.
    pub session_id: Option<String>,
}

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset per-turn status before starting a new turn. Keeps the
    /// transcript (`log`) and `session_id` so multi-turn conversations
    /// accumulate instead of restarting.
    pub fn begin_turn(&mut self) {
        self.status = RunStatus::Running;
    }

    pub fn push_stdout(&mut self, line: &str) {
        if let Some(session_id) = extract_session_id(line) {
            self.session_id = Some(session_id);
        }
        let event = classify_line(line);
        // `ThinkingTokens` is a running counter re-sent every few tokens
        // (hundreds to thousands of events per turn) — update the existing
        // counter line in place instead of appending one log entry per
        // event, or it drowns out the actual `Thinking`/`AssistantText`/
        // `ToolCall` events in between.
        if matches!(event, AgentEvent::ThinkingTokens(_)) {
            if let Some(last) = self.log.last_mut() {
                if matches!(last.event, AgentEvent::ThinkingTokens(_)) {
                    last.event = event;
                    return;
                }
            }
        }
        self.log.push(LogEntry {
            source: StreamSource::Stdout,
            event,
        });
    }

    pub fn push_stderr(&mut self, line: &str) {
        self.log.push(LogEntry {
            source: StreamSource::Stderr,
            event: AgentEvent::Unparseable(line.to_string()),
        });
    }

    pub fn finish(&mut self, outcome: &ExecOutcome) {
        self.status = if outcome.crashed {
            RunStatus::Crashed
        } else {
            RunStatus::Completed {
                exit_code: outcome.exit_code.unwrap_or(-1),
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thinking_tokens_line(estimated_tokens: u64) -> String {
        format!(
            r#"{{"type":"system","subtype":"thinking_tokens","estimated_tokens":{estimated_tokens},"estimated_tokens_delta":1}}"#
        )
    }

    #[test]
    fn coalesces_consecutive_thinking_tokens_into_one_log_entry() {
        let mut state = SessionState::new();
        for tokens in [1, 2, 3, 1000, 1004] {
            state.push_stdout(&thinking_tokens_line(tokens));
        }
        assert_eq!(state.log.len(), 1);
        assert!(matches!(
            state.log[0].event,
            AgentEvent::ThinkingTokens(1004)
        ));
    }

    #[test]
    fn thinking_tokens_does_not_coalesce_across_other_events() {
        let mut state = SessionState::new();
        state.push_stdout(&thinking_tokens_line(1));
        state.push_stdout(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#);
        state.push_stdout(&thinking_tokens_line(2));
        assert_eq!(state.log.len(), 3);
    }
}
