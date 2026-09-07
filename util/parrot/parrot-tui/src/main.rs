// SPDX-License-Identifier: Apache-2.0

mod app;
mod cli;
mod splash;
mod theme_detect;

use std::io;

use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use parrot_core::{Palette, ParrotClient};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use cli::Cli;
use theme_detect::{detect_color_capability, resolve_theme_mode, TuiPalette};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let client = match ParrotClient::connect(
        cli.gateway.as_deref(),
        cli.gateway_endpoint.as_deref(),
    )
    .await
    {
        Ok(client) => client,
        Err(err) => {
            eprintln!("parrot: {err}");
            std::process::exit(1);
        }
    };

    let workspace = match client.resolve_workspace(cli.workspace.as_deref()).await {
        Ok(workspace) => workspace,
        Err(err) => {
            eprintln!("parrot: {err}");
            std::process::exit(1);
        }
    };

    let mut environment = parrot_core::detect_passthrough_environment();
    environment.extend(cli.env.iter().cloned());
    // Printed before the TUI takes over the terminal (raw mode + alternate
    // screen) specifically so a missing var is visible immediately, rather
    // than surfacing three layers downstream as a cryptic 403 from inside
    // the agent CLI once it's already streaming.
    if environment.is_empty() {
        eprintln!(
            "parrot: no environment variables passed through (none auto-detected, no --env given)"
        );
    } else {
        let mut keys: Vec<_> = environment.keys().cloned().collect();
        keys.sort();
        eprintln!("parrot: passing environment: {}", keys.join(", "));
    }

    if let Err(err) = run(cli, client, workspace, environment).await {
        eprintln!("parrot: {err}");
        std::process::exit(1);
    }
}

async fn run(
    cli: Cli,
    client: ParrotClient,
    workspace: Option<String>,
    environment: std::collections::HashMap<String, String>,
) -> io::Result<()> {
    let capability = detect_color_capability();
    let theme_mode = resolve_theme_mode(cli.theme);
    let palette = TuiPalette::new(Palette::for_mode(theme_mode), capability);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(
        &mut terminal,
        &cli,
        &client,
        workspace,
        environment,
        &palette,
    )
    .await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    cli: &Cli,
    client: &ParrotClient,
    workspace: Option<String>,
    environment: std::collections::HashMap<String, String>,
    palette: &TuiPalette,
) -> io::Result<()> {
    if !cli.no_splash {
        let greeting = client.identity.greeting();
        let info = splash::SplashInfo {
            greeting: &greeting,
            gateway_name: &client.gateway.name,
            gateway_endpoint: &client.gateway.endpoint,
        };
        splash::run_splash(terminal, palette, &info)?;
    }

    let options = app::RunOptions {
        agent: cli.agent,
        workspace,
        mcp_config: cli.mcp_config.clone(),
        environment,
        one_shot_prompt: cli.prompt.clone(),
    };

    app::run_dashboard(terminal, client, &cli.sandbox, options, palette).await
}
