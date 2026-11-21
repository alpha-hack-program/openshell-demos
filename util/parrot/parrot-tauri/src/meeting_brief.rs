// SPDX-License-Identifier: Apache-2.0

//! The "Meeting Brief" feature: a deterministic report for the banker's
//! next upcoming meeting, assembled from a fixed recipe of direct MCP
//! tool calls — no LLM, no agent loop. This is the thing the
//! "Meridian Advisor Desk" mockup calls the copilot's output, but it's
//! really just orchestration: every number and verdict here is exactly
//! what the matching tool returned, with simple arithmetic (sector
//! concentration %, ahead/behind benchmark) on top — nothing invented.
//! The chat pane (main.rs) is the place for genuinely open-ended
//! questions; this is for the one specific, well-known report a banker
//! asks for dozens of times a day.

use std::collections::BTreeMap;
use std::sync::Arc;

use parrot_core::McpGatewayClient;
use tauri::State;

use crate::mcp::McpState;

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
#[derive(Clone, serde::Serialize)]
pub struct PerformancePeriod {
    period: String,
    twr: Option<f64>,
    benchmark_twr: Option<f64>,
}

#[derive(Clone, serde::Serialize)]
pub struct AllocationSlice {
    sector: String,
    pct: f64,
}

#[derive(Clone, serde::Serialize)]
pub struct NewsItem {
    headline: String,
    ticker: Option<String>,
    sector: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct SuitabilityResult {
    product_name: String,
    potentially_suitable: bool,
    explanation: String,
}

#[derive(Clone, serde::Serialize)]
pub struct RegulatoryFragment {
    source: String,
    text: String,
}

#[derive(Clone, serde::Serialize)]
pub struct TalkingPoint {
    title: String,
    detail: String,
}

#[derive(Clone, serde::Serialize)]
pub struct EvidenceCall {
    n: u32,
    tool: String,
    detail: String,
    source: String,
}

#[derive(serde::Serialize)]
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
pub async fn mcp_meeting_brief(state: State<'_, McpState>) -> Result<MeetingBrief, String> {
    let client = state
        .client()
        .await
        .ok_or_else(|| "not logged in to the MCP Gateway yet".to_string())?;
    build_meeting_brief(&client).await
}

/// The actual orchestration, kept separate from the `#[tauri::command]`
/// wrapper above so it can be exercised directly (see this crate's
/// development notes) without needing a running Tauri app/webview.
pub async fn build_meeting_brief(client: &Arc<McpGatewayClient>) -> Result<MeetingBrief, String> {
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

    let talking_points =
        build_talking_points(&over_limit_sectors, &allocation, &suitability, &performance);
    let headline = build_headline(&client_name, &over_limit_sectors, &performance);

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

    /// Exercises the real recipe against a live cluster — not run in CI
    /// (needs a logged-in Keycloak user via Playwright, same as the demo's
    /// own headless-browser-automation docs). Run manually:
    ///
    /// ```sh
    /// cargo test -p parrot-tauri meeting_brief::tests::live -- --ignored --nocapture
    /// ```
    ///
    /// with `KEYCLOAK_HOST`, `MCP_GATEWAY_URL`, and `PLAYWRIGHT_LOGIN_SCRIPT`
    /// set (see `docs/headless-browser-automation.md` for the script).
    #[tokio::test]
    #[ignore]
    async fn live_build_meeting_brief_against_alice() {
        let host = std::env::var("KEYCLOAK_HOST").expect("set KEYCLOAK_HOST");
        let gateway_url = std::env::var("MCP_GATEWAY_URL").expect("set MCP_GATEWAY_URL");
        let script = std::env::var("PLAYWRIGHT_LOGIN_SCRIPT").expect("set PLAYWRIGHT_LOGIN_SCRIPT");

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

        let brief = build_meeting_brief(&client)
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
