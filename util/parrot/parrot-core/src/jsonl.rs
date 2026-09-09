// SPDX-License-Identifier: Apache-2.0

//! Classification of one JSONL line from a streaming agent CLI's stdout
//! (Claude Code's `--output-format stream-json`, Codex's `exec --json`).

use serde_json::Value;

/// A classified line from an agent's streaming JSON stdout.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Free-form assistant text.
    AssistantText(String),
    /// The assistant invoked a tool.
    ToolCall { name: String, input: Value },
    /// A tool finished and returned output.
    ToolResult { content: Value, is_error: bool },
    /// Extended-thinking content, when the model surfaces its reasoning.
    Thinking(String),
    /// A live estimated-token-count heartbeat Claude Code emits while the
    /// model is still generating a thinking block (one event per few
    /// tokens — hundreds to thousands per turn) — a progress counter, not
    /// content. Distinct from `Thinking`, which carries the actual
    /// reasoning text once the block completes.
    ThinkingTokens(u64),
    /// The turn's final summary/result object.
    Result(Value),
    /// An error surfaced by the agent process itself (not a transport error).
    Error(String),
    /// Informational/status event (e.g. Claude Code's `system` messages —
    /// turn init, compaction boundaries, etc.) — not actionable, but not
    /// unrecognized either.
    Info(String),
    /// Recognized JSON, but not a shape parrot classifies yet.
    Unknown(Value),
    /// Not JSON at all — some agent CLIs interleave plain diagnostic lines
    /// even in JSON-output modes.
    Unparseable(String),
}

/// Classify one line of stdout from a streaming agent CLI.
///
/// Tries the Claude Code `stream-json` envelope first, then Codex's
/// `exec --json` envelope (see [`classify_codex`] — verified against a
/// live run, but only partially: some item types are still unconfirmed).
pub fn classify_line(line: &str) -> AgentEvent {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return AgentEvent::Unparseable(String::new());
    }

    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return AgentEvent::Unparseable(line.to_string()),
    };

    if let Some(event) = classify_claude_code(&value) {
        return event;
    }
    if let Some(event) = classify_codex(&value) {
        return event;
    }
    AgentEvent::Unknown(value)
}

fn classify_claude_code(value: &Value) -> Option<AgentEvent> {
    let ty = value.get("type")?.as_str()?;
    let event = match ty {
        "assistant" => {
            claude_code_assistant_block(value).unwrap_or_else(|| AgentEvent::Unknown(value.clone()))
        }
        "user" => {
            claude_code_tool_result(value).unwrap_or_else(|| AgentEvent::Unknown(value.clone()))
        }
        "result" => AgentEvent::Result(value.clone()),
        "error" => AgentEvent::Error(
            value
                .get("error")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string(),
        ),
        "system" => {
            let subtype = value
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or("system");
            if subtype == "thinking_tokens" {
                let estimated_tokens = value
                    .get("estimated_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                AgentEvent::ThinkingTokens(estimated_tokens)
            } else {
                AgentEvent::Info(format!("system: {subtype}"))
            }
        }
        _ => return None,
    };
    Some(event)
}

fn claude_code_assistant_block(value: &Value) -> Option<AgentEvent> {
    let content = value.get("message")?.get("content")?.as_array()?;
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = block.get("text").and_then(Value::as_str)?;
                return Some(AgentEvent::AssistantText(text.to_string()));
            }
            Some("thinking") => {
                let thinking = block.get("thinking").and_then(Value::as_str)?;
                return Some(AgentEvent::Thinking(thinking.to_string()));
            }
            Some("tool_use") => {
                let name = block.get("name").and_then(Value::as_str)?.to_string();
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                return Some(AgentEvent::ToolCall { name, input });
            }
            _ => continue,
        }
    }
    None
}

fn claude_code_tool_result(value: &Value) -> Option<AgentEvent> {
    let content = value.get("message")?.get("content")?.as_array()?;
    let block = content.first()?;
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let is_error = block
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let content = block.get("content").cloned().unwrap_or(Value::Null);
    Some(AgentEvent::ToolResult { content, is_error })
}

