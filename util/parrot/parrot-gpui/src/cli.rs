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

/// Bounds for `--font-size`/`PARROT_FONT_SIZE`: below this, text is
/// unreadable; above it, the composer/log layout breaks down long before
/// gpui itself would complain.
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 96.0;

fn parse_font_size(value: &str) -> Result<f32, String> {
    let size: f32 = value
        .parse()
        .map_err(|_| format!("invalid font size '{value}' (expected a number)"))?;
    if !size.is_finite() || !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size) {
        return Err(format!(
            "font size '{value}' out of range (expected {MIN_FONT_SIZE}-{MAX_FONT_SIZE})"
        ));
    }
    Ok(size)
}

/// parrot-gpui — a native GUI frontend for streaming agent CLIs (Claude
/// Code, Codex) running inside OpenShell sandboxes.
///
/// Interactive by default: type a prompt, press Enter, watch it stream,
/// then type the next one. Pass `--prompt` for a one-shot run instead —
/// unlike `parrot-tui`'s `--prompt` (which never opens a persistent UI),
/// this still opens the window and submits the turn automatically as
/// soon as it's ready, so you can watch it stream; it does not auto-close
/// the window when the turn finishes.
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

    /// Submit this prompt automatically as soon as the window opens,
    /// instead of waiting for you to type one. The window stays open
    /// afterward for follow-up turns, same as a normal interactive
    /// session — this only skips typing the first prompt.
    #[arg(long)]
    pub prompt: Option<String>,

    /// Base font size in pixels, scaling all UI text uniformly (it drives
    /// gpui's rem size, and every text element here is sized in rems).
    /// Defaults to gpui's own default (16px) when unset.
    #[arg(long = "font-size", env = "PARROT_FONT_SIZE", value_parser = parse_font_size)]
    pub font_size: Option<f32>,
}
