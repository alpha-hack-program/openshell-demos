// SPDX-License-Identifier: Apache-2.0

//! The "Meeting Brief" feature: a report for the banker's next upcoming
//! meeting. Every *fact* (performance, allocation, suitability, notes,
//! news) comes from a fixed recipe of direct MCP tool calls — no LLM
//! involved, nothing invented. The *headline*/*talking points* are the
//! interpretive part: a one-shot, schema-constrained LLM call
//! (`request_brief_insights`) is handed exactly those already-verified
//! facts and asked to turn them into prose, with no tool access of its
//! own (it can't go look up anything not already given to it, so it
//! can't introduce a fact that isn't traceable to a tool result above).
//! If that call fails or returns something unparseable,
//! `build_headline`/`build_talking_points`'s plain Rust templating is the
//! fallback — the brief never breaks because the LLM step had a bad day.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use parrot_core::McpGatewayClient;
use tauri::State;

use crate::mcp::McpState;
use crate::AppState;

/// This demo's product catalog (`mcp-kyc-compliance`'s `check_suitability`
/// takes a `product_id` but there's no `list_products` tool for the banker
/// role to discover these from — see `mcp/mcp-portfolio/.../schema-init-
/// configmap.yaml`'s seed data, the one place they're defined).
const PRODUCTS: &[(&str, &str)] = &[
    ("prod-001", "Meridian Capital Preservation Note"),
    ("prod-002", "Meridian Balanced Growth Fund"),
    ("prod-003", "Meridian Global Logistics Equity Fund"),
    ("prod-004", "Meridian Textile Sector Note"),
    ("prod-005", "Meridian Technology Innovation Fund"),
    ("prod-006", "Meridian Import Trade Finance Fund"),
];

/// A client's sector concentration above this fails `check_suitability`
/// for any further product in that sector — mirrors
/// `mcp-kyc-compliance/data/corpus/03-suitability.md`'s own rule, used
/// here only to decide which sectors to call out, not to second-guess the
/// tool's own pass/fail verdict.
const CONCENTRATION_LIMIT_PCT: f64 = 25.0;