/// Classifier for Codex's `exec --json` event stream (verified against
/// live `codex exec --json` 0.146.0 runs — an item-based envelope, not the
/// `{"msg":{"type":...}}` shape an earlier version of this function
/// guessed at): `{"type":"thread.started","thread_id":...}`,
/// `{"type":"turn.started"}`,
/// `{"type":"item.started"|"item.updated"|"item.completed","item":{"type":...,...}}`
/// (`item.updated` fires for in-place mutations like a todo list gaining a
/// checked-off item, distinct from `started`/`completed`),
/// `{"type":"turn.completed","usage":{...}}`. Confirmed item types:
/// `agent_message` (`text`), `error` (`message`), `mcp_tool_call`
/// (`server`, `tool`, `arguments`, `result`, `error`, `status`:
/// `in_progress`/`completed`/`failed` — `error` is an object, not a
/// string, on `failed`), and `todo_list` (`items`: `[{text,
/// completed}]`, re-sent in full on every change). Other item types
/// (local command execution, reasoning, ...) haven't been observed yet
/// and fall through to `Unknown` rather than guessing their shape.
fn classify_codex(value: &Value) -> Option<AgentEvent> {
    let ty = value.get("type")?.as_str()?;
    match ty {
        "thread.started" => Some(AgentEvent::Info("thread started".to_string())),
        "turn.started" => Some(AgentEvent::Info("turn started".to_string())),
        "turn.completed" => Some(AgentEvent::Result(value.clone())),
        "item.started" | "item.updated" | "item.completed" => {
            Some(codex_item_event(value).unwrap_or_else(|| AgentEvent::Unknown(value.clone())))
        }
        _ => None,
    }
}

fn codex_item_event(value: &Value) -> Option<AgentEvent> {
    let item = value.get("item")?;
    match item.get("type").and_then(Value::as_str)? {
        "agent_message" => {
            let text = item.get("text").and_then(Value::as_str)?;
            Some(AgentEvent::AssistantText(text.to_string()))
        }
        "error" => {
            let message = item.get("message").and_then(Value::as_str)?;
            Some(AgentEvent::Error(message.to_string()))
        }
        "mcp_tool_call" => codex_mcp_tool_call_event(item),
        "todo_list" => codex_todo_list_event(item),
        _ => None,
    }
}

fn codex_todo_list_event(item: &Value) -> Option<AgentEvent> {
    let items = item.get("items")?.as_array()?;
    let summary = items
        .iter()
        .map(|entry| {
            let text = entry.get("text").and_then(Value::as_str).unwrap_or("?");
            let done = entry
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!("[{}] {text}", if done { "x" } else { " " })
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(AgentEvent::Info(format!("todo:\n{summary}")))
}

fn codex_mcp_tool_call_event(item: &Value) -> Option<AgentEvent> {
    let server = item.get("server").and_then(Value::as_str)?;
    let tool = item.get("tool").and_then(Value::as_str)?;
    let status = item.get("status").and_then(Value::as_str)?;
    match status {
        "in_progress" => {
            let input = item.get("arguments").cloned().unwrap_or(Value::Null);
            Some(AgentEvent::ToolCall {
                name: format!("{server}::{tool}"),
                input,
            })
        }
        // A tool call that fails (e.g. an invalid argument the MCP server
        // rejects) gets its own terminal status, "failed" — distinct from
        // "completed" with an error field, though we treat both as an
        // error ToolResult. `error` is an object (`{"message": "..."}`),
        // not a plain string.
        "completed" | "failed" => {
            let is_error = status == "failed" || item.get("error").is_some_and(|e| !e.is_null());
            let content = if is_error {
                item.get("error").cloned().unwrap_or(Value::Null)
            } else {
                item.get("result").cloned().unwrap_or(Value::Null)
            };
            Some(AgentEvent::ToolResult { content, is_error })
        }
        _ => None,
    }
}

/// Longest error message rendered in full before truncation — a tool error
/// is usually a short human-readable string, but nothing stops a server
/// from returning something huge, and the dashboard shouldn't hang trying
/// to word-wrap it.
const MAX_TOOL_RESULT_TEXT_LEN: usize = 2000;

/// Best-effort plain-text summary of a `ToolResult`'s `content`, for
/// display — e.g. in the "tool result: error" log line. Handles every
/// shape observed so far: Claude Code errors are a plain string; Codex
/// errors are `{"message": "..."}`; either agent's `content` can also be
/// an array of `{"type":"text","text":"..."}` blocks. Falls back to
/// compact JSON for anything else rather than showing nothing.
pub fn tool_result_text(value: &Value) -> String {
    fn text_blocks(blocks: &[Value]) -> Option<String> {
        let texts: Vec<&str> = blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect();
        (!texts.is_empty()).then(|| texts.join("\n"))
    }

    let text = match value {
        Value::String(s) => s.clone(),
        Value::Object(map) => map
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                map.get("content")
                    .and_then(Value::as_array)
                    .and_then(|blocks| text_blocks(blocks))
            })
            .unwrap_or_else(|| value.to_string()),
        Value::Array(blocks) => text_blocks(blocks).unwrap_or_else(|| value.to_string()),
        Value::Null => String::new(),
        other => other.to_string(),
    };

    if text.chars().count() > MAX_TOOL_RESULT_TEXT_LEN {
        let truncated: String = text.chars().take(MAX_TOOL_RESULT_TEXT_LEN).collect();
        format!("{truncated}… (truncated)")
    } else {
        text
    }
}

