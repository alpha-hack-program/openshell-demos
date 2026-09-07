// SPDX-License-Identifier: Apache-2.0

//! Placeholder GPUI frontend for parrot.
//!
//! Scope is intentionally minimal for now: prove `parrot-gpui` builds and
//! links against `parrot-core`'s types by holding a `SessionState`
//! internally, and render a static "Hello, parrot" window using the
//! shared palette. No tray/dashboard — that's future work.

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Render, SharedString,
    Window, WindowBounds, WindowOptions,
};
use parrot_core::{Palette, SessionState};

fn rgb_u32(color: parrot_core::Rgb) -> u32 {
    (u32::from(color.0) << 16) | (u32::from(color.1) << 8) | u32::from(color.2)
}

struct HelloParrot {
    greeting: SharedString,
    palette: Palette,
    // Held to prove the dependency wiring works; not rendered yet.
    #[allow(dead_code)]
    session: SessionState,
}

impl Render for HelloParrot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .size(px(480.0))
            .bg(rgb(rgb_u32(self.palette.background)))
            .text_color(rgb(rgb_u32(self.palette.foreground)))
            .text_xl()
            .child(self.greeting.clone())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(480.0), px(480.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| HelloParrot {
                    greeting: "Hello, parrot".into(),
                    palette: Palette::dark(),
                    session: SessionState::new(),
                })
            },
        )
        .expect("failed to open parrot-gpui window");
        cx.activate(true);
    });
}