/// `twr`/`benchmark_twr` are `None` when the gateway has no performance
/// data for this client for this period — confirmed live (not every
/// client/period combination in this demo's fixture data has one), so
/// this is a real, expected case, not a call to fail the whole brief over.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PerformancePeriod {
    period: String,
    twr: Option<f64>,
    benchmark_twr: Option<f64>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct AllocationSlice {
    sector: String,
    pct: f64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct NewsItem {
    headline: String,
    ticker: Option<String>,
    sector: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SuitabilityResult {
    product_name: String,
    potentially_suitable: bool,
    explanation: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RegulatoryFragment {
    source: String,
    text: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TalkingPoint {
    title: String,
    detail: String,
}

/// The shape `request_brief_insights`'s prompt asks the LLM to reply
/// with, verbatim -- parsed straight out of `run_structured_turn`'s
/// result.
#[derive(serde::Deserialize)]
struct BriefInsights {
    headline: String,
    talking_points: Vec<TalkingPoint>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct EvidenceCall {
    n: u32,
    tool: String,
    detail: String,
    source: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct MeetingBrief {
    meeting_id: String,
    client_id: String,
    client_name: String,
    datetime: String,
    risk_profile: Option<String>,
    kyc_status: Option<String>,
    pep_flag: bool,
    sector_focus: Option<String>,
    notes: Option<String>,
    headline: String,
    performance: Vec<PerformancePeriod>,
    allocation: Vec<AllocationSlice>,
    over_limit_sectors: Vec<String>,
    news: Vec<NewsItem>,
    suitability: Vec<SuitabilityResult>,
    regulatory: Option<RegulatoryFragment>,
    talking_points: Vec<TalkingPoint>,
    evidence: Vec<EvidenceCall>,
}

fn obj(pairs: Vec<(&str, serde_json::Value)>) -> serde_json::Map<String, serde_json::Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

struct EvidenceLog(Vec<EvidenceCall>);

impl EvidenceLog {
    fn push(&mut self, tool: &str, detail: String, source: &str) {
        self.0.push(EvidenceCall {
            n: self.0.len() as u32 + 1,
            tool: tool.to_string(),
            detail,
            source: source.to_string(),
        });
    }
}

#[tauri::command]
pub async fn mcp_meeting_brief(
    state: State<'_, AppState>,
    mcp_state: State<'_, McpState>,
) -> Result<MeetingBrief, String> {
    let mcp_client = mcp_state
        .client()
        .await
        .ok_or_else(|| "not logged in to the MCP Gateway yet".to_string())?;
    let sandbox = state
        .sandbox
        .lock()
        .expect("sandbox mutex poisoned")
        .clone()
        .ok_or_else(|| "not connected to a sandbox yet".to_string())?;
    let brief = build_meeting_brief(
        &mcp_client,
        &state.client,
        &sandbox,
        state.workspace.as_deref(),
        state.agent,
        state.environment.clone(),
    )
    .await?;
    // Best-effort: a brief that generated fine but failed to save to disk
    // should still reach the UI -- only `list_saved_briefs`/
    // `load_saved_brief` callers need to know persistence actually
    // worked, and they'll see an empty/missing result if it didn't.
    if let Err(err) = save_brief(&brief) {
        eprintln!("parrot-tauri: failed to save meeting brief to disk: {err}");
    }
    Ok(brief)
}

/// `$XDG_STATE_HOME/parrot-tauri/briefs/` (falling back to
/// `$HOME/.local/state` like the rest of the XDG base-dir spec) -- briefs
/// are per-persona the same way the OpenShell CLI's own
/// `$XDG_STATE_HOME`-scoped state already is, so alice's and bob's saved
/// briefs never collide even launched from the same machine (each demo
/// persona's `parrot.sh` invocation sets its own `XDG_STATE_HOME`).
fn briefs_dir() -> Result<std::path::PathBuf, String> {
    let state_home = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| std::path::PathBuf::from(home).join(".local").join("state"))
        })
        .ok_or_else(|| "neither XDG_STATE_HOME nor HOME is set".to_string())?;
    Ok(state_home.join("parrot-tauri").join("briefs"))
}

/// Keeps only characters safe in a filename -- `client_id`/`date` come
/// from tool-returned data, not arbitrary user text, but this is cheap
/// insurance against a stray `/` turning into a path traversal.
fn sanitize_filename_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Just the `YYYY-MM-DD` portion of an RFC 3339 `datetime` -- the key a
/// saved brief is recovered by, alongside `client_id`. Falls back to the
/// whole string if it doesn't contain a `T` (shouldn't happen for real
/// tool-returned datetimes, but keeps this infallible either way).
fn date_of(datetime: &str) -> &str {
    datetime.split('T').next().unwrap_or(datetime)
}

fn brief_path(client_id: &str, date: &str) -> Result<std::path::PathBuf, String> {
    Ok(briefs_dir()?.join(format!(
        "{}__{}.json",
        sanitize_filename_component(client_id),
        sanitize_filename_component(date)
    )))
}

fn save_brief(brief: &MeetingBrief) -> Result<(), String> {
    let path = brief_path(&brief.client_id, date_of(&brief.datetime))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(brief).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct SavedBriefSummary {
    client_id: String,
    client_name: String,
    date: String,
}

/// Lists every saved brief, newest meeting first -- used to populate the
/// sidebar's "Saved briefs" section. Corrupt/unreadable files are skipped
/// rather than failing the whole list, since this is a cache of past
/// generations, not critical state.
#[tauri::command]
pub fn list_saved_briefs() -> Result<Vec<SavedBriefSummary>, String> {
    let dir = briefs_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.to_string()),
    };

    let mut summaries: Vec<(String, SavedBriefSummary)> = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(brief) = serde_json::from_slice::<MeetingBrief>(&bytes) else {
            continue;
        };
        let date = date_of(&brief.datetime).to_string();
        summaries.push((
            brief.datetime.clone(),
            SavedBriefSummary {
                client_id: brief.client_id,
                client_name: brief.client_name,
                date,
            },
        ));
    }
    summaries.sort_by(|a, b| b.0.cmp(&a.0));
    // Sidebar only ever shows the 3 most recent — cap here rather than in
    // the UI so there's a single source of truth for "recent".
    Ok(summaries.into_iter().take(3).map(|(_, s)| s).collect())
}

/// Recovers one previously generated brief by exactly the key it was
/// saved under.
#[tauri::command]
pub fn load_saved_brief(client_id: String, date: String) -> Result<MeetingBrief, String> {
    let path = brief_path(&client_id, &date)?;
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("no saved brief for {client_id} on {date}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// The actual orchestration, kept separate from the `#[tauri::command]`
/// wrapper above so it can be exercised directly (see this crate's
/// development notes) without needing a running Tauri app/webview.
/// `exec_client`/`sandbox`/`workspace`/`agent`/`environment` are only used
/// for `request_brief_insights`'s one-shot LLM call -- deliberately no
/// `mcp_config` parameter here, since that call must never get tool
/// access (see this module's doc comment).
#[allow(clippy::too_many_arguments)]
pub async fn build_meeting_brief(
    client: &Arc<McpGatewayClient>,
    exec_client: &parrot_core::ParrotClient,
    sandbox: &str,
    workspace: Option<&str>,
    agent: parrot_core::Agent,
    environment: HashMap<String, String>,
) -> Result<MeetingBrief, String> {
    let mut evidence = EvidenceLog(Vec::new());

    let meetings_out = call(
        client,
        "mcp_crm_calendar_get_upcoming_meetings",
        obj(vec![]),
    )
    .await?;
    let meetings: Vec<serde_json::Value> = meetings_out
        .as_ref()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let meeting = meetings
        .first()
        .ok_or_else(|| "no upcoming meetings".to_string())?;
    let meeting_id = field_str(meeting, "id");
    let client_id = field_str(meeting, "client_id");
    let datetime = field_str(meeting, "datetime");
    evidence.push(
        "get_upcoming_meetings",
        format!("Next: {meeting_id} · {client_id}"),
        "CRM",
    );

    let (notes_r, positions_r, perf_mtd_r, perf_qtd_r, perf_ytd_r, risk_r, clients_r) = tokio::join!(
        client.call_tool(
            "mcp_crm_calendar_get_meeting_notes".to_string(),
            obj(vec![("meeting_id", meeting_id.clone().into())]),
        ),
        client.call_tool(
            "mcp_portfolio_get_positions".to_string(),
            obj(vec![("client_id", client_id.clone().into())]),
        ),
        client.call_tool(
            "mcp_portfolio_get_performance".to_string(),
            obj(vec![
                ("client_id", client_id.clone().into()),
                ("period", "MTD".into()),
            ]),
        ),
        client.call_tool(
            "mcp_portfolio_get_performance".to_string(),
            obj(vec![
                ("client_id", client_id.clone().into()),
                ("period", "QTD".into()),
            ]),
        ),
        client.call_tool(
            "mcp_portfolio_get_performance".to_string(),
            obj(vec![
                ("client_id", client_id.clone().into()),
                ("period", "YTD".into()),
            ]),
        ),
        client.call_tool(
            "mcp_kyc_compliance_get_risk_profile".to_string(),
            obj(vec![("client_id", client_id.clone().into())]),
        ),
        client.call_tool("mcp_portfolio_list_my_clients".to_string(), obj(vec![])),
    );

    let notes_out = parrot_core::unwrap_tool_output(&notes_r.map_err(|e| e.to_string())?);
    evidence.push("get_meeting_notes", meeting_id.clone(), "CRM");
    let notes = notes_out
        .as_ref()
        .and_then(|v| v.get("notes"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let positions_out = parrot_core::unwrap_tool_output(&positions_r.map_err(|e| e.to_string())?);
    let positions: Vec<serde_json::Value> = positions_out
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    evidence.push(
        "get_positions",
        format!("{client_id} · {} position(s)", positions.len()),
        "Portfolio",
    );

    let mut sector_totals: BTreeMap<String, f64> = BTreeMap::new();
    let mut total_value = 0.0_f64;
    let mut tickers: Vec<String> = Vec::new();
    for position in &positions {
        let sector = field_str(position, "sector");
        let value = position
            .get("market_value")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        *sector_totals.entry(sector).or_insert(0.0) += value;
        total_value += value;
        if let Some(ticker) = position.get("ticker").and_then(|v| v.as_str()) {
            tickers.push(ticker.to_string());
        }
    }
    let allocation: Vec<AllocationSlice> = sector_totals
        .iter()
        .map(|(sector, value)| AllocationSlice {
            sector: sector.clone(),
            pct: if total_value > 0.0 {
                value / total_value * 100.0
            } else {
                0.0
            },
        })
        .collect();
    let over_limit_sectors: Vec<String> = allocation
        .iter()
        .filter(|a| a.pct >= CONCENTRATION_LIMIT_PCT)
        .map(|a| a.sector.clone())
        .collect();

    evidence.push(
        "get_performance",
        format!("{client_id} · MTD, QTD, YTD"),
        "Portfolio",
    );
    let mut performance = Vec::new();
    for (period, result) in [
        ("MTD", perf_mtd_r),
        ("QTD", perf_qtd_r),
        ("YTD", perf_ytd_r),
    ] {
        let out = match result {
            Ok(call_result) => parrot_core::unwrap_tool_output(&call_result),
            // Confirmed live: a client can genuinely have no performance
            // data for a given period (too little history, etc.) — the
            // gateway returns a JSON-RPC error for that one period, not an
            // empty/zero result. Render it as "no data" rather than
            // failing the whole brief over one missing period.
            Err(_) => None,
        };
        performance.push(PerformancePeriod {
            period: period.to_string(),
            twr: out
                .as_ref()
                .and_then(|v| v.get("twr"))
                .and_then(|v| v.as_f64()),
            benchmark_twr: out
                .as_ref()
                .and_then(|v| v.get("benchmark_twr"))
                .and_then(|v| v.as_f64()),
        });
    }

    let risk_out = parrot_core::unwrap_tool_output(&risk_r.map_err(|e| e.to_string())?);
    evidence.push("get_risk_profile", client_id.clone(), "KYC");
    let risk_profile = risk_out
        .as_ref()
        .and_then(|v| v.get("risk_profile"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let kyc_status = risk_out
        .as_ref()
        .and_then(|v| v.get("kyc_status"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let pep_flag = risk_out
        .as_ref()
        .and_then(|v| v.get("pep_flag"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let clients_out = parrot_core::unwrap_tool_output(&clients_r.map_err(|e| e.to_string())?);
    evidence.push(
        "list_my_clients",
        "resolve client name".to_string(),
        "Portfolio",
    );
    let clients: Vec<serde_json::Value> = clients_out
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let matching_client = clients
        .iter()
        .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(client_id.as_str()));
    let client_name = matching_client
        .and_then(|c| c.get("name"))
        .and_then(|v| v.as_str())
        .map_or_else(|| client_id.clone(), str::to_string);
    let sector_focus = matching_client
        .and_then(|c| c.get("sector_focus"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let sectors_arg: Vec<serde_json::Value> = sector_focus
        .iter()
        .cloned()
        .map(serde_json::Value::String)
        .collect();
    let tickers_arg: Vec<serde_json::Value> = tickers
        .iter()
        .cloned()
        .map(serde_json::Value::String)
        .collect();
    let news_out = call(
        client,
        "mcp_market_news_get_relevant_news",
        obj(vec![
            ("tickers", serde_json::Value::Array(tickers_arg)),
            ("sectors", serde_json::Value::Array(sectors_arg)),
        ]),
    )
    .await?;
    evidence.push(
        "get_relevant_news",
        format!(
            "holdings tickers · {}",
            sector_focus.as_deref().unwrap_or("—")
        ),
        "Market news",
    );
    let news: Vec<NewsItem> = news_out
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .take(4)
        .map(|item| NewsItem {
            headline: field_str(&item, "headline"),
            ticker: item
                .get("ticker")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            sector: item
                .get("sector")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
        .collect();

    let mut suitability = Vec::new();
    for (product_id, product_name) in PRODUCTS {
        let out = call(
            client,
            "mcp_kyc_compliance_check_suitability",
            obj(vec![
                ("client_id", client_id.clone().into()),
                ("product_id", (*product_id).into()),
            ]),
        )
        .await?;
        evidence.push(
            "check_suitability",
            format!("{client_id} · {product_name}"),
            "KYC",
        );
        suitability.push(SuitabilityResult {
            product_name: (*product_name).to_string(),
            potentially_suitable: out
                .as_ref()
                .and_then(|v| v.get("potentially_suitable"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            explanation: out
                .as_ref()
                .and_then(|v| v.get("explanation"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }

    let regulatory_query = if over_limit_sectors.is_empty() {
        "product suitability"
    } else {
        "sector concentration limit"
    };
    let regulatory_out = call(
        client,
        "mcp_kyc_compliance_search_regulatory_guidance",
        obj(vec![("query", regulatory_query.into())]),
    )
    .await?;
    evidence.push(
        "search_regulatory_guidance",
        regulatory_query.to_string(),
        "KYC",
    );
    let regulatory = regulatory_out
        .and_then(|v| v.as_array().and_then(|arr| arr.first().cloned()))
        .map(|fragment| RegulatoryFragment {
            source: field_str(&fragment, "source"),
            text: excerpt_regulatory_text(&field_str(&fragment, "text")),
        });

    let insights = request_brief_insights(
        exec_client,
        sandbox,
        workspace,
        agent,
        environment,
        &client_name,
        &performance,
        &allocation,
        &over_limit_sectors,
        &suitability,
        notes.as_deref(),
    )
    .await;
    let (headline, talking_points) = match insights {
        Ok(insights) => (insights.headline, insights.talking_points),
        Err(err) => {
            eprintln!(
                "parrot-tauri: meeting-brief LLM insights failed, falling back to \
                 deterministic templating: {err}"
            );
            (
                build_headline(&client_name, &over_limit_sectors, &performance),
                build_talking_points(&over_limit_sectors, &allocation, &suitability, &performance),
            )
        }
    };

    Ok(MeetingBrief {
        meeting_id,
        client_id,
        client_name,
        datetime,
        risk_profile,
        kyc_status,
        pep_flag,
        sector_focus,
        notes,
        headline,
        performance,
        allocation,
        over_limit_sectors,
        news,
        suitability,
        regulatory,
        talking_points,
        evidence: evidence.0,
    })
}

async fn call(
    client: &Arc<McpGatewayClient>,
    tool_name: &str,
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let result = client
        .call_tool(tool_name.to_string(), arguments)
        .await
        .map_err(|e| e.to_string())?;
    Ok(parrot_core::unwrap_tool_output(&result))
}

/// One-shot, no-tools LLM call: hands the model exactly the facts already
/// fetched above and asks it to turn them into a headline + talking
/// points, as JSON matching [`BriefInsights`]. `mcp_config: None` on the
/// underlying turn means the model has no tools to call -- it can only
/// reason over what's in the prompt, so it can't introduce a claim that
/// doesn't trace back to a fact this function was given.
#[allow(clippy::too_many_arguments)]
async fn request_brief_insights(
    exec_client: &parrot_core::ParrotClient,
    sandbox: &str,
    workspace: Option<&str>,
    agent: parrot_core::Agent,
    environment: HashMap<String, String>,
    client_name: &str,
    performance: &[PerformancePeriod],
    allocation: &[AllocationSlice],
    over_limit_sectors: &[String],
    suitability: &[SuitabilityResult],
    notes: Option<&str>,
) -> Result<BriefInsights, String> {
    let facts = serde_json::json!({
        "client_name": client_name,
        "performance": performance,
        "allocation": allocation,
        "over_limit_sectors": over_limit_sectors,
        "suitability": suitability,
        "notes_from_last_meeting": notes,
    });
    let prompt = format!(
        "You are drafting the headline and talking points for a private banker's \
         pre-meeting brief. Use ONLY the facts in this JSON -- every one of them is \
         already verified; do not invent any number, product, or claim that isn't in \
         here:\n\n{facts}\n\n\
         Respond with ONLY a JSON object, no markdown code fence and no text before or \
         after it, matching exactly this shape:\n\
         {{\"headline\": \"one or two sentences summarizing the meeting\", \
         \"talking_points\": [{{\"title\": \"short title\", \"detail\": \"one sentence\"}}]}}\n\
         Produce 2 to 4 talking points, ordered by priority (sector concentration or \
         compliance flags first if present)."
    );

    // `evidence` is always empty here (no `mcp_config`, see this module's
    // doc comment) -- not surfaced in `BriefInsights`, nothing to merge.
    let result = parrot_core::run_structured_turn(
        exec_client,
        sandbox,
        workspace,
        agent,
        None,
        environment,
        &prompt,
    )
    .await
    .map_err(|e| e.to_string())?;

    serde_json::from_value(result.value).map_err(|e| e.to_string())
}

/// `search_regulatory_guidance` returns each hit's *entire* source
/// document as `text` (confirmed live — not a pre-trimmed snippet), which
/// is real data but too much to put in a sidebar card. Picks whichever
/// paragraph actually mentions the concentration-limit language (the
/// usual reason a brief cites this at all) over the generic opening
/// paragraph, and caps it — still real, quoted text, just shorter.
const REGULATORY_EXCERPT_MAX_CHARS: usize = 320;

fn excerpt_regulatory_text(text: &str) -> String {
    let paragraphs: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.starts_with('#'))
        .collect();
    let chosen = paragraphs
        .iter()
        .find(|p| p.contains("25%") || p.to_lowercase().contains("sector"))
        .or_else(|| paragraphs.first())
        .copied()
        .unwrap_or(text)
        .replace('\n', " ");

    let char_count = chosen.chars().count();
    if char_count > REGULATORY_EXCERPT_MAX_CHARS {
        let truncated: String = chosen.chars().take(REGULATORY_EXCERPT_MAX_CHARS).collect();
        format!("{truncated}…")
    } else {
        chosen
    }
}

fn field_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Simple, deterministic prose over numbers already fetched above — not
/// model-generated. Every claim here must trace to a field already on
/// `MeetingBrief`; if a future edit adds a claim it can't trace to, that's
/// a regression, not a style choice.
fn build_headline(
    client_name: &str,
    over_limit_sectors: &[String],
    performance: &[PerformancePeriod],
) -> String {
    let ytd = performance
        .iter()
        .find(|p| p.period == "YTD")
        .and_then(|p| p.twr.zip(p.benchmark_twr));
    let pace = match ytd {
        Some((twr, benchmark_twr)) if twr >= benchmark_twr => "A year ahead of benchmark",
        Some(_) => "A year behind benchmark",
        None => "A year with no performance data on file",
    };
    match over_limit_sectors.first() {
        Some(sector) => format!(
            "{pace} for {client_name}. The conversation to have is concentration: \
             {sector} is above the {CONCENTRATION_LIMIT_PCT:.0}% policy limit, so rebalancing \
             comes before any new idea in that sector."
        ),
        None => format!("{pace} for {client_name}, with no sector concentration concerns."),
    }
}

fn build_talking_points(
    over_limit_sectors: &[String],
    allocation: &[AllocationSlice],
    suitability: &[SuitabilityResult],
    performance: &[PerformancePeriod],
) -> Vec<TalkingPoint> {
    let mut points = Vec::new();

    if let Some(sector) = over_limit_sectors.first() {
        let pct = allocation
            .iter()
            .find(|a| &a.sector == sector)
            .map_or(0.0, |a| a.pct);
        points.push(TalkingPoint {
            title: format!("Trim {sector} to {CONCENTRATION_LIMIT_PCT:.0}%"),
            detail: format!(
                "{sector} is {pct:.0}% of the book, above the {CONCENTRATION_LIMIT_PCT:.0}% \
                 policy limit — bring it back inside policy before anything else."
            ),
        });
    }

    if let Some(fit) = suitability.iter().find(|s| s.potentially_suitable) {
        points.push(TalkingPoint {
            title: format!("Consider {}", fit.product_name),
            detail: fit.explanation.clone(),
        });
    }

    if let Some((twr, benchmark_twr)) = performance
        .iter()
        .find(|p| p.period == "YTD")
        .and_then(|p| p.twr.zip(p.benchmark_twr))
    {
        let ahead = twr >= benchmark_twr;
        points.push(TalkingPoint {
            title: "Walk through the year".to_string(),
            detail: format!(
                "{} benchmark year to date ({:+.1}% vs {:+.1}%).",
                if ahead { "Ahead of" } else { "Behind" },
                twr * 100.0,
                benchmark_twr * 100.0,
            ),
        });
    }

    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_unsafe_filename_characters() {
        assert_eq!(sanitize_filename_component("cli-004"), "cli-004");
        assert_eq!(
            sanitize_filename_component("../../etc/passwd"),
            "______etc_passwd"
        );
    }

    #[test]
    fn date_of_extracts_date_portion() {
        assert_eq!(date_of("2026-11-22T10:30:00Z"), "2026-11-22");
        assert_eq!(date_of("no-time-portion"), "no-time-portion");
    }

    /// Exercises the real recipe against a live cluster — not run in CI
    /// (needs a logged-in Keycloak user via Playwright, same as the demo's
    /// own headless-browser-automation docs). Run manually:
    ///
    /// ```sh
    /// cargo test -p parrot-tauri meeting_brief::tests::live -- --ignored --nocapture
    /// ```
    ///
    /// with `KEYCLOAK_HOST`, `MCP_GATEWAY_URL`, and `PLAYWRIGHT_LOGIN_SCRIPT`
    /// set (see `docs/headless-browser-automation.md` for the script), plus
    /// `SANDBOX_NAME`/`WORKSPACE` for the exec client `request_brief_insights`
    /// needs (defaults to alice's usual `claude-alice`/`alice`) — run with
    /// the same `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`SSL_CERT_FILE` env a real
    /// `parrot-tauri` launch for that workspace would use, so the OpenShell
    /// gateway/sandbox lookup below resolves the same way `main()` does.
    #[tokio::test]
    #[ignore]
    async fn live_build_meeting_brief_against_alice() {
        let host = std::env::var("KEYCLOAK_HOST").expect("set KEYCLOAK_HOST");
        let gateway_url = std::env::var("MCP_GATEWAY_URL").expect("set MCP_GATEWAY_URL");
        let script = std::env::var("PLAYWRIGHT_LOGIN_SCRIPT").expect("set PLAYWRIGHT_LOGIN_SCRIPT");
        let sandbox = std::env::var("SANDBOX_NAME").unwrap_or_else(|_| "claude-alice".to_string());
        let workspace = std::env::var("WORKSPACE").unwrap_or_else(|_| "alice".to_string());

        let config = parrot_core::McpAuthConfig {
            issuer: format!("https://{host}/realms/openshell"),
            client_id: "openshell-cli".to_string(),
            scopes: vec!["openid".to_string(), "profile".to_string()],
            insecure: false,
        };
        let pending = parrot_core::start_login(&config)
            .await
            .expect("start_login");
        let url = pending.authorize_url.clone();
        let playwright = tokio::spawn(async move {
            tokio::process::Command::new("node")
                .arg(script)
                .arg(url)
                .arg("alice")
                .arg("alice")
                .status()
                .await
        });
        let token = pending.complete().await.expect("complete login");
        playwright.await.expect("join").expect("playwright run");

        let client = Arc::new(
            McpGatewayClient::connect(&gateway_url, &token)
                .await
                .expect("connect to MCP Gateway"),
        );
        let exec_client = parrot_core::ParrotClient::connect(None, None)
            .await
            .expect("connect to OpenShell gateway");

        let brief = build_meeting_brief(
            &client,
            &exec_client,
            &sandbox,
            Some(workspace.as_str()),
            parrot_core::Agent::ClaudeCode,
            parrot_core::detect_passthrough_environment(),
        )
        .await
        .expect("build_meeting_brief");

        assert!(!brief.client_id.is_empty());
        assert!(!brief.evidence.is_empty());
        assert_eq!(brief.suitability.len(), PRODUCTS.len());
        assert_eq!(brief.performance.len(), 3);

        println!("client: {} ({})", brief.client_name, brief.client_id);
        println!("headline: {}", brief.headline);
        println!(
            "allocation: {:?}",
            brief
                .allocation
                .iter()
                .map(|a| (&a.sector, a.pct))
                .collect::<Vec<_>>()
        );
        println!("over_limit_sectors: {:?}", brief.over_limit_sectors);
        for point in &brief.talking_points {
            println!("talking point: {} — {}", point.title, point.detail);
        }
        println!("evidence calls: {}", brief.evidence.len());
    }
}
