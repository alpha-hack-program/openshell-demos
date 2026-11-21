// SPDX-License-Identifier: Apache-2.0

//! A direct MCP client against the keycloak-oidc demo's MCP Gateway
//! (`https://mcp.<cluster-apps-domain>/mcp`) — a second, independent
//! connection from [`crate::client::ParrotClient`]/the OpenShell gateway.
//! Authenticate first via [`crate::mcp_auth`], then [`McpGatewayClient::connect`]
//! with the resulting token.
//!
//! The gateway aggregates every registered MCP server behind one session:
//! `tools/list` returns every tool with its originating server's name
//! prefixed (`mcp_portfolio_get_positions`, `mcp_compatibility_calc_tax`,
//! ...), stripped before the gateway forwards the call — see
//! `demos/keycloak-oidc/mcp-servers/README.md`'s "RHCL MCP Gateway
//! integration" section. [`list_tools_grouped`] splits that back apart
//! client-side so a frontend's toolbar can group by server, using a
//! caller-supplied list of known server names (there is no generic way to
//! discover that list purely from `tools/list`, since server names
//! themselves contain underscores — `mcp_crm_calendar`,
//! `mcp_market_news` — making the prefix ambiguous without it).

use std::collections::BTreeMap;

use rmcp::model::{CallToolRequestParams, CallToolResult, ClientInfo, RawContent, Tool};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;

use crate::error::{ParrotError, Result};
use crate::mcp_auth::McpToken;

/// One backend MCP server's name as registered with the gateway (e.g.
/// `mcp-portfolio`) — the chart's own naming convention, hyphens included;
/// [`list_tools_grouped`] converts to the gateway's underscore-prefix form
/// internally.
pub type McpServerName = String;

pub struct McpGatewayClient {
    peer: rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>,
}

impl McpGatewayClient {
    /// Opens one MCP session against `gateway_url` (e.g.
    /// `https://mcp.apps.example.com/mcp`), authenticated with `token`.
    pub async fn connect(gateway_url: &str, token: &McpToken) -> Result<Self> {
        let http_client = reqwest::Client::builder()
            .build()
            .map_err(|e| ParrotError::Mcp(format!("failed to build HTTP client: {e}")))?;
        let config = StreamableHttpClientTransportConfig::with_uri(gateway_url.to_string())
            .auth_header(token.access_token.clone());
        let transport = StreamableHttpClientTransport::with_client(http_client, config);

        let peer = ClientInfo::default()
            .serve(transport)
            .await
            .map_err(|e| ParrotError::Mcp(format!("MCP Gateway connect failed: {e}")))?;

        Ok(Self { peer })
    }

    /// Every tool the gateway's current authorization lets this token see
    /// — inherently per-user already, since that filtering happens
    /// server-side (Envoy RBAC / CEL authz over `resource_access` roles).
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        self.peer
            .list_all_tools()
            .await
            .map_err(|e| ParrotError::Mcp(format!("tools/list failed: {e}")))
    }

    /// Groups `tools` by which registered server (from `known_servers`,
    /// e.g. `["mcp-compatibility", "mcp-portfolio", ...]`) produced them,
    /// via the gateway's `<name-with-underscores>_` prefix convention.
    /// Tools matching no known server's prefix land under the empty-string
    /// key rather than being dropped, so a stale `known_servers` list
    /// (a server registered after this list was last updated) still shows
    /// up somewhere instead of silently vanishing.
    pub fn group_by_server(
        tools: Vec<Tool>,
        known_servers: &[McpServerName],
    ) -> BTreeMap<McpServerName, Vec<Tool>> {
        let prefixes: Vec<(String, &McpServerName)> = known_servers
            .iter()
            .map(|name| (format!("{}_", name.replace('-', "_")), name))
            .collect();

        let mut grouped: BTreeMap<McpServerName, Vec<Tool>> = BTreeMap::new();
        for tool in tools {
            // Longest-prefix-first avoids a short server name's prefix
            // (rare in this demo, but not guaranteed in general)
            // shadowing a longer one that also matches.
            let server = prefixes
                .iter()
                .filter(|(prefix, _)| tool.name.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(_, name)| (*name).clone())
                .unwrap_or_default();
            grouped.entry(server).or_default().push(tool);
        }
        grouped
    }

    /// Calls `tool_name` (the gateway's prefixed name, exactly as returned
    /// by `tools/list`) with `arguments`, already validated against that
    /// tool's `input_schema` by the caller (the generated playground form).
    pub async fn call_tool(
        &self,
        tool_name: String,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<rmcp::model::CallToolResult> {
        let params = CallToolRequestParams::new(tool_name).with_arguments(arguments);
        self.peer
            .call_tool(params)
            .await
            .map_err(|e| ParrotError::Mcp(format!("tools/call failed: {e}")))
    }
}

/// Unwraps this demo's MCP servers' common result envelope —
/// `{"output": ..., "called_by": "...", "roles": [...]}`, JSON-encoded
/// inside the tool result's first text content block (confirmed live
/// against every server in this demo) — returning just `output`. Falls
/// back to the whole parsed value for a result that isn't shaped this way
/// (a different server, or a future change here) rather than dropping it,
/// and to `None` only when there's no text content at all or it isn't
/// valid JSON.
pub fn unwrap_tool_output(result: &CallToolResult) -> Option<serde_json::Value> {
    let text = result
        .content
        .iter()
        .find_map(|content| match &content.raw {
            RawContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(
        parsed
            .as_object()
            .and_then(|obj| obj.get("output").cloned())
            .unwrap_or(parsed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> Tool {
        Tool::new_with_raw(name.to_string(), None, serde_json::Map::new())
    }

    #[test]
    fn groups_tools_by_known_server_prefix() {
        let tools = vec![
            tool("mcp_portfolio_get_positions"),
            tool("mcp_portfolio_list_my_clients"),
            tool("mcp_compatibility_calc_tax"),
            tool("mcp_market_news_get_relevant_news"),
        ];
        let known = vec![
            "mcp-portfolio".to_string(),
            "mcp-compatibility".to_string(),
            "mcp-market-news".to_string(),
        ];

        let grouped = McpGatewayClient::group_by_server(tools, &known);

        assert_eq!(grouped["mcp-portfolio"].len(), 2);
        assert_eq!(grouped["mcp-compatibility"].len(), 1);
        assert_eq!(grouped["mcp-market-news"].len(), 1);
    }

    #[test]
    fn tools_matching_no_known_server_land_in_the_fallback_bucket() {
        let tools = vec![tool("mcp_kyc_compliance_get_risk_profile")];
        let known = vec!["mcp-portfolio".to_string()];

        let grouped = McpGatewayClient::group_by_server(tools, &known);

        assert_eq!(grouped[""].len(), 1);
    }

    #[test]
    fn longest_matching_prefix_wins_over_a_shorter_ambiguous_one() {
        let tools = vec![tool("mcp_crm_calendar_get_upcoming_meetings")];
        // "mcp-crm" would also match as a prefix of "mcp_crm_calendar_...";
        // the real, longer registration must win.
        let known = vec!["mcp-crm".to_string(), "mcp-crm-calendar".to_string()];

        let grouped = McpGatewayClient::group_by_server(tools, &known);

        assert_eq!(grouped["mcp-crm-calendar"].len(), 1);
        assert!(!grouped.contains_key("mcp-crm"));
    }
}
