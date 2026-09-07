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
        self.log.push(LogEntry {
            source: StreamSource::Stdout,
            event: classify_line(line),
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
