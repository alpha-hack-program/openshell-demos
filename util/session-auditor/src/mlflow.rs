//! Codex → MLflow trace export — pure Rust, in-process (`reqwest`), no
//! Python, no Node, no protobuf. Claude Code's own MLflow tracing is a
//! completely separate, already-working Python `mlflow autolog claude`
//! Stop-hook (see `demos/keycloak-oidc/docs/mlflow-tracing.md`) — untouched
//! by this module.
//!
//! The wire protocol here was verified live against this repo's real RHOAI
//! MLflow server (not assumed from docs), and it is **not** the single
//! "StartTraceV3 with inline spans" call it might look like from MLflow's
//! own REST API docs:
//!
//! 1. `GET/POST .../api/2.0/mlflow/experiments/{get-by-name,create}` —
//!    get-or-create the experiment, same as `mlflow.set_experiment` does
//!    client-side.
//! 2. `POST {tracking_uri}/api/3.0/mlflow/traces` (`StartTraceV3`) —
//!    **trace_info only**. Confirmed by reading the real `mlflow==3.12.0`
//!    client (`mlflow/store/tracking/rest_store.py`): it always calls this
//!    with `spans=[]`. A probe with a populated `spans` array here got a
//!    silent `200 OK` with the spans dropped on the floor — no error, just
//!    missing data. `trace_info.trace_id` must be supplied by the client
//!    as `"tr-" + hex(16 random bytes)`; the server does not generate one
//!    (an omitted trace_id also silently "succeeds", with `trace_id: ""`
//!    forever after).
//! 3. `POST {otlp_root}/v1/traces` — the *actual* span data, as a second,
//!    independent OTLP/HTTP call (confirmed from
//!    `mlflow/server/otel_api.py`). Two load-bearing, easy-to-get-wrong
//!    details, both confirmed live:
//!    - `otlp_root` is the tracking URI **without** its `/mlflow` path
//!      segment — this route is mounted at the webserver root, not under
//!      the `/mlflow` prefix every other REST call in this file uses.
//!      Getting this wrong returns the exact same generic
//!      `{"error":{"code":"ENDPOINT_NOT_FOUND"}}` as any other wrong path.
//!    - The endpoint accepts plain OTLP/HTTP **JSON** (not just protobuf),
//!      but `traceId`/`spanId`/`parentSpanId` must be **hex** strings, not
//!      base64 (the server hex-decodes then re-encodes to base64 itself
//!      before protobuf-JSON parsing; sending base64 directly silently
//!      corrupts the IDs).
//!
//! Traces produced this way are confirmed to store spans in MLflow's
//! tracking-store database (`mlflow.trace.spansLocation: TRACKING_STORE`),
//! not as a fetchable artifact file — don't try to verify via artifact
//! fetch, use `traces/search` instead.
//!
//! **A fourth landmine, found live after shipping, not during initial
//! protocol discovery**: a span attribute whose JSON value contains
//! recognizable token-count field names (`total_tokens`, `input_tokens`,
//! `output_tokens`, ...) — e.g. a `mlflow.chat.tokenUsage` attribute on an
//! "llm" span — silently breaks this server's computation of
//! `trace_info.execution_duration` for the *whole trace* (the field goes
//! missing entirely, not just wrong), even though the OTLP export itself
//! still returns `200` with no error. Confirmed via a dedicated probe:
//! `{"foo": 1}` under that same attribute key left duration intact;
//! `{"total_tokens": 5}` broke it. This is a server-side bug in this
//! RHOAI MLflow build's JSON-OTLP ingestion path, not something fixable
//! client-side — don't put per-call token usage on spans here. The
//! trace-level `mlflow.trace.tokenUsage` (set via `StartTraceV3`'s
//! `trace_metadata`, a separate code path) is unaffected.
//!
//! **A fifth landmine**: Claude and Codex/DeepSeek report token usage
//! with fundamentally different accounting conventions, confirmed by
//! comparing real traces side by side in the MLflow UI. Claude's
//! `input_tokens`/`total_tokens` already exclude cache-read tokens
//! (reported separately as `cache_read_input_tokens`); Codex/DeepSeek's
//! raw `input_tokens` *includes* the cached portion, making its
//! `total_tokens` wildly inflated relative to Claude's for an otherwise
//! equivalent turn (one real example: 38,102 raw input tokens, of which
//! 37,504 were cache hits — only 598 were actually new). `token_usage_from`
//! subtracts the cached portion so both agents' `mlflow.trace.tokenUsage`
//! mean the same thing — see its own doc comment for the full story.

use crate::{flatten_codex_text_field, log};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

/// Server-side allowlist in `mlflow/server/otel_api.py` — any other
/// `service.name` is silently dropped (not rejected), not just ignored by
/// us. `codex_cli_rs` correctly identifies this data's real origin, even
/// though `session-auditor`, not `codex` itself, is the process making
/// this HTTP call.
const SERVICE_NAME: &str = "codex_cli_rs";

pub struct MlflowConfig {
    tracking_uri: String,
    tracking_token: String,
    workspace: String,
    insecure_tls: bool,
    experiment_name: String,
}

impl MlflowConfig {
    /// `None` means "tracing not configured for this sandbox" — every
    /// caller treats that as a silent no-op, matching this binary's
    /// fail-soft convention everywhere else. Read directly from the
    /// environment rather than threaded through the shared `clap::Args`:
    /// this is a Stop-hook push concern independent of the classification
    /// `Config`, with no CLI-flag override need of its own.
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var("MLFLOW_CODEX_TRACING_ENABLED")
            .map(|v| v == "true")
            .unwrap_or(false);
        if !enabled {
            return None;
        }
        let tracking_uri = std::env::var("MLFLOW_TRACKING_URI").ok()?;
        let tracking_token = std::env::var("MLFLOW_TRACKING_TOKEN").ok()?;
        let workspace = std::env::var("MLFLOW_WORKSPACE").ok()?;
        let insecure_tls = std::env::var("MLFLOW_TRACKING_INSECURE_TLS")
            .map(|v| v == "true")
            .unwrap_or(false);
        let experiment_name = std::env::var("MLFLOW_CODEX_EXPERIMENT_NAME")
            .unwrap_or_else(|_| "codex-code-traces".to_string());
        Some(MlflowConfig {
            tracking_uri: tracking_uri.trim_end_matches('/').to_string(),
            tracking_token,
            workspace,
            insecure_tls,
            experiment_name,
        })
    }

    /// The OTLP ingest route lives at the server root, not under the
    /// `/mlflow` prefix the rest of this file's REST calls use — see the
    /// module doc comment. `MLFLOW_TRACKING_URI` is conventionally
    /// `https://<host>:<port>/mlflow` throughout this repo.
    fn otlp_root(&self) -> &str {
        self.tracking_uri
            .strip_suffix("/mlflow")
            .unwrap_or(&self.tracking_uri)
    }
}

struct ToolCall {
    name: String,
    arguments: String,
    output: String,
    start_time_unix_nano: i128,
    end_time_unix_nano: i128,
}