/// Opportunistically pull a session/thread id out of a raw JSONL line,
/// regardless of how it classifies. Claude Code includes `session_id` on
/// most stream-json event types; Codex uses `thread_id` instead (on its
/// `thread.started` event) — scanning generically for either avoids
/// depending on exactly which shape produced the line, and both feed the
/// same `TurnOptions::session_id` for `--resume`/`exec resume`.
pub fn extract_session_id(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    value
        .get("session_id")
        .or_else(|| value.get("thread_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real, captured Claude Code MCP error -- `content` is a plain string.
    #[test]
    fn tool_result_text_from_plain_string() {
        let value = serde_json::json!("MCP error -32602: client_id no encontrado para el llamante autenticado");
        assert_eq!(
            tool_result_text(&value),
            "MCP error -32602: client_id no encontrado para el llamante autenticado"
        );
    }

    // Real, captured Codex `mcp_tool_call` "failed" shape -- `error` is an
    // object with a `message` field, not a plain string.
    #[test]
    fn tool_result_text_from_codex_error_object() {
        let value = serde_json::json!({"message": "tool call error: tool call failed for `mcp-portfolio/get_performance`"});
        assert_eq!(
            tool_result_text(&value),
            "tool call error: tool call failed for `mcp-portfolio/get_performance`"
        );
    }

    #[test]
    fn tool_result_text_from_text_block_array() {
        let value = serde_json::json!([{"type": "text", "text": "not found"}]);
        assert_eq!(tool_result_text(&value), "not found");
    }

    #[test]
    fn tool_result_text_from_nested_content_object() {
        let value = serde_json::json!({"content": [{"type": "text", "text": "no permission"}]});
        assert_eq!(tool_result_text(&value), "no permission");
    }

    #[test]
    fn tool_result_text_falls_back_to_json_for_unrecognized_shape() {
        let value = serde_json::json!({"code": 42});
        assert_eq!(tool_result_text(&value), r#"{"code":42}"#);
    }

    #[test]
    fn tool_result_text_truncates_long_messages() {
        let long = "x".repeat(MAX_TOOL_RESULT_TEXT_LEN + 500);
        let value = serde_json::json!(long);
        let result = tool_result_text(&value);
        assert!(result.ends_with("… (truncated)"));
        assert!(result.chars().count() < long.chars().count());
    }

    #[test]
    fn extracts_session_id_when_present() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#;
        assert_eq!(extract_session_id(line), Some("abc-123".to_string()));
    }

    #[test]
    fn extracts_codex_thread_id_as_session_id() {
        let line =
            r#"{"type":"thread.started","thread_id":"01a07df3-c913-7602-b0eb-da2f407e44d5"}"#;
        assert_eq!(
            extract_session_id(line),
            Some("01a07df3-c913-7602-b0eb-da2f407e44d5".to_string())
        );
    }

    #[test]
    fn extract_session_id_none_when_absent() {
        assert_eq!(extract_session_id(r#"{"type":"assistant"}"#), None);
    }

    #[test]
    fn classifies_assistant_text() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        match classify_line(line) {
            AgentEvent::AssistantText(text) => assert_eq!(text, "hi"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classifies_unparseable_lines() {
        matches!(classify_line("not json"), AgentEvent::Unparseable(_));
    }

    // Real, captured `claude --output-format stream-json --verbose` line —
    // Claude Code emits hundreds to thousands of these per turn while a
    // thinking block is being generated, purely as a token-count heartbeat
    // (no reasoning text), distinct from the `"thinking"` content block
    // classified below.
    #[test]
    fn classifies_thinking_tokens_heartbeat() {
        let line = r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":1004,"estimated_tokens_delta":1,"uuid":"e2290348-5470-43ec-89dd-0f91012a184e","session_id":"6e6e883a-cee1-4f6a-9efd-6ce8112bb6a3"}"#;
        match classify_line(line) {
            AgentEvent::ThinkingTokens(estimated_tokens) => assert_eq!(estimated_tokens, 1004),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classifies_thinking_content_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"reasoning about the request"}]}}"#;
        match classify_line(line) {
            AgentEvent::Thinking(text) => assert_eq!(text, "reasoning about the request"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // The following four lines are a real, captured `codex exec --json`
    // 0.146.0 session (thread.started -> turn.started -> item.completed x2
    // -> turn.completed), not synthesized.

    #[test]
    fn classifies_codex_thread_started_as_info() {
        let line =
            r#"{"type":"thread.started","thread_id":"01a07dd9-7f75-74e0-ab3e-9a38886f1400"}"#;
        assert!(matches!(classify_line(line), AgentEvent::Info(_)));
    }

    #[test]
    fn classifies_codex_agent_message_item() {
        let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Hi! What can I help you with today?"}}"#;
        match classify_line(line) {
            AgentEvent::AssistantText(text) => {
                assert_eq!(text, "Hi! What can I help you with today?")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classifies_codex_error_item() {
        let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Model metadata for `deepseek-v4-flash` not found."}}"#;
        match classify_line(line) {
            AgentEvent::Error(message) => {
                assert_eq!(message, "Model metadata for `deepseek-v4-flash` not found.")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classifies_codex_turn_completed_as_result() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":11889,"output_tokens":42}}"#;
        assert!(matches!(classify_line(line), AgentEvent::Result(_)));
    }

    #[test]
    fn unrecognized_codex_item_falls_back_to_unknown() {
        let line = r#"{"type":"item.completed","item":{"id":"item_2","type":"reasoning","summary":"..."}}"#;
        assert!(matches!(classify_line(line), AgentEvent::Unknown(_)));
    }

    // The following two lines are a real, captured `codex exec --json`
    // 0.146.0 mcp_tool_call pair (get_upcoming_meetings), not synthesized.

    #[test]
    fn classifies_codex_mcp_tool_call_started() {
        let line = r#"{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","server":"mcp-crm-calendar","tool":"get_upcoming_meetings","arguments":{},"result":null,"error":null,"status":"in_progress"}}"#;
        match classify_line(line) {
            AgentEvent::ToolCall { name, .. } => {
                assert_eq!(name, "mcp-crm-calendar::get_upcoming_meetings")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classifies_codex_mcp_tool_call_completed() {
        let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","server":"mcp-crm-calendar","tool":"get_upcoming_meetings","arguments":{},"result":{"content":[{"type":"text","text":"..."}],"structured_content":null},"error":null,"status":"completed"}}"#;
        match classify_line(line) {
            AgentEvent::ToolResult { is_error, .. } => assert!(!is_error),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classifies_codex_mcp_tool_call_error_as_tool_result_error() {
        // Real, captured "status":"failed" line (get_performance rejecting
        // an out-of-range period) — `error` is an object, not a string.
        let line = r#"{"type":"item.completed","item":{"id":"item_7","type":"mcp_tool_call","server":"mcp-portfolio","tool":"get_performance","arguments":{"client_id":"cli-001","period":"YTD"},"result":null,"error":{"message":"tool call error: tool call failed for `mcp-portfolio/get_performance`"},"status":"failed"}}"#;
        match classify_line(line) {
            AgentEvent::ToolResult { is_error, .. } => assert!(is_error),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // Real, captured `codex exec --json` 0.146.0 todo_list lifecycle
    // (item.started -> item.updated -> item.completed), not synthesized.

    #[test]
    fn classifies_codex_todo_list_item_updated_envelope() {
        let line = r#"{"type":"item.updated","item":{"id":"item_2","type":"todo_list","items":[{"text":"Identify client_id","completed":true},{"text":"Review positions","completed":false}]}}"#;
        match classify_line(line) {
            AgentEvent::Info(summary) => {
                assert!(summary.contains("[x] Identify client_id"));
                assert!(summary.contains("[ ] Review positions"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn classifies_codex_todo_list_started_and_completed() {
        let started = r#"{"type":"item.started","item":{"id":"item_2","type":"todo_list","items":[{"text":"Step one","completed":false}]}}"#;
        assert!(matches!(classify_line(started), AgentEvent::Info(_)));

        let completed = r#"{"type":"item.completed","item":{"id":"item_2","type":"todo_list","items":[{"text":"Step one","completed":true}]}}"#;
        match classify_line(completed) {
            AgentEvent::Info(summary) => assert!(summary.contains("[x] Step one")),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
