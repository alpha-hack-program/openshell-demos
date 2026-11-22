// SPDX-License-Identifier: Apache-2.0

//! Tauri commands for the MCP Gateway toolbar/playground — Phase 1 only
//! (login + tool discovery; `tools/call` wiring lands with the
//! JSON-Schema-driven playground). See `parrot_core::mcp_auth`/
//! `parrot_core::mcp_client` for the actual OAuth/MCP logic this wraps;
//! this module is just the Tauri glue. Entirely separate from `main.rs`'s
//! `AppState`/chat pane — the MCP Gateway is optional and unconfigured
//! launches still work for chat.

use std::sync::Arc;

use parrot_core::{CallToolResult, McpAuthConfig, McpGatewayClient, McpServerName, McpTool};
use tauri::State;
use tokio::sync::Mutex;

pub struct McpState {
    gateway_url: Option<String>,
    auth_config: Option<McpAuthConfig>,
    known_servers: Vec<McpServerName>,
    client: Mutex<Option<Arc<McpGatewayClient>>>,
    // `McpGatewayClient` itself doesn't carry the identity (it's derived
    // from the login token, not the MCP session) -- cached here so
    // `mcp_login`'s already-connected short-circuit can still answer it.
    identity: Mutex<Option<McpIdentity>>,
}

impl McpState {
    pub fn new(
        gateway_url: Option<String>,
        auth_config: Option<McpAuthConfig>,
        known_servers: Vec<McpServerName>,
    ) -> Self {
        Self {
            gateway_url,
            auth_config,
            known_servers,
            client: Mutex::new(None),
            identity: Mutex::new(None),
        }
    }

    pub fn configured(&self) -> bool {
        self.gateway_url.is_some() && self.auth_config.is_some()
    }

    /// The connected session, if `mcp_login` has already succeeded — used
    /// by every command below (and by `meeting_brief.rs`) instead of each
    /// locking/cloning `self.client` by hand.
    pub async fn client(&self) -> Option<Arc<McpGatewayClient>> {
        self.client.lock().await.clone()
    }
}

/// One server's slice of `tools/list`, already split apart by
/// [`parrot_core::McpGatewayClient::group_by_server`] — `server` is empty
/// for tools matching none of `--mcp-server`'s known names.
#[derive(serde::Serialize)]
pub struct McpToolGroup {
    pub server: String,
    pub tools: Vec<McpTool>,
}

/// Opens `url` in the system browser — deliberately not the
/// `tauri-plugin-opener` crate, since parrot-tauri's one confirmed target
/// so far is macOS (the whole reason it exists over parrot-gpui); the
/// Linux/Windows arms are best-effort, unverified on either.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn();
    }
}

/// The identity the *MCP Gateway* login resolved — distinct from
/// `AppState`'s own `identity_greeting` (the OpenShell-gateway session,
/// which may be a different login entirely; the two happen to be the same
/// demo persona — alice/bob/charlie — but nothing guarantees that in
/// general). The header updates to this once MCP login succeeds, since
/// that's the identity "your book only" actually refers to.
#[derive(Clone, serde::Serialize)]
pub struct McpIdentity {
    pub display_name: Option<String>,
    /// The stable Keycloak login username (`preferred_username`), distinct
    /// from `display_name` (which prefers the human-readable `name` claim)
    /// -- the splash screen's Connect flow uses this, not `display_name`,
    /// to compute `{sandbox_prefix}{agent}-{preferred_username}`.
    pub preferred_username: Option<String>,
}

#[tauri::command]
pub async fn mcp_login(state: State<'_, McpState>) -> Result<McpIdentity, String> {
    // A webview reload (--debug's right-click Reload) only wipes the
    // frontend's JS state, not this backend's -- it re-renders the
    // disconnected UI and re-triggers a login click even though `state`
    // already holds a live, still-authenticated client from before the
    // reload. Without this short-circuit, that re-click re-runs the full
    // OAuth dance (and, with `prompt=login` below, a real re-entry of
    // credentials) for no reason.
    if state.client.lock().await.is_some() {
        if let Some(identity) = state.identity.lock().await.clone() {
            return Ok(identity);
        }
    }

    let gateway_url = state
        .gateway_url
        .clone()
        .ok_or_else(|| "MCP Gateway not configured (pass --mcp-gateway-url)".to_string())?;
    let auth_config = state
        .auth_config
        .clone()
        .ok_or_else(|| "Keycloak not configured (pass --keycloak-host)".to_string())?;

    let pending = parrot_core::start_login(&auth_config)
        .await
        .map_err(|e| e.to_string())?;
    open_browser(&pending.authorize_url);
    let token = pending.complete().await.map_err(|e| e.to_string())?;
    let identity = parrot_core::identity_from_access_token(&token.access_token);
    let preferred_username = parrot_core::preferred_username_from_access_token(&token.access_token);

    let client = McpGatewayClient::connect(&gateway_url, &token)
        .await
        .map_err(|e| e.to_string())?;
    *state.client.lock().await = Some(Arc::new(client));
    let identity = McpIdentity {
        display_name: identity.display_name,
        preferred_username,
    };
    *state.identity.lock().await = Some(identity.clone());
    Ok(identity)
}