/// One `function_call` decided during a single model inference step. A
/// step can decide on *several* of these at once (parallel tool calling) —
/// confirmed live (aud-codex-bob, 2026-11-14, a "catch me up" turn): one
/// step emitted 8 separate `function_call` items, all before the single
/// `token_usage_record` marking that step's end, with all 8
/// `function_call_output`s only arriving afterward. A first version of
/// this code kept only the *last* decision per step, which — compared
/// side by side with a real Claude trace in the MLflow UI — showed as an
/// "llm" span claiming it decided to call tool X, immediately followed in
/// the UI by several unrelated tool spans that ran before X ever did.
struct ToolCallDecision {
    name: String,
    arguments: String,
}

/// One per `token_usage_record` line — confirmed live that codex-cli logs
/// exactly one of these after each model inference step. Codex's own
/// transcript never exposes the raw request sent to the model (unlike
/// Claude Code's SDK-level autolog, which can), so there's no
/// Claude-equivalent "Inputs: model, messages" to show here — `tool_calls`
/// and `message` below (either or both may be non-empty/`None` for a given
/// step — confirmed live that a step can emit an interim message *and* a
/// batch of tool calls) are the one thing about the step we can observe
/// directly and honestly. Added after live comparison against a real
/// Claude Code trace in the MLflow UI showed Claude's traces include an
/// "llm" span per inference step and Codex's didn't — the flat
/// AGENT+TOOL-only span tree undersold how many model calls a tool-using
/// turn actually made. Deliberately does *not* carry this step's own
/// per-call token usage (`payload.usage`) — see the module doc comment's
/// fourth landmine for why that can't safely go on a span here.
struct LlmCall {
    start_time_unix_nano: i128,
    end_time_unix_nano: i128,
    tool_calls: Vec<ToolCallDecision>,
    message: Option<String>,
}

#[derive(Default)]
struct TokenUsage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    reasoning_output_tokens: u64,
}

impl TokenUsage {
    fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
    }
}

struct CodexTrace {
    prompt: String,
    response: String,
    tool_calls: Vec<ToolCall>,
    llm_calls: Vec<LlmCall>,
    token_usage: Option<TokenUsage>,
    /// Codex's own conversation identity (`session_meta.payload.session_id`
    /// — confirmed live, aud-codex-bob, codex-cli 0.153.4), mapped to
    /// `mlflow.trace.session` so the MLflow UI's "Session" column groups
    /// Codex turns the same way it already does for Claude's (confirmed
    /// live that Claude's traces carry this exact metadata key). Added
    /// after live comparison showed Codex traces missing the column
    /// entirely. `None` only if a transcript somehow lacks a `session_meta`
    /// line — the trace still pushes, just without session grouping.
    session_id: Option<String>,
    start_time_unix_nano: i128,
    end_time_unix_nano: i128,
    /// `"OK"` or `"ERROR"` — the only two `trace_info.state` values this
    /// binary ever sends, confirmed accepted live.
    status: &'static str,
}

