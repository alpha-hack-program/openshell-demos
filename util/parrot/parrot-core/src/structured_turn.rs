// SPDX-License-Identifier: Apache-2.0

//! A one-shot, schema-constrained agent turn: runs the same sandboxed
//! agent CLI `submit_turn` drives (see [`crate::agent::Agent::build_turn`]),
//! but collects the whole response instead of streaming it to a chat UI,
//! and parses its final assistant text as JSON -- for prompts that ask the
//! model to answer ONLY with JSON matching a given shape (meeting-brief
//! insights, client snapshots, ...) rather than free-form chat prose.
//!
//! Neither Claude Code nor Codex expose a grammar-constrained/schema
//! decoding flag (checked both CLIs' help output) -- "JSON schema" here
//! means *prompted* JSON: the caller's prompt must itself describe the
//! expected shape, and this function best-effort extracts a JSON object
//! from whatever text comes back. Always a fresh turn (no
//! `--resume`/`exec resume`): each call is independent.

use std::collections::HashMap;
use std::collections::VecDeque;

use tokio_stream::StreamExt;

use crate::agent::{Agent, TurnOptions};
use crate::client::ParrotClient;
use crate::error::{ParrotError, Result};
use crate::exec::StreamedExecOptions;
use crate::jsonl::{classify_line, tool_result_text, AgentEvent};

/// One tool call a structured turn made, FIFO-paired with its result --
/// same pairing `parrot-tauri/ui/app.js`'s `evidenceRowsFromLog` does for
/// the interactive chat log, so every evidence-panel source in the UI
/// (chat, Meeting Brief, Client Snapshot) ends up describing a call the
/// same way.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolEvidence {
    pub tool: String,
    pub detail: String,
    pub is_error: bool,
}

/// [`run_structured_turn`]'s result: the parsed JSON plus every tool call
/// the turn made getting there (empty when `mcp_config` was `None`).
pub struct StructuredTurnResult {
    pub value: serde_json::Value,
    pub evidence: Vec<ToolEvidence>,
}

/// Runs `prompt` as a fresh, one-shot turn and parses the agent's final
/// text response as JSON. `mcp_config` is passed straight to
/// [`Agent::build_turn`] -- `None` keeps the model from calling any tools
/// (pure reasoning over whatever facts the prompt already contains);
/// `Some(path)` lets it use the sandbox's configured MCP tools to go look
/// things up itself.
pub async fn run_structured_turn(
    client: &ParrotClient,
    sandbox: &str,
    workspace: Option<&str>,
    agent: Agent,
    mcp_config: Option<&str>,
    environment: HashMap<String, String>,
    prompt: &str,
) -> Result<StructuredTurnResult> {
    let turn_opts = TurnOptions {
        prompt,
        mcp_config,
        session_id: None,
        // Always true here, not just when `mcp_config` is `None`: a
        // structured-JSON turn should never wander into a built-in tool
        // (Bash, WebSearch, ...) regardless of whether it also has MCP
        // tools configured -- it's a no-op when `mcp_config` is `Some`
        // since `build_turn` already disables built-ins in that case.
        no_builtin_tools: true,
    };
    let command = agent.build_turn(&turn_opts);
    let exec_opts = StreamedExecOptions {
        environment,
        ..Default::default()
    };

    let mut exec = client
        .exec_streamed(sandbox, workspace, &command, exec_opts)
        .await?;

    let mut last_text: Option<String> = None;
    let mut pending_calls: VecDeque<String> = VecDeque::new();
    let mut evidence: Vec<ToolEvidence> = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut outcome_done = false;
    let mut crashed = false;
    let mut crash_reason = None;

    loop {
        tokio::select! {
            biased;
            line = exec.stdout.next(), if !stdout_done => match line {
                Some(line) => match classify_line(&line) {
                    AgentEvent::AssistantText(text) => last_text = Some(text),
                    AgentEvent::ToolCall { name, .. } => pending_calls.push_back(name),
                    AgentEvent::ToolResult { content, is_error } => {
                        let tool = pending_calls
                            .pop_front()
                            .unwrap_or_else(|| "(unknown tool)".to_string());
                        let detail = if is_error {
                            format!("error: {}", tool_result_text(&content))
                        } else {
                            "ok".to_string()
                        };
                        evidence.push(ToolEvidence {
                            tool,
                            detail,
                            is_error,
                        });
                    }
                    _ => {}
                },
                None => stdout_done = true,
            },
            line = exec.stderr.next(), if !stderr_done => match line {
                Some(_) => {}
                None => stderr_done = true,
            },
            outcome = &mut exec.outcome, if !outcome_done => {
                if let Ok(outcome) = outcome {
                    crashed = outcome.crashed;
                    crash_reason = outcome.crash_reason;
                }
                outcome_done = true;
            }
        }
        if stdout_done && stderr_done && outcome_done {
            break;
        }
    }

    if crashed {
        return Err(ParrotError::AgentTurn(format!(
            "agent exec stream failed: {}",
            crash_reason.unwrap_or_else(|| "unknown error".to_string())
        )));
    }

    let text = last_text
        .ok_or_else(|| ParrotError::AgentTurn("agent produced no text response".to_string()))?;
    let value = parse_json_response(&text)?;
    Ok(StructuredTurnResult { value, evidence })
}

/// Models reliably wrap JSON in a ```` ```json ```` fence (or add a
/// sentence before/after it) even when told to respond with nothing else
/// -- strip a fence if present, then fall back to the substring between
/// the first `{` and the last `}` before giving up.
fn parse_json_response(text: &str) -> Result<serde_json::Value> {
    let trimmed = text.trim();

    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }

    let fenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|s| s.rfind("```").map(|end| &s[..end]))
        .map(str::trim);
    if let Some(candidate) = fenced {
        if let Ok(value) = serde_json::from_str(candidate) {
            return Ok(value);
        }
    }

    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            if let Ok(value) = serde_json::from_str(&trimmed[start..=end]) {
                return Ok(value);
            }
        }
    }

    Err(ParrotError::AgentTurn(format!(
        "agent response wasn't valid JSON:\n{text}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json() {
        let value = parse_json_response(r#"{"a": 1}"#).unwrap();
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn parses_fenced_json() {
        let value = parse_json_response("```json\n{\"a\": 1}\n```").unwrap();
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn parses_json_with_surrounding_prose() {
        let value =
            parse_json_response("Sure, here you go:\n{\"a\": 1}\nLet me know if you need more.")
                .unwrap();
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn errors_on_no_json() {
        assert!(parse_json_response("no json here").is_err());
    }
}
