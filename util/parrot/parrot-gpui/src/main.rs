// SPDX-License-Identifier: Apache-2.0

mod app;
mod cli;

use std::sync::Arc;

use clap::Parser;
use gpui::{
    px, size, App, AppContext as _, Application, Bounds, WindowAppearance, WindowBounds,
    WindowOptions,
};
use parrot_core::{detect_passthrough_environment, Palette, ParrotClient, ThemeMode};
use tokio::runtime::Runtime;

use app::ParrotWindow;
use cli::Cli;

fn resolve_palette(theme: ThemeMode, appearance: WindowAppearance) -> Palette {
    match theme {
        ThemeMode::Light => Palette::light(),
        ThemeMode::Dark => Palette::dark(),
        ThemeMode::System => match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Palette::light(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Palette::dark(),
        },
    }
}

fn main() {
    let cli = Cli::parse();

    // `parrot-core`'s client/exec plumbing is Tokio-based; gpui's own
    // executor doesn't drive it. Connect synchronously here (same
    // fail-fast behavior as `parrot-tui`'s main), then keep the runtime
    // alive for the app's lifetime so later turns can be spawned onto it.
    let runtime = Arc::new(Runtime::new().expect("failed to start tokio runtime"));
    let (client, workspace) = runtime
        .block_on(async {
            let client =
                ParrotClient::connect(cli.gateway.as_deref(), cli.gateway_endpoint.as_deref())
                    .await?;
            let workspace = client.resolve_workspace(cli.workspace.as_deref()).await?;
            Ok::<_, parrot_core::ParrotError>((client, workspace))
        })
        .unwrap_or_else(|err| {
            eprintln!("parrot-gpui: {err}");
            std::process::exit(1);
        });

    let mut environment = detect_passthrough_environment();
    environment.extend(cli.env.iter().cloned());
    if environment.is_empty() {
        eprintln!(
            "parrot-gpui: no environment variables passed through (none auto-detected, no --env given)"
        );
    } else {
        let mut keys: Vec<_> = environment.keys().cloned().collect();
        keys.sort();
        eprintln!("parrot-gpui: passing environment: {}", keys.join(", "));
    }

    let client = Arc::new(client);
    let theme = cli.theme;

    Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(900.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let palette = resolve_palette(theme, window.appearance());
                cx.new(|cx| {
                    ParrotWindow::new(
                        &cli,
                        client,
                        workspace,
                        environment,
                        runtime,
                        palette,
                        window,
                        cx,
                    )
                })
            },
        )
        .expect("failed to open parrot-gpui window");
        cx.activate(true);
    });
}
