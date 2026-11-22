// SPDX-License-Identifier: Apache-2.0

//! "Client snapshot": the templated reply to clicking a client in the
//! sidebar's book (previously just a free-text "Tell me about {name}"
//! prompt through the full interactive chat, rendered as plain markdown).
//!
//! Unlike `meeting_brief.rs`, this gives the LLM real tool access
//! (`--mcp-config`) rather than pre-fetching every fact in Rust first —
//! "which client" can be genuinely ambiguous (a first name, a nickname, a
//! typo) and there's no fixed recipe of tool calls that resolves that the
//! way Meeting Brief's "next meeting's client" lookup is unambiguous. The
//! model is left to use its own judgement sorting that out and deciding
//! which of the available tools are worth calling; the only constraint is
//! the output shape, not how it gets there — a one-shot,
//! schema-constrained turn via `parrot_core::run_structured_turn`, same as
//! Meeting Brief's insights call, just with tools turned on instead of off.
//!
//! Deliberately never resumes the chat session (`run_structured_turn` is
//! always called with no prior session id here) -- confirmed by design,
//! not a bug: each click is an independent lookup, and folding it into
//! the ongoing chat thread would let unrelated prior conversation leak
//! into what's supposed to be a clean, schema-constrained JSON reply.
//! The one externally visible side effect is that each click opens its
//! own Codex session from the sandbox/MLflow tracing's point of view,
//! separate from whatever the chat composer is doing in the same
//! sandbox -- that's expected, not a sign the resume mechanism is
//! broken (see `parrot-core/src/exec.rs`'s live Codex resume test,
//! which confirms the shared agent/exec resume logic itself is correct).

use tauri::State;

use crate::AppState;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SnapshotFact {
    label: String,
    value: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TalkingPoint {
    title: String,
    detail: String,
}

/// Same shape as `meeting_brief::EvidenceCall` (`n`/`tool`/`detail`/
/// `source`) so the UI's evidence-panel rendering doesn't need a second
/// code path -- `source` is always "Sandbox agent" here since, unlike
/// Meeting Brief's hand-labeled calls (CRM/Portfolio/KYC...), these come
/// generically from whatever tools the model itself chose to call.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EvidenceRow {
    n: u32,
    tool: String,
    detail: String,
    source: String,
}

/// The shape the prompt below asks the LLM to reply with, verbatim --
/// `evidence` is added afterwards from `run_structured_turn`'s own
/// tool-call tracking, not part of what the model returns.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ClientSnapshot {
    client_name: String,
    summary: String,
    facts: Vec<SnapshotFact>,
    talking_points: Vec<TalkingPoint>,
    #[serde(default)]
    evidence: Vec<EvidenceRow>,
}

#[tauri::command]
pub async fn mcp_client_snapshot(
    query: String,
    state: State<'_, AppState>,
) -> Result<ClientSnapshot, String> {
    let sandbox = state
        .sandbox
        .lock()
        .expect("sandbox mutex poisoned")
        .clone()
        .ok_or_else(|| "not connected to a sandbox yet".to_string())?;
    let mcp_config = state
        .mcp_config
        .clone()
        .or_else(|| state.agent.default_mcp_config().map(str::to_string));
    build_client_snapshot(
        &query,
        &state.client,
        &sandbox,
        state.workspace.as_deref(),
        state.agent,
        mcp_config.as_deref(),
        state.environment.clone(),
    )
    .await
}