/// Parses the same Codex JSONL transcript `extract_transcript_codex`
/// flattens to text, but into structured fields for trace-building
/// instead. Deliberately a separate function rather than a literal reuse
/// of that one: the output shape here (prompt/response/tool
/// calls/timing/tokens) is fundamentally different from a flattened
/// classification string, but the line-by-line `payload.type` dispatch and
/// the `flatten_codex_text_field` helper are shared, so both stay in sync
/// with whatever shape codex-cli actually emits.
fn parse_codex_trace(content: &str) -> Option<CodexTrace> {
    let lines: Vec<&str> = content.lines().collect();

    // `session_meta` appears exactly once, at the very start of the file —
    // confirmed live across multiple turns of a resumed conversation that
    // it does *not* repeat per turn — so this scans the whole file,
    // deliberately unscoped by the turn boundary found below.
    let session_id = lines.iter().find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        (v.get("type").and_then(|t| t.as_str()) == Some("session_meta"))
            .then(|| v.pointer("/payload/session_id").and_then(|s| s.as_str()))
            .flatten()
            .map(|s| s.to_string())
    });

    // Scope everything else to the CURRENT turn only. Confirmed live
    // (aud-codex-bob, 2026-11-15, a resumed 2-turn session): codex-cli
    // appends every turn of a `--resume`d conversation to the *same*
    // rollout file, each bounded by its own `event_msg`/`task_started` ...
    // `task_complete` pair. The Stop hook fires once per turn but always
    // reads the whole (growing) file — without this, the second trace in a
    // resumed conversation showed the *first* turn's prompt (scanning the
    // whole file for the first real user message) with the *union* of
    // every turn's tool calls, and an `execution_duration` covering the
    // cumulative time since the session began rather than just that turn.
    let turn_start = lines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, line)| {
            let v: Value = serde_json::from_str(line).ok()?;
            (v.get("type").and_then(|t| t.as_str()) == Some("event_msg")
                && v.pointer("/payload/type").and_then(|t| t.as_str()) == Some("task_started"))
            .then_some(i)
        })
        .unwrap_or(0);

    let mut prompt: Option<String> = None;
    let mut response: Option<String> = None;
    let mut pending_calls: HashMap<String, (String, String, i128)> = HashMap::new();
    let mut tool_calls = Vec::new();
    let mut llm_calls = Vec::new();
    let mut token_usage: Option<TokenUsage> = None;
    let mut first_ts: Option<i128> = None;
    let mut last_ts: Option<i128> = None;
    // One "segment" per model inference step, bounded by `token_usage_record`
    // lines (confirmed live: codex-cli logs exactly one of these after each
    // step) — see `LlmCall`'s own doc comment. A step's `function_call`s
    // accumulate in `segment_tool_calls` (plural — a step can decide on
    // several at once, confirmed live) rather than overwriting a single
    // slot.
    let mut segment_start_ts: Option<i128> = None;
    let mut segment_tool_calls: Vec<ToolCallDecision> = Vec::new();
    let mut segment_message: Option<String> = None;

    for line in &lines[turn_start..] {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(ts) = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_rfc3339_nanos)
        else {
            continue;
        };
        if first_ts.is_none() {
            first_ts = Some(ts);
        }
        last_ts = Some(ts);

        if v.get("type").and_then(|t| t.as_str()) == Some("response_item")
            && segment_start_ts.is_none()
        {
            segment_start_ts = Some(ts);
        }

        let Some(payload) = v.get("payload") else {
            continue;
        };
        match payload.get("type").and_then(|t| t.as_str()) {
            Some("message") => {
                let role = payload.get("role").and_then(|r| r.as_str());
                let text = payload
                    .get("content")
                    .map(flatten_codex_text_field)
                    .unwrap_or_default();
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                match role {
                    // codex-cli 0.153.4 injects AGENTS.md + environment
                    // context as a *separate, synthetic "user"-role
                    // message* ahead of the real one — re-confirmed live
                    // (aud-codex-bob, 2026-11-14): both carry role="user"
                    // and type="message", indistinguishable without
                    // checking `internal_chat_message_metadata_passthrough
                    // .content_item_kinds`, which tags the synthetic one
                    // `["agents_md.instructions",
                    // "environments.environment_context"]` and the real
                    // one `["user.text"]`. Without this check, picking up
                    // the giant AGENTS.md dump as the trace's "prompt"
                    // instead of the actual question — confirmed live, the
                    // hard way (a real trace landed with exactly that bug).
                    //
                    // Always overwrites (last one wins), mirroring
                    // `response` below — within the current turn's scoped
                    // slice (see `turn_start` above) there's normally just
                    // one real user message, but "last" is the right
                    // choice if that ever isn't true, the same way the
                    // user's actual latest question is what matters, not
                    // whichever came first.
                    Some("user") if !is_synthetic_context_message(payload) => {
                        prompt = Some(text.to_string());
                    }
                    Some("assistant") => {
                        segment_message = Some(text.to_string());
                        response = Some(text.to_string());
                    }
                    _ => {} // "developer" (AGENTS.md injection) — not real conversation
                }
            }
            Some("function_call") => {
                let name = payload
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("?")
                    .to_string();
                let arguments = payload
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string();
                segment_tool_calls.push(ToolCallDecision {
                    name: name.clone(),
                    arguments: arguments.clone(),
                });
                if let Some(call_id) = payload.get("call_id").and_then(|c| c.as_str()) {
                    pending_calls.insert(call_id.to_string(), (name, arguments, ts));
                }
            }
            Some("function_call_output") => {
                let Some(call_id) = payload.get("call_id").and_then(|c| c.as_str()) else {
                    continue;
                };
                let Some((name, arguments, start_ts)) = pending_calls.remove(call_id) else {
                    continue;
                };
                let output = payload
                    .get("output")
                    .map(flatten_codex_text_field)
                    .unwrap_or_default();
                tool_calls.push(ToolCall {
                    name,
                    arguments,
                    output: output.trim().to_string(),
                    start_time_unix_nano: start_ts,
                    end_time_unix_nano: ts,
                });
            }
            _ => {}
        }

        if v.get("type").and_then(|t| t.as_str()) == Some("token_usage_record") {
            // Sums each step's own (non-cumulative) `usage` across the
            // *current turn only* — deliberately not `thread_token_usage`
            // (Codex's own running total since the session began), which
            // would keep growing across a multi-turn resumed conversation
            // and isn't comparable to Claude's per-turn reporting.
            // Confirmed live, comparing real traces side by side with
            // Claude's: Claude's `total_tokens` already excludes
            // cache-read tokens (reported as a separate field), while
            // Codex/DeepSeek's `input_tokens` *includes* them — a second,
            // independent reason the two aren't directly comparable
            // numbers, unrelated to this cumulative-vs-per-turn fix.
            if let Some(usage) = payload.get("usage") {
                let step_usage = token_usage_from(usage);
                token_usage
                    .get_or_insert_with(TokenUsage::default)
                    .add(&step_usage);
            }
            // Finalizes the current model-inference segment, see `LlmCall`'s
            // doc comment. Only skipped if the step produced neither a tool
            // call nor a message at all (shouldn't happen in practice, but
            // segment_start_ts alone isn't a strong enough signal — the
            // very first "response_item" is often the developer/AGENTS.md
            // message, well before any real decision).
            if let Some(start_ts) = segment_start_ts.take() {
                let tool_calls_this_step = std::mem::take(&mut segment_tool_calls);
                let message_this_step = segment_message.take();
                if !tool_calls_this_step.is_empty() || message_this_step.is_some() {
                    llm_calls.push(LlmCall {
                        start_time_unix_nano: start_ts,
                        end_time_unix_nano: ts,
                        tool_calls: tool_calls_this_step,
                        message: message_this_step,
                    });
                }
            }
        }
    }

    let (start_time_unix_nano, end_time_unix_nano) = match (first_ts, last_ts) {
        (Some(s), Some(e)) => (s, e),
        _ => return None,
    };
    // Status is derived from whether a real assistant response was
    // captured, not from the transcript's own `task_complete` event —
    // confirmed live (aud-codex-bob, 2026-11-14) that `task_complete` can
    // still be missing from the on-disk transcript at the moment the Stop
    // hook reads it (a write-ordering race between Codex appending that
    // trailing bookkeeping line and dispatching the hook), even on a
    // completely successful turn with a correct response already present.
    // The Stop hook firing at all is itself evidence the turn completed
    // normally; an empty response is the one thing we can observe
    // directly and reliably treat as a failure signal.
    let has_response = response.as_deref().is_some_and(|r| !r.trim().is_empty());
    Some(CodexTrace {
        prompt: prompt.unwrap_or_default(),
        response: response.unwrap_or_default(),
        tool_calls,
        llm_calls,
        token_usage,
        session_id,
        start_time_unix_nano,
        end_time_unix_nano,
        status: if has_response { "OK" } else { "ERROR" },
    })
}

/// See the doc comment at this function's one call site (inside
/// `parse_codex_trace`'s `"user"` match arm) for the full story.
fn is_synthetic_context_message(payload: &Value) -> bool {
    const SYNTHETIC_KINDS: &[&str] =
        &["agents_md.instructions", "environments.environment_context"];
    payload
        .pointer("/internal_chat_message_metadata_passthrough/content_item_kinds")
        .and_then(|v| v.as_array())
        .is_some_and(|kinds| {
            kinds
                .iter()
                .filter_map(|k| k.as_str())
                .any(|k| SYNTHETIC_KINDS.contains(&k))
        })
}

fn field_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(|n| n.as_u64()).unwrap_or(0)
}

/// Parses a `token_usage_record` usage object (`payload.usage`, this
/// step's own non-cumulative count — deliberately not `payload.
/// thread_token_usage`, Codex's own running total since the session
/// began, see the call site in `parse_codex_trace`) into a `TokenUsage`,
/// normalized to Claude's own accounting convention.
///
/// Confirmed live, comparing real traces side by side in the MLflow UI:
/// Claude's `input_tokens`/`total_tokens` already *exclude* cache-read
/// tokens (reported separately in `cache_read_input_tokens`), but
/// Codex/DeepSeek's raw `input_tokens` *includes* them — e.g. a real
/// turn showed `input_tokens: 38102` of which `cached_input_tokens:
/// 37504` was cache, i.e. only 598 tokens were genuinely new, yet the
/// raw numbers made the two look ~100x apart at a glance. Without this,
/// a user comparing a Codex trace next to a Claude one for the same
/// prompt sees wildly non-comparable "Total" numbers that mean two
/// different things. Subtracting the confirmed-overlapping
/// `cached_input_tokens` here makes `input_tokens`/`total_tokens` mean
/// the same thing — "tokens newly processed this step" — for both
/// agents' traces. (`cache_write_input_tokens` is left alone: unlike
/// `cached_input_tokens`, there's no live confirmation it's a subset of
/// `input_tokens` rather than additive, and it has been `0` in every
/// real transcript seen so far, so there's nothing to get wrong either
/// way yet.)
fn token_usage_from(v: &Value) -> TokenUsage {
    let raw_input = field_u64(v, "input_tokens");
    let cache_read = field_u64(v, "cached_input_tokens");
    TokenUsage {
        input_tokens: raw_input.saturating_sub(cache_read),
        output_tokens: field_u64(v, "output_tokens"),
        total_tokens: raw_input.saturating_sub(cache_read) + field_u64(v, "output_tokens"),
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: field_u64(v, "cache_write_input_tokens"),
        reasoning_output_tokens: field_u64(v, "reasoning_output_tokens"),
    }
}