#[tauri::command]
pub async fn mcp_list_tools(state: State<'_, McpState>) -> Result<Vec<McpToolGroup>, String> {
    let client = state
        .client
        .lock()
        .await
        .clone()
        .ok_or_else(|| "not logged in to the MCP Gateway yet".to_string())?;
    let tools = client.list_tools().await.map_err(|e| e.to_string())?;
    let grouped = McpGatewayClient::group_by_server(tools, &state.known_servers);
    Ok(grouped
        .into_iter()
        .map(|(server, tools)| McpToolGroup { server, tools })
        .collect())
}

/// Invokes `tool_name` (the gateway's prefixed name, exactly as returned by
/// `mcp_list_tools`) with `arguments` — already collected and type-coerced
/// by the playground's generated form (`ui/schema-form.js`) against that
/// tool's `inputSchema`, so no further validation happens here.
#[tauri::command]
pub async fn mcp_call_tool(
    tool_name: String,
    arguments: serde_json::Value,
    state: State<'_, McpState>,
) -> Result<CallToolResult, String> {
    let client = state
        .client
        .lock()
        .await
        .clone()
        .ok_or_else(|| "not logged in to the MCP Gateway yet".to_string())?;
    let arguments = match arguments {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Null => serde_json::Map::new(),
        _ => return Err("tool arguments must be a JSON object".to_string()),
    };
    client
        .call_tool(tool_name, arguments)
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Deserialize)]
struct RawMeeting {
    id: String,
    client_id: String,
    datetime: String,
}

#[derive(serde::Deserialize)]
struct RawClient {
    id: String,
    name: String,
    risk_profile: Option<String>,
    kyc_status: Option<String>,
    pep_flag: Option<bool>,
    sector_focus: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TodayMeeting {
    id: String,
    client_id: String,
    client_name: Option<String>,
    datetime: String,
}

#[derive(serde::Serialize)]
pub struct TodayClient {
    id: String,
    name: String,
    risk_profile: Option<String>,
    kyc_status: Option<String>,
    pep_flag: bool,
    sector_focus: Option<String>,
}

#[derive(serde::Serialize)]
pub struct Today {
    meetings: Vec<TodayMeeting>,
    clients: Vec<TodayClient>,
}

/// The "Today" rail's data: the banker's upcoming meetings (joined with
/// client names) and client book, fetched directly via two tool calls —
/// deliberately not routed through chat/the agent at all, so it's there
/// before the operator asks anything, exactly like a native app's own
/// dashboard rather than something the LLM "decided" to show.
#[tauri::command]
pub async fn mcp_today(state: State<'_, McpState>) -> Result<Today, String> {
    let client = state
        .client
        .lock()
        .await
        .clone()
        .ok_or_else(|| "not logged in to the MCP Gateway yet".to_string())?;

    let (meetings_result, clients_result) = tokio::join!(
        client.call_tool(
            "mcp_crm_calendar_get_upcoming_meetings".to_string(),
            serde_json::Map::new(),
        ),
        client.call_tool(
            "mcp_portfolio_list_my_clients".to_string(),
            serde_json::Map::new(),
        ),
    );
    let meetings_result = meetings_result.map_err(|e| e.to_string())?;
    let clients_result = clients_result.map_err(|e| e.to_string())?;

    let raw_meetings: Vec<RawMeeting> = parrot_core::unwrap_tool_output(&meetings_result)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let raw_clients: Vec<RawClient> = parrot_core::unwrap_tool_output(&clients_result)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let meetings = raw_meetings
        .into_iter()
        .map(|m| {
            let client_name = raw_clients
                .iter()
                .find(|c| c.id == m.client_id)
                .map(|c| c.name.clone());
            TodayMeeting {
                id: m.id,
                client_id: m.client_id,
                client_name,
                datetime: m.datetime,
            }
        })
        .collect();

    let clients = raw_clients
        .into_iter()
        .map(|c| TodayClient {
            id: c.id,
            name: c.name,
            risk_profile: c.risk_profile,
            kyc_status: c.kyc_status,
            pep_flag: c.pep_flag.unwrap_or(false),
            sector_focus: c.sector_focus,
        })
        .collect();

    Ok(Today { meetings, clients })
}