/// The actual orchestration, kept separate from the `#[tauri::command]`
/// wrapper above so it can be exercised directly (see this crate's
/// development notes) without needing a running Tauri app/webview --
/// mirrors `meeting_brief::build_meeting_brief`'s split for the same reason.
#[allow(clippy::too_many_arguments)]
pub async fn build_client_snapshot(
    query: &str,
    exec_client: &parrot_core::ParrotClient,
    sandbox: &str,
    workspace: Option<&str>,
    agent: parrot_core::Agent,
    mcp_config: Option<&str>,
    environment: std::collections::HashMap<String, String>,
) -> Result<ClientSnapshot, String> {
    let prompt = format!(
        "A private banker wants a quick snapshot of one of their clients, referred to \
         here as \"{query}\" -- this might be their full name, a first name, or a typo. \
         Use your available tools to find the best-matching client and look up their \
         positions, recent performance, risk profile, and any compliance flags \
         (suitability, PEP, KYC status). If the match is ambiguous or you can't find a \
         client at all, pick your best guess (or say so) rather than leaving fields \
         empty.\n\n\
         Respond with ONLY a JSON object, no markdown code fence and no text before or \
         after it, matching exactly this shape:\n\
         {{\"client_name\": \"resolved client name (or your best guess, noted as such)\", \
         \"summary\": \"2 to 3 sentence overview of where this client stands\", \
         \"facts\": [{{\"label\": \"short label, e.g. Risk profile\", \"value\": \"short value\"}}], \
         \"talking_points\": [{{\"title\": \"short title\", \"detail\": \"one sentence\"}}]}}\n\
         Produce 3 to 6 facts and 1 to 3 talking points. Every fact must come from a \
         tool result, not be guessed."
    );

    let result = parrot_core::run_structured_turn(
        exec_client,
        sandbox,
        workspace,
        agent,
        mcp_config,
        environment,
        &prompt,
    )
    .await
    .map_err(|e| e.to_string())?;

    let mut snapshot: ClientSnapshot =
        serde_json::from_value(result.value).map_err(|e| e.to_string())?;
    snapshot.evidence = result
        .evidence
        .into_iter()
        .enumerate()
        .map(|(i, e)| EvidenceRow {
            n: i as u32 + 1,
            tool: e.tool,
            detail: e.detail,
            source: "Sandbox agent".to_string(),
        })
        .collect();
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the real recipe against a live cluster -- not run in CI
    /// (needs the OpenShell CLI's own config/state pointed at an
    /// already-logged-in identity, same as `meeting_brief`'s live test).
    /// Run manually, from an env with `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/
    /// `SSL_CERT_FILE` already set up for a demo persona (e.g. alice):
    ///
    /// ```sh
    /// SANDBOX_NAME=claude-alice WORKSPACE=alice \
    /// ANTHROPIC_BASE_URL=... ANTHROPIC_MODEL=... \
    /// cargo test -p parrot-tauri client_snapshot::tests::live -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn live_build_client_snapshot_against_alice() {
        let sandbox = std::env::var("SANDBOX_NAME").unwrap_or_else(|_| "claude-alice".to_string());
        let workspace = std::env::var("WORKSPACE").unwrap_or_else(|_| "alice".to_string());

        let exec_client = parrot_core::ParrotClient::connect(None, None)
            .await
            .expect("connect to OpenShell gateway");

        let snapshot = build_client_snapshot(
            "Elena",
            &exec_client,
            &sandbox,
            Some(workspace.as_str()),
            parrot_core::Agent::ClaudeCode,
            parrot_core::Agent::ClaudeCode.default_mcp_config(),
            parrot_core::detect_passthrough_environment(),
        )
        .await
        .expect("build_client_snapshot");

        assert!(!snapshot.client_name.is_empty());
        assert!(!snapshot.facts.is_empty());

        println!("client_name: {}", snapshot.client_name);
        println!("summary: {}", snapshot.summary);
        for fact in &snapshot.facts {
            println!("fact: {} = {}", fact.label, fact.value);
        }
        for point in &snapshot.talking_points {
            println!("talking point: {} — {}", point.title, point.detail);
        }
        assert!(!snapshot.evidence.is_empty(), "expected tool-call evidence");
        for e in &snapshot.evidence {
            println!("evidence: {} — {} ({})", e.tool, e.detail, e.source);
        }
    }
}