/// Parses a subset of RFC3339 sufficient for codex-cli's own
/// `"2026-11-14T17:46:03.476Z"` timestamps (always UTC, millisecond
/// precision, `Z` suffix) into unix nanoseconds, without pulling in a date
/// crate for a single fixed, well-known shape.
fn parse_rfc3339_nanos(s: &str) -> Option<i128> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    let (time, millis) = match time.split_once('.') {
        Some((t, ms)) => {
            let padded = format!("{ms:0<3}");
            (t, padded[..3].parse::<i64>().ok()?)
        }
        None => (time, 0),
    };
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    // Days since the Unix epoch via a standard civil-date algorithm
    // (Howard Hinnant's `days_from_civil`) — avoids pulling in a date
    // crate for one fixed, always-UTC input shape.
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    Some((seconds as i128) * 1_000_000_000 + (millis as i128) * 1_000_000)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_hex_id(len: usize) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    // A session-auditor worker makes a handful of these calls per turn and
    // only needs collision-resistant-enough IDs to correlate one trace's
    // own trace_info with its own spans — not cryptographic randomness.
    // Seeded from wall-clock nanos, PID, and a per-process call counter,
    // mixed with a cheap splitmix64 step, rather than pulling in a `rand`
    // crate for this alone. The counter is load-bearing, not decorative:
    // the trace id and every span id for one turn are all generated within
    // microseconds of each other in the same process, and (unlike
    // process-to-process calls) a stack address does *not* vary between
    // calls at the same call site in one process — ASLR randomizes once
    // per process load, not per call — so without a counter, two sibling
    // spans generated back-to-back could get identical ids on a system
    // whose clock doesn't tick between them.
    static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);
    let call_index = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64;
    seed ^= (std::process::id() as u64).wrapping_mul(0x9E3779B97F4A7C15);
    seed ^= call_index.wrapping_mul(0xBF58476D1CE4E5B9);

    let mut bytes = Vec::with_capacity(len);
    let mut state = seed;
    while bytes.len() < len {
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        bytes.extend_from_slice(&z.to_le_bytes());
    }
    bytes.truncate(len);
    hex_encode(&bytes)
}

fn format_rfc3339_millis(unix_nanos: i128) -> String {
    let total_ms = unix_nanos / 1_000_000;
    // i64 comfortably covers any realistic calendar date (days since epoch
    // fits in i32, let alone i64) — narrow here once rather than carrying
    // i128 through the civil-date math.
    let days = (total_ms.div_euclid(86_400_000)) as i64;
    let ms_of_day = total_ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let hour = ms_of_day / 3_600_000;
    let minute = (ms_of_day / 60_000) % 60;
    let second = (ms_of_day / 1_000) % 60;
    let millis = ms_of_day % 1_000;
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Inverse of `days_from_civil` (same Hinnant algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `Stop` handler entry point for the Codex MLflow trace push — a no-op
/// unless `MLFLOW_CODEX_TRACING_ENABLED=true` is set, so Claude sandboxes
/// (which never set it) and Codex sandboxes without tracing configured are
/// both unaffected. Independent of, and runs regardless of, the
/// classification call in `main.rs` — two unrelated best-effort concerns
/// sharing one transcript read.
pub fn maybe_push_codex_trace(content: &str, session_id: &str) {
    let Some(config) = MlflowConfig::from_env() else {
        return;
    };
    let Some(trace) = parse_codex_trace(content) else {
        log(&format!(
            "session {session_id}: no timestamped lines in Codex transcript, skipping MLflow trace"
        ));
        return;
    };

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log(&format!(
                "session {session_id}: failed to start tokio runtime for MLflow push: {e}"
            ));
            return;
        }
    };
    rt.block_on(push_trace(&config, &trace, session_id));
}

async fn push_trace(config: &MlflowConfig, trace: &CodexTrace, session_id: &str) {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(15));
    if config.insecure_tls {
        builder = builder.danger_accept_invalid_certs(true);
    }
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            log(&format!(
                "session {session_id}: failed to build MLflow HTTP client: {e}"
            ));
            return;
        }
    };

    let experiment_id = match get_or_create_experiment(&client, config).await {
        Ok(id) => id,
        Err(e) => {
            log(&format!(
                "session {session_id}: MLflow experiment lookup/create failed: {e}"
            ));
            return;
        }
    };

    let trace_id_hex = random_hex_id(16);
    if let Err(e) = start_trace(&client, config, &experiment_id, &trace_id_hex, trace).await {
        log(&format!(
            "session {session_id}: MLflow StartTraceV3 failed: {e}"
        ));
        return;
    }
    if let Err(e) = export_spans(&client, config, &trace_id_hex, trace).await {
        log(&format!(
            "session {session_id}: MLflow OTLP span export failed: {e}"
        ));
        return;
    }
    log(&format!(
        "session {session_id}: pushed MLflow trace tr-{trace_id_hex} to experiment {} ({})",
        config.experiment_name, experiment_id
    ));
}

async fn get_or_create_experiment(
    client: &reqwest::Client,
    config: &MlflowConfig,
) -> Result<String, String> {
    if let Some(id) = get_experiment_id(client, config).await? {
        return Ok(id);
    }
    let url = format!("{}/api/2.0/mlflow/experiments/create", config.tracking_uri);
    // Tolerate "already exists" the same way this repo's shell scripts
    // tolerate `|| true` on `provider create`/`sandbox create` against
    // already-existing resources — another sandbox's worker may have won
    // the create race since the lookup above.
    let _ = client
        .post(&url)
        .bearer_auth(&config.tracking_token)
        .header("X-MLFLOW-WORKSPACE", &config.workspace)
        .json(&json!({ "name": config.experiment_name }))
        .send()
        .await
        .map_err(|e| format!("failed to create experiment: {e}"))?;
    get_experiment_id(client, config)
        .await?
        .ok_or_else(|| "experiment still not found after create".to_string())
}

