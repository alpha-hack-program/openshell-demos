// SPDX-License-Identifier: Apache-2.0

//! Agent-specific command construction.
//!
//! End users (alice/bob/charlie) shouldn't have to know or type
//! `--mcp-config`, `--permission-mode`, `--output-format`, or `--resume` —
//! parrot knows which agent it's driving and builds the right invocation
//! for each turn itself.

use std::collections::HashMap;

/// Which agent CLI to drive inside the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
}

/// Inputs for building one turn's command line.
pub struct TurnOptions<'a> {
    pub prompt: &'a str,
    /// Path to an MCP config file inside the sandbox, if any.
    pub mcp_config: Option<&'a str>,
    /// A prior turn's session id, to continue the same conversation.
    pub session_id: Option<&'a str>,
}

impl Agent {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" | "claude-code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    /// Whether this agent can continue a prior turn via a session id.
    /// Claude Code's `--resume <session_id>` and Codex's
    /// `exec resume <thread_id> <prompt>` are both confirmed live: a
    /// second turn resuming a captured id correctly recalled a fact
    /// established in the first.
    pub fn supports_resume(&self) -> bool {
        matches!(self, Self::ClaudeCode | Self::Codex)
    }

    /// Default MCP config path for this agent in this demo's sandboxes.
    /// `None` means "don't inject an MCP flag; use whatever's already
    /// configured in the sandbox."
    pub fn default_mcp_config(&self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode => Some("/sandbox/.claude/mcp-servers.json"),
            Self::Codex => None,
        }
    }

    /// Build the full argv for one turn.
    pub fn build_turn(&self, opts: &TurnOptions<'_>) -> Vec<String> {
        match self {
            Self::ClaudeCode => {
                let mut args = vec![
                    "claude".to_string(),
                    "-p".to_string(),
                    opts.prompt.to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                    "--verbose".to_string(),
                    "--permission-mode".to_string(),
                    "bypassPermissions".to_string(),
                ];
                if let Some(mcp_config) = opts.mcp_config {
                    args.push("--mcp-config".to_string());
                    args.push(mcp_config.to_string());
                    args.push("--strict-mcp-config".to_string());
                }
                if let Some(session_id) = opts.session_id {
                    args.push("--resume".to_string());
                    args.push(session_id.to_string());
                }
                args
            }
            Self::Codex => {
                let mut args = vec!["codex".to_string(), "exec".to_string()];
                // `codex exec resume <thread_id> <prompt>` is a distinct
                // subcommand (not a flag on plain `exec`) — the session id
                // is Codex's own `thread_id`, threaded via
                // `extract_session_id`'s generic session/thread-id scan.
                match opts.session_id {
                    Some(session_id) => {
                        args.push("resume".to_string());
                        args.push(session_id.to_string());
                        args.push(opts.prompt.to_string());
                    }
                    None => args.push(opts.prompt.to_string()),
                }
                // The sandbox isn't a git repo, and Codex's own bubblewrap
                // sandbox can't create user namespaces nested inside the
                // OpenShell container — OpenShell's sandbox (network
                // policy, binary permissions, credential isolation) is
                // the real security boundary here. Confirmed live against
                // the keycloak-oidc demo's Annex A Codex recipe.
                args.push("--skip-git-repo-check".to_string());
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                args.push("--json".to_string());
                args
            }
        }
    }
}

/// Environment variables auto-detected from parrot's own process
/// environment and passed through to the sandbox exec by default, so
/// users don't have to retype e.g. `ANTHROPIC_BASE_URL`/`ANTHROPIC_MODEL`
/// on every invocation. `--env` flags still override/extend this.
const PASSTHROUGH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "OPENAI_BASE_URL",
    "OPENAI_MODEL",
];

pub fn detect_passthrough_environment() -> HashMap<String, String> {
    PASSTHROUGH_ENV_VARS
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_string(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_first_turn_has_no_resume() {
        let opts = TurnOptions {
            prompt: "hi",
            mcp_config: Some("/sandbox/.claude/mcp-servers.json"),
            session_id: None,
        };
        let args = Agent::ClaudeCode.build_turn(&opts);
        assert!(!args.iter().any(|a| a == "--resume"));
        assert!(args.iter().any(|a| a == "--mcp-config"));
    }

    #[test]
    fn codex_bypasses_its_own_inner_sandbox() {
        let opts = TurnOptions {
            prompt: "hi",
            mcp_config: None,
            session_id: None,
        };
        let args = Agent::Codex.build_turn(&opts);
        assert!(args.iter().any(|a| a == "--skip-git-repo-check"));
        assert!(args
            .iter()
            .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
        assert!(!args.iter().any(|a| a == "resume"));
    }

    #[test]
    fn codex_followup_turn_uses_resume_subcommand() {
        let opts = TurnOptions {
            prompt: "and then?",
            mcp_config: None,
            session_id: Some("01a07df3-c913-7602-b0eb-da2f407e44d5"),
        };
        let args = Agent::Codex.build_turn(&opts);
        assert_eq!(args[2], "resume");
        assert_eq!(args[3], "01a07df3-c913-7602-b0eb-da2f407e44d5");
        assert_eq!(args[4], "and then?");
    }

    #[test]
    fn claude_code_followup_turn_resumes_session() {
        let opts = TurnOptions {
            prompt: "and then?",
            mcp_config: None,
            session_id: Some("abc-123"),
        };
        let args = Agent::ClaudeCode.build_turn(&opts);
        let idx = args
            .iter()
            .position(|a| a == "--resume")
            .expect("--resume present");
        assert_eq!(args[idx + 1], "abc-123");
    }
}
