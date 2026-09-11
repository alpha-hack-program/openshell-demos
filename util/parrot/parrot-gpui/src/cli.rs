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

/// parrot-gpui — a native GUI frontend for streaming agent CLIs (Claude
/// Code, Codex) running inside OpenShell sandboxes.
///
/// Always interactive (no one-shot `--prompt` mode — that's `parrot-tui`'s
/// job): type a prompt, press Enter, watch it stream, then type the next
/// one.
#[derive(Debug, Parser)]
#[command(name = "parrot-gpui", version)]
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

    /// Color theme.
    #[arg(long, value_parser = parse_theme, default_value = "system")]
    pub theme: ThemeMode,

    /// Skip the splash screen.
    #[arg(long)]
    pub no_splash: bool,
}