async fn get_experiment_id(
    client: &reqwest::Client,
    config: &MlflowConfig,
) -> Result<Option<String>, String> {
    let url = format!(
        "{}/api/2.0/mlflow/experiments/get-by-name",
        config.tracking_uri
    );
    let resp = client
        .get(&url)
        .query(&[("experiment_name", config.experiment_name.as_str())])
        .bearer_auth(&config.tracking_token)
        .header("X-MLFLOW-WORKSPACE", &config.workspace)
        .send()
        .await
        .map_err(|e| format!("failed to look up experiment: {e}"))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON from get-by-name: {e}"))?;
    Ok(body
        .pointer("/experiment/experiment_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}

/// `StartTraceV3` — trace_info only, see the module doc comment for why
/// `spans` is never sent here.
async fn start_trace(
    client: &reqwest::Client,
    config: &MlflowConfig,
    experiment_id: &str,
    trace_id_hex: &str,
    trace: &CodexTrace,
) -> Result<(), String> {
    let url = format!("{}/api/3.0/mlflow/traces", config.tracking_uri);
    let request_time = format_rfc3339_millis(trace.start_time_unix_nano);
    let body = json!({
        "trace": {
            "trace_info": {
                "trace_id": format!("tr-{trace_id_hex}"),
                "trace_location": {
                    "type": "MLFLOW_EXPERIMENT",
                    "mlflow_experiment": { "experiment_id": experiment_id },
                },
                "request_time": request_time,
                "state": trace.status,
                "trace_metadata": trace_metadata(trace),
                "tags": { "mlflow.traceName": "codex_conversation" },
            }
        }
    });
    let resp = client
        .post(&url)
        .bearer_auth(&config.tracking_token)
        .header("X-MLFLOW-WORKSPACE", &config.workspace)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("failed to POST StartTraceV3: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("StartTraceV3 returned {status}: {text}"));
    }
    Ok(())
}

fn trace_metadata(trace: &CodexTrace) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("mlflow.trace_schema.version".to_string(), json!("3"));
    // Plain strings, not JSON-wrapped like `{"prompt": ...}` — confirmed
    // live, the hard way: a JSON-wrapped value here makes the MLflow UI's
    // trace-list "Request"/"Response" columns show the raw JSON text
    // verbatim (`{"prompt":"..."}"`) instead of the extracted
    // prompt/response string the way Claude's traces display it. `status`
    // used to be bundled into traceOutputs too, but that was always
    // redundant with `trace_info.state` — dropped along with the wrapper.
    metadata.insert("mlflow.traceInputs".to_string(), json!(trace.prompt));
    metadata.insert("mlflow.traceOutputs".to_string(), json!(trace.response));
    metadata.insert("mlflow.source.name".to_string(), json!("session-auditor"));
    metadata.insert("mlflow.source.type".to_string(), json!("LOCAL"));
    if let Some(session_id) = &trace.session_id {
        // Same metadata key Claude's own traces carry (confirmed live,
        // `traces/search` against `claude-code-traces`) — lets the MLflow
        // UI's "Session" column group Codex turns the same way it already
        // groups Claude's.
        metadata.insert("mlflow.trace.session".to_string(), json!(session_id));
    }
    if let Some(usage) = &trace.token_usage {
        metadata.insert(
            "mlflow.trace.tokenUsage".to_string(),
            json!(
                json!({
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                    "total_tokens": usage.total_tokens,
                    "cache_read_input_tokens": usage.cache_read_input_tokens,
                    "cache_creation_input_tokens": usage.cache_creation_input_tokens,
                    "reasoning_output_tokens": usage.reasoning_output_tokens,
                })
                .to_string()
            ),
        );
    }
    Value::Object(metadata)
}

/// The actual span data, via a second, independent OTLP/HTTP JSON call —
/// see the module doc comment for why this isn't part of `start_trace`.
async fn export_spans(
    client: &reqwest::Client,
    config: &MlflowConfig,
    trace_id_hex: &str,
    trace: &CodexTrace,
) -> Result<(), String> {
    let url = format!("{}/v1/traces", config.otlp_root());
    let root_span_id = random_hex_id(8);

    let mut spans = vec![otlp_span(
        trace_id_hex,
        &root_span_id,
        None,
        "codex_conversation",
        "AGENT",
        trace.start_time_unix_nano,
        trace.end_time_unix_nano,
        // Plain text, not JSON-wrapped like the TOOL/LLM spans below —
        // confirmed live, the hard way: a JSON-wrapped root span (and
        // trace-level `mlflow.traceInputs`/`traceOutputs`, see
        // `trace_metadata` below) makes the MLflow UI's trace-list
        // "Request"/"Response" columns show the raw
        // `{"prompt": "..."}" text verbatim instead of the extracted
        // prompt/response string the way Claude's traces display it. A
        // dedicated probe confirmed plain strings here produce a clean
        // `request_preview`/`response_preview`; JSON-wrapped ones don't.
        &trace.prompt,
        &trace.response,
        trace.status,
        &[],
    )];
    for tc in &trace.tool_calls {
        let span_id = random_hex_id(8);
        let arguments: Value =
            serde_json::from_str(&tc.arguments).unwrap_or_else(|_| json!(tc.arguments));
        // tc.output is the MCP tool's raw response body, itself JSON-encoded
        // text -- parse it the same way arguments is parsed above. Without
        // this, the MLflow UI sees a plain string (even though its content
        // is JSON) and renders it as an unformatted blob instead of a
        // syntax-highlighted, indented tree the way Claude's own traces do.
        let output: Value = serde_json::from_str(&tc.output).unwrap_or_else(|_| json!(tc.output));
        spans.push(otlp_span(
            trace_id_hex,
            &span_id,
            Some(&root_span_id),
            &tc.name,
            "TOOL",
            tc.start_time_unix_nano,
            tc.end_time_unix_nano,
            &json!({ "arguments": arguments }).to_string(),
            &json!({ "output": output }).to_string(),
            "OK",
            &[],
        ));
    }
    // One "llm" span per model inference step — siblings of the TOOL spans
    // above under the root, same flat one-level tree, not nested further.
    // Added after live comparison against a real Claude trace in the
    // MLflow UI showed Claude's traces include these and Codex's didn't.
    for call in &trace.llm_calls {
        let span_id = random_hex_id(8);
        // Both fields can be non-empty/`Some` for one step (confirmed live:
        // a step can emit an interim message *and* a batch of tool calls),
        // so build the output from whichever are actually present instead
        // of assuming exactly one.
        let mut outputs = serde_json::Map::new();
        if !call.tool_calls.is_empty() {
            let tool_calls: Vec<Value> = call
                .tool_calls
                .iter()
                .map(|tc| {
                    let arguments: Value =
                        serde_json::from_str(&tc.arguments).unwrap_or_else(|_| json!(tc.arguments));
                    json!({ "name": tc.name, "arguments": arguments })
                })
                .collect();
            outputs.insert("tool_calls".to_string(), json!(tool_calls));
        }
        if let Some(message) = &call.message {
            outputs.insert("message".to_string(), json!(message));
        }
        spans.push(otlp_span(
            trace_id_hex,
            &span_id,
            Some(&root_span_id),
            "llm",
            "LLM",
            call.start_time_unix_nano,
            call.end_time_unix_nano,
            // Codex's transcript never exposes the raw request sent to the
            // model (unlike Claude Code's SDK-level autolog) — there's no
            // honest "messages" list to put here.
            "{}",
            &Value::Object(outputs).to_string(),
            "OK",
            // Deliberately NOT a `mlflow.chat.tokenUsage` attribute here
            // (or any attribute whose JSON value contains recognizable
            // token-count field names like `total_tokens`/`input_tokens`
            // anywhere in a trace's spans) — confirmed live, empirically,
            // against this RHOAI MLflow server: a span attribute value
            // containing those field names silently breaks the server's
            // own `execution_duration` computation for the whole trace
            // (trace_info.execution_duration goes missing entirely, not
            // just wrong), even though the OTLP export itself still
            // returns 200 with no error. Narrowed down via a dedicated
            // probe: `{"foo": 1}` under this same attribute key left
            // duration intact; `{"total_tokens": 5}` broke it. This is a
            // server-side bug, not something fixable client-side — the
            // trace-level `mlflow.trace.tokenUsage` (set via StartTraceV3's
            // trace_metadata, a separate code path) is unaffected and
            // still carries the session's token usage.
            &[],
        ));
    }

    let body = json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": SERVICE_NAME } },
                ],
            },
            "scopeSpans": [{
                "scope": { "name": "session-auditor" },
                "spans": spans,
            }],
        }],
    });

    let experiment_id = get_experiment_id(client, config)
        .await?
        .ok_or_else(|| "experiment disappeared between StartTraceV3 and span export".to_string())?;

    let resp = client
        .post(&url)
        .bearer_auth(&config.tracking_token)
        .header("X-MLFLOW-WORKSPACE", &config.workspace)
        .header("x-mlflow-experiment-id", &experiment_id)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("failed to POST OTLP spans: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("OTLP span export returned {status}: {text}"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn otlp_span(
    trace_id_hex: &str,
    span_id_hex: &str,
    parent_span_id_hex: Option<&str>,
    name: &str,
    span_type: &str,
    start_time_unix_nano: i128,
    end_time_unix_nano: i128,
    // Pre-formatted text, not a `Value` the function JSON-encodes itself —
    // callers decide whether that's a bare string (the root span's
    // prompt/response, so the MLflow UI's list-view preview shows clean
    // text) or pre-serialized JSON (TOOL/LLM spans, so the UI's
    // drill-down view renders a syntax-highlighted tree). Passing a
    // `Value` here and always calling `.to_string()` on it (the original
    // design) double-JSON-encoded plain strings into `"\"like
    // this\""`, confirmed live as the cause of the root span (and
    // `mlflow.traceInputs`/`traceOutputs`, see `trace_metadata`) showing
    // raw JSON instead of plain text in the UI.
    inputs: &str,
    outputs: &str,
    status: &str,
    extra_attributes: &[(&str, String)],
) -> Value {
    let status_code = if status == "OK" {
        "STATUS_CODE_OK"
    } else {
        "STATUS_CODE_ERROR"
    };
    let mut attributes = vec![
        json!({ "key": "mlflow.spanType", "value": { "stringValue": json!(span_type).to_string() } }),
        json!({ "key": "mlflow.spanInputs", "value": { "stringValue": inputs } }),
        json!({ "key": "mlflow.spanOutputs", "value": { "stringValue": outputs } }),
    ];
    for (key, value) in extra_attributes {
        attributes.push(json!({ "key": key, "value": { "stringValue": value } }));
    }
    json!({
        "traceId": trace_id_hex,
        "spanId": span_id_hex,
        "parentSpanId": parent_span_id_hex,
        "name": name,
        "startTimeUnixNano": start_time_unix_nano.to_string(),
        "endTimeUnixNano": end_time_unix_nano.to_string(),
        "attributes": attributes,
        "events": [],
        "status": { "code": status_code, "message": "" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lines mirror the real shape re-confirmed live against a running
    // aud-codex-bob sandbox (codex-cli 0.153.4, 2026-11-14) — see the
    // doc comment on `extract_transcript_codex` in main.rs.
    const SAMPLE_TRANSCRIPT: &str = r#"{"timestamp": "2026-11-14T17:46:03.400Z", "type": "session_meta", "payload": {"session_id": "01a1db2a-9527-7c53-b5be-38a5babc4508"}}
{"timestamp": "2026-11-14T17:46:03.476Z", "type": "event_msg", "payload": {"type": "task_started"}}
{"timestamp": "2026-11-14T17:46:03.532Z", "type": "response_item", "payload": {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "huge AGENTS.md dump that should be skipped"}]}}
{"timestamp": "2026-11-14T17:46:03.557Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "What positions does client cli-004 have?"}]}}
{"timestamp": "2026-11-14T17:46:06.409Z", "type": "response_item", "payload": {"type": "function_call", "name": "mcp_portfolio_get_positions", "arguments": "{\"client_id\": \"cli-004\"}", "call_id": "call_00_zPpG8uK6eI3sI2Y7BNXz4559"}}
{"timestamp": "2026-11-14T17:46:06.448Z", "type": "response_item", "payload": {"type": "function_call_output", "call_id": "call_00_zPpG8uK6eI3sI2Y7BNXz4559", "output": [{"type": "input_text", "text": "Wall time: 0.0331 seconds\nOutput:"}, {"type": "input_text", "text": "tool call error: client_id no encontrado"}]}}
{"timestamp": "2026-11-14T17:46:08.697Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "The backend rejected the lookup."}]}}
{"timestamp": "2026-11-14T17:46:08.729Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 25316, "cached_input_tokens": 24960, "cache_write_input_tokens": 0, "output_tokens": 524, "reasoning_output_tokens": 350, "total_tokens": 25840}, "thread_token_usage": {"input_tokens": 25316, "cached_input_tokens": 24960, "cache_write_input_tokens": 0, "output_tokens": 524, "reasoning_output_tokens": 350, "total_tokens": 25840}}}
{"timestamp": "2026-11-14T17:46:08.737Z", "type": "event_msg", "payload": {"type": "task_complete"}}
"#;

    #[test]
    fn parses_real_shaped_codex_transcript() {
        let trace = parse_codex_trace(SAMPLE_TRANSCRIPT).expect("should parse");
        assert_eq!(trace.prompt, "What positions does client cli-004 have?");
        assert_eq!(trace.response, "The backend rejected the lookup.");
        assert_eq!(trace.status, "OK");
        assert_eq!(
            trace.session_id.as_deref(),
            Some("01a1db2a-9527-7c53-b5be-38a5babc4508")
        );
        assert_eq!(trace.tool_calls.len(), 1);
        let tc = &trace.tool_calls[0];
        assert_eq!(tc.name, "mcp_portfolio_get_positions");
        assert_eq!(tc.arguments, r#"{"client_id": "cli-004"}"#);
        // Exact match, not just `.contains` — regression test for a real,
        // live-confirmed bug (aud-codex-bob, 2026-11-14): codex-cli's own
        // "Wall time: ... seconds\nOutput:" preamble item was leaking into
        // this field verbatim before `is_codex_wall_time_preamble` existed.
        assert_eq!(tc.output, "tool call error: client_id no encontrado");
        assert!(tc.start_time_unix_nano < tc.end_time_unix_nano);
        assert!(trace.start_time_unix_nano < trace.end_time_unix_nano);

        let usage = trace.token_usage.expect("token usage should be present");
        // input_tokens is normalized to exclude the cached portion (raw
        // 25316 - cached 24960 = 356), matching Claude's own accounting
        // convention — see `token_usage_from`'s doc comment.
        assert_eq!(usage.input_tokens, 356);
        assert_eq!(usage.cache_read_input_tokens, 24960);
        assert_eq!(usage.total_tokens, 356 + 524); // input + output, cache excluded
        assert_eq!(usage.reasoning_output_tokens, 350);
    }

    #[test]
    fn missing_response_is_error_status() {
        let content = r#"{"timestamp": "2026-11-14T17:46:03.476Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}}
"#;
        let trace = parse_codex_trace(content).expect("should parse");
        assert_eq!(trace.status, "ERROR");
    }

    #[test]
    fn missing_task_complete_event_is_still_ok_status() {
        // Regression test for a real, live-confirmed bug (aud-codex-bob,
        // 2026-11-14): the Stop hook can read the transcript before
        // codex-cli has flushed its trailing `task_complete` bookkeeping
        // line, even on a fully successful turn — status must not depend
        // on that event's presence.
        let content = r#"{"timestamp": "2026-11-14T17:46:03.476Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}}
{"timestamp": "2026-11-14T17:46:04.000Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hello"}]}}
"#;
        let trace = parse_codex_trace(content).expect("should parse");
        assert_eq!(trace.status, "OK");
    }

    #[test]
    fn skips_synthetic_agents_md_user_message_when_finding_the_real_prompt() {
        // Regression test for a real, live-confirmed bug (aud-codex-bob,
        // 2026-11-14): codex-cli 0.153.4 injects the AGENTS.md dump and
        // environment context as a *separate, synthetic* "user"-role
        // message ahead of the real one. Without the
        // `content_item_kinds` check, "first user message wins" picks
        // this up instead of the actual question.
        // Note the `r##"..."##` delimiter (not the usual `r#"..."#`): the
        // AGENTS.md text itself starts with a literal `#`, and `"#` would
        // otherwise prematurely close a single-`#` raw string right there.
        let content = r##"{"timestamp": "2026-11-14T23:01:24.477Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "# AGENTS.md instructions for /sandbox..."}], "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["agents_md.instructions", "environments.environment_context"]}}}
{"timestamp": "2026-11-14T23:01:24.496Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "What is 9 plus 9?"}], "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["user.text"]}}}
{"timestamp": "2026-11-14T23:01:25.472Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "18"}]}}
"##;
        let trace = parse_codex_trace(content).expect("should parse");
        assert_eq!(trace.prompt, "What is 9 plus 9?");
        assert_eq!(trace.response, "18");
        assert_eq!(trace.status, "OK");
    }

    #[test]
    fn second_turn_of_a_resumed_session_only_sees_its_own_turn() {
        // Regression test for a real, live-confirmed bug (aud-codex-bob,
        // 2026-11-15): codex-cli appends every turn of a `--resume`d
        // conversation to the *same* rollout file (confirmed: two full
        // `task_started`...`task_complete` cycles in one real transcript).
        // The Stop hook fires once per turn but always reads the whole,
        // growing file. Before turn-scoping existed, turn 2's trace showed
        // turn 1's prompt ("Any meetings today?" instead of the real "And
        // tomorrow?") and would have unioned both turns' tool calls.
        // Structure mirrors the real transcript exactly, trimmed to the
        // fields that matter: session_meta + turn 1 (with a tool call)
        // only up through its own task_complete, then a second
        // task_started...task_complete cycle with a different question and
        // no tool call.
        let content = r#"{"timestamp": "2026-11-15T10:09:50.000Z", "type": "session_meta", "payload": {"session_id": "01a1deaf-4574-71f3-8f5c-7b7354ec4c87"}}
{"timestamp": "2026-11-15T10:09:50.100Z", "type": "event_msg", "payload": {"type": "task_started"}}
{"timestamp": "2026-11-15T10:09:50.200Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Any meetings today?"}], "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["user.text"]}}}
{"timestamp": "2026-11-15T10:09:50.300Z", "type": "response_item", "payload": {"type": "function_call", "name": "mcp_crm_calendar_get_upcoming_meetings", "arguments": "{}", "call_id": "call_1"}}
{"timestamp": "2026-11-15T10:09:50.400Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 100, "output_tokens": 10, "total_tokens": 110, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}, "thread_token_usage": {"input_tokens": 100, "output_tokens": 10, "total_tokens": 110, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}}}
{"timestamp": "2026-11-15T10:09:50.500Z", "type": "response_item", "payload": {"type": "function_call_output", "call_id": "call_1", "output": [{"type": "input_text", "text": "mtg-001, mtg-002"}]}}
{"timestamp": "2026-11-15T10:09:50.600Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Yes, one meeting today and one tomorrow."}]}}
{"timestamp": "2026-11-15T10:09:50.700Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 50, "output_tokens": 20, "total_tokens": 70, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}, "thread_token_usage": {"input_tokens": 150, "output_tokens": 30, "total_tokens": 180, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}}}
{"timestamp": "2026-11-15T10:09:50.800Z", "type": "event_msg", "payload": {"type": "task_complete"}}
{"timestamp": "2026-11-15T10:10:40.000Z", "type": "event_msg", "payload": {"type": "task_started"}}
{"timestamp": "2026-11-15T10:10:40.100Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "And tomorrow?"}], "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["user.text"]}}}
{"timestamp": "2026-11-15T10:10:40.200Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Yes, one meeting tomorrow at 10:00 UTC."}]}}
{"timestamp": "2026-11-15T10:10:40.300Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 60, "output_tokens": 15, "total_tokens": 75, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}, "thread_token_usage": {"input_tokens": 210, "output_tokens": 45, "total_tokens": 255, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}}}
{"timestamp": "2026-11-15T10:10:40.400Z", "type": "event_msg", "payload": {"type": "task_complete"}}
"#;
        let trace = parse_codex_trace(content).expect("should parse");

        // The real bug: this must be turn 2's own question, not turn 1's.
        assert_eq!(trace.prompt, "And tomorrow?");
        assert_eq!(trace.response, "Yes, one meeting tomorrow at 10:00 UTC.");

        // session_id still resolves even though session_meta sits well
        // before the turn-2 boundary.
        assert_eq!(
            trace.session_id.as_deref(),
            Some("01a1deaf-4574-71f3-8f5c-7b7354ec4c87")
        );

        // Turn 1's tool call must not leak into turn 2's trace.
        assert!(trace.tool_calls.is_empty());
        assert_eq!(trace.llm_calls.len(), 1);

        // execution_duration must reflect only turn 2's ~0.4s window, not
        // the ~50s elapsed since the session (and turn 1) began.
        let duration_nanos = trace.end_time_unix_nano - trace.start_time_unix_nano;
        assert!(
            duration_nanos < 1_000_000_000,
            "expected turn 2's own short duration, got {duration_nanos}ns (looks like it included turn 1)"
        );

        // Trace-level token usage is this turn's own cost only (75, its
        // one step's `usage`) — not the cumulative 255 `thread_token_usage`
        // would show, which would keep growing across turns and isn't
        // comparable to Claude's per-turn reporting (see `token_usage_from`
        // and its call site for why).
        assert_eq!(trace.token_usage.unwrap().total_tokens, 75);
    }

    #[test]
    fn splits_llm_calls_on_each_token_usage_record() {
        // Added after live comparison against a real Claude Code trace in
        // the MLflow UI showed Claude's traces include an "llm" span per
        // model inference step and Codex's didn't. Mirrors the real shape:
        // one token_usage_record after the tool-call decision, a second
        // after the final message — two separate model steps, each with
        // its own (non-cumulative) `usage`.
        let content = r#"{"timestamp": "2026-11-14T17:46:03.557Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "What positions does client cli-004 have?"}]}}
{"timestamp": "2026-11-14T17:46:06.409Z", "type": "response_item", "payload": {"type": "function_call", "name": "mcp_portfolio_get_positions", "arguments": "{\"client_id\": \"cli-004\"}", "call_id": "call_1"}}
{"timestamp": "2026-11-14T17:46:06.420Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 100, "output_tokens": 10, "total_tokens": 110, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}, "thread_token_usage": {"input_tokens": 100, "output_tokens": 10, "total_tokens": 110, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}}}
{"timestamp": "2026-11-14T17:46:06.448Z", "type": "response_item", "payload": {"type": "function_call_output", "call_id": "call_1", "output": [{"type": "input_text", "text": "Wall time: 0.0331 seconds\nOutput:"}, {"type": "input_text", "text": "{\"positions\": []}"}]}}
{"timestamp": "2026-11-14T17:46:08.697Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "No positions found."}]}}
{"timestamp": "2026-11-14T17:46:08.729Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 200, "output_tokens": 20, "total_tokens": 220, "cached_input_tokens": 50, "cache_write_input_tokens": 0, "reasoning_output_tokens": 5}, "thread_token_usage": {"input_tokens": 300, "output_tokens": 30, "total_tokens": 330, "cached_input_tokens": 50, "cache_write_input_tokens": 0, "reasoning_output_tokens": 5}}}
"#;
        let trace = parse_codex_trace(content).expect("should parse");
        assert_eq!(trace.llm_calls.len(), 2);

        assert_eq!(trace.llm_calls[0].tool_calls.len(), 1);
        assert_eq!(
            trace.llm_calls[0].tool_calls[0].name,
            "mcp_portfolio_get_positions"
        );
        assert_eq!(
            trace.llm_calls[0].tool_calls[0].arguments,
            r#"{"client_id": "cli-004"}"#
        );
        assert!(trace.llm_calls[0].message.is_none());

        assert!(trace.llm_calls[1].tool_calls.is_empty());
        assert_eq!(
            trace.llm_calls[1].message.as_deref(),
            Some("No positions found.")
        );

        // Trace-level usage is the sum of both steps' own normalized
        // `usage` — step 1: 100 input (0 cached) + 10 output = 110; step 2:
        // 200 input - 50 cached = 150 new + 20 output = 170. Sum: 280, not
        // 330 (which would double-count step 2's 50 cached tokens as if
        // they were new, or come from `thread_token_usage`, which would
        // also happen to read 330 here only because there's no earlier
        // turn in this transcript to inflate it — see the dedicated
        // resumed-session test for where that distinction actually
        // matters). Per-step usage isn't surfaced on spans themselves
        // (deliberately — see the module doc comment's fourth landmine).
        assert_eq!(trace.token_usage.unwrap().total_tokens, 280);
    }

    #[test]
    fn one_step_can_decide_on_several_parallel_tool_calls() {
        // Regression test for a real, live-confirmed bug (aud-codex-bob,
        // 2026-11-14, a "catch me up" turn): a single model inference step
        // emitted 8 separate `function_call` items before the one
        // `token_usage_record` marking that step's end — all 8
        // `function_call_output`s arrived only afterward. A first version
        // of this code kept only the *last* `function_call` per step
        // (overwriting, not accumulating), so the resulting "llm" span
        // claimed it decided to call only the last of the 8 tools —
        // confirmed live, comparing against a real Claude trace in the
        // MLflow UI, as an "llm" span whose declared tool call didn't match
        // the several unrelated tool spans that actually ran right after it.
        let content = r#"{"timestamp": "2026-11-14T17:46:03.557Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "catch me up"}]}}
{"timestamp": "2026-11-14T17:46:04.000Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Let me pull the notes."}]}}
{"timestamp": "2026-11-14T17:46:04.100Z", "type": "response_item", "payload": {"type": "function_call", "name": "mcp_crm_calendar_get_meeting_notes", "arguments": "{\"meeting_id\": \"mtg-001\"}", "call_id": "call_1"}}
{"timestamp": "2026-11-14T17:46:04.200Z", "type": "response_item", "payload": {"type": "function_call", "name": "mcp_portfolio_get_positions", "arguments": "{\"client_id\": \"cli-001\"}", "call_id": "call_2"}}
{"timestamp": "2026-11-14T17:46:04.300Z", "type": "response_item", "payload": {"type": "function_call", "name": "mcp_kyc_compliance_get_risk_profile", "arguments": "{\"client_id\": \"cli-002\"}", "call_id": "call_3"}}
{"timestamp": "2026-11-14T17:46:04.400Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 500, "output_tokens": 50, "total_tokens": 550, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}, "thread_token_usage": {"input_tokens": 500, "output_tokens": 50, "total_tokens": 550, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}}}
{"timestamp": "2026-11-14T17:46:05.000Z", "type": "response_item", "payload": {"type": "function_call_output", "call_id": "call_1", "output": [{"type": "input_text", "text": "notes ok"}]}}
{"timestamp": "2026-11-14T17:46:05.100Z", "type": "response_item", "payload": {"type": "function_call_output", "call_id": "call_2", "output": [{"type": "input_text", "text": "positions ok"}]}}
{"timestamp": "2026-11-14T17:46:05.200Z", "type": "response_item", "payload": {"type": "function_call_output", "call_id": "call_3", "output": [{"type": "input_text", "text": "risk ok"}]}}
{"timestamp": "2026-11-14T17:46:06.000Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Here's your catch-up."}]}}
{"timestamp": "2026-11-14T17:46:06.100Z", "type": "token_usage_record", "payload": {"usage": {"input_tokens": 600, "output_tokens": 60, "total_tokens": 660, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}, "thread_token_usage": {"input_tokens": 1100, "output_tokens": 110, "total_tokens": 1210, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "reasoning_output_tokens": 0}}}
"#;
        let trace = parse_codex_trace(content).expect("should parse");
        assert_eq!(trace.llm_calls.len(), 2);

        // The first step both emitted an interim message AND decided on
        // all 3 tool calls — neither should be lost.
        let step1 = &trace.llm_calls[0];
        assert_eq!(step1.message.as_deref(), Some("Let me pull the notes."));
        assert_eq!(step1.tool_calls.len(), 3);
        assert_eq!(
            step1.tool_calls[0].name,
            "mcp_crm_calendar_get_meeting_notes"
        );
        assert_eq!(step1.tool_calls[1].name, "mcp_portfolio_get_positions");
        assert_eq!(
            step1.tool_calls[2].name,
            "mcp_kyc_compliance_get_risk_profile"
        );

        assert_eq!(trace.tool_calls.len(), 3);

        let step2 = &trace.llm_calls[1];
        assert!(step2.tool_calls.is_empty());
        assert_eq!(step2.message.as_deref(), Some("Here's your catch-up."));
    }

    #[test]
    fn rfc3339_nanos_round_trips_through_formatting() {
        let nanos = parse_rfc3339_nanos("2026-11-14T17:46:03.476Z").expect("should parse");
        assert_eq!(format_rfc3339_millis(nanos), "2026-11-14T17:46:03.476Z");
    }

    #[test]
    fn random_hex_ids_differ_across_back_to_back_calls() {
        let a = random_hex_id(8);
        let b = random_hex_id(8);
        assert_eq!(a.len(), 16);
        assert_ne!(a, b, "sibling spans must not collide on id");
    }
}
