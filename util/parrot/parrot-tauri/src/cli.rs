// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use parrot_core::{Agent, ThemeMode};

fn parse_theme(value: &str) -> Result<ThemeMode, String> {
    match value {
        "light" => Ok(ThemeMode::Light),
        "dark" => Ok(ThemeMode::Dark),
        "system" => Ok(ThemeMode::System),
        other => Err(format!(
            "invalid theme '{other}' (expected light|dark|system)"
        )),
    }
}

fn parse_env_var(value: &str) -> Result<(String, String), String> {
    match value.split_once('=') {
        Some((key, val)) if !key.is_empty() => Ok((key.to_string(), val.to_string())),
        _ => Err(format!(
            "invalid --env value '{value}' (expected KEY=VALUE)"
        )),
    }
}

fn parse_agent(value: &str) -> Result<Agent, String> {
    Agent::parse(value).ok_or_else(|| format!("invalid agent '{value}' (expected claude|codex)"))
}

/// parrot-tauri — a desktop (Tauri/WebView) frontend for streaming agent
/// CLIs (Claude Code, Codex) running inside OpenShell sandboxes.
///
/// Same connect-by-flags model as `parrot-tui`/`parrot-gpui`: there is no
/// in-app login/picker screen, so `--sandbox` (and `--gateway`/`--workspace`
/// when ambiguous) must be given at launch, same as the other two frontends.
/// Interactive by default: type a prompt, press Enter, watch it stream,
/// then type the next one. Pass `--prompt` to also submit one automatically
/// as soon as the window opens (the window stays open afterward, same as
/// `parrot-gpui`'s `--prompt`).
#[derive(Debug, Parser)]
#[command(name = "parrot-tauri", version)]
pub struct Cli {
    /// Gateway name to connect to. Defaults to the active gateway.
    #[arg(short = 'g', long = "gateway", env = "OPENSHELL_GATEWAY")]
    pub gateway: Option<String>,

    /// Gateway endpoint URL, bypassing name-based lookup for the endpoint
    /// itself (stored metadata, if resolvable, is still used for auth).
    #[arg(long = "gateway-endpoint", env = "OPENSHELL_GATEWAY_ENDPOINT")]
    pub gateway_endpoint: Option<String>,

    /// Sandbox to run the agent inside.
    #[arg(short = 's', long = "sandbox")]
    pub sandbox: String,

    /// Workspace the sandbox lives in. Auto-detected when you're only a
    /// member of one workspace (the common case); required via this flag
    /// when you belong to more than one.
    #[arg(short = 'w', long = "workspace")]
    pub workspace: Option<String>,

    /// Which agent to drive.
    #[arg(long, value_parser = parse_agent, default_value = "claude")]
    pub agent: Agent,

    /// Submit this prompt automatically as soon as the window opens,
    /// instead of waiting for you to type one.
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,

    /// Override the agent's default MCP config path. Only needed if your
    /// sandbox doesn't use the demo's usual layout.
    #[arg(long = "mcp-config")]
    pub mcp_config: Option<String>,

    /// Extra or overriding environment variable for the exec, as
    /// KEY=VALUE. Repeatable. `ANTHROPIC_BASE_URL`/`ANTHROPIC_MODEL`/etc.
    /// are picked up automatically from your shell if already set — this
    /// is only for overrides or anything not auto-detected.
    #[arg(long = "env", value_parser = parse_env_var)]
    pub env: Vec<(String, String)>,

    /// Color theme. "system" is resolved against the OS appearance in the
    /// webview via a `prefers-color-scheme` media query, not queried here.
    #[arg(long, value_parser = parse_theme, default_value = "system")]
    pub theme: ThemeMode,

    /// Skip the splash screen.
    #[arg(long)]
    pub no_splash: bool,

    /// MCP Gateway endpoint for the tool playground (e.g.
    /// `https://mcp.<cluster-apps-domain>/mcp` — see
    /// `demos/keycloak-oidc/mcp-servers/README.md`'s "RHCL MCP Gateway
    /// integration"). The left toolbar/playground stay unavailable (not a
    /// startup failure — the chat pane doesn't need this) until this,
    /// `--keycloak-host`, and a login are all in place.
    #[arg(long = "mcp-gateway-url", env = "MCP_GATEWAY_URL")]
    pub mcp_gateway_url: Option<String>,

    /// Keycloak host for the MCP Gateway login (Authorization Code + PKCE
    /// against this realm's `openshell-cli` client — same client
    /// `openshell gateway login` itself uses). Matches the demo's own
    /// `.env`'s `KEYCLOAK_HOST`.
    #[arg(long = "keycloak-host", env = "KEYCLOAK_HOST")]
    pub keycloak_host: Option<String>,

    #[arg(
        long = "keycloak-realm",
        env = "KEYCLOAK_REALM",
        default_value = "openshell"
    )]
    pub keycloak_realm: String,

    #[arg(
        long = "keycloak-client-id",
        env = "KEYCLOAK_CLIENT_ID_CLI",
        default_value = "openshell-cli"
    )]
    pub keycloak_client_id: String,

    /// Skip TLS verification for the Keycloak login — self-signed dev
    /// clusters only.
    #[arg(long = "mcp-insecure")]
    pub mcp_insecure: bool,

    /// Known MCP server names registered with the gateway (hyphenated, as
    /// in the Helm chart's `values.yaml`), used to group `tools/list`'s
    /// flat, prefixed results back into a per-server toolbar — see
    /// `parrot_core::mcp_client`'s doc comment for why this can't be
    /// derived generically from the tool list alone. Defaults to the
    /// keycloak-oidc demo's own five servers; override for a different
    /// deployment.
    #[arg(
        long = "mcp-server",
        default_values = [
            "mcp-compatibility",
            "mcp-portfolio",
            "mcp-crm-calendar",
            "mcp-market-news",
            "mcp-kyc-compliance",
        ]
    )]
    pub mcp_servers: Vec<String>,
}
