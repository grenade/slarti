use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, SharedString, Window,
    WindowBounds, WindowOptions,
};

/// Minimal app model showcasing the current GPUI API.
/// TODO: Replace static text with terminal output and wire updates from a PTY reader thread.
struct AppModel {
    text: SharedString,
}

impl AppModel {
    fn new() -> Self {
        Self {
            text: "starting shell... (UI refactored to current GPUI API)".into(),
        }
    }
}

impl Render for AppModel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Keep UI simple: just render a full-size container with the text.
        // When wiring a terminal, replace this with a proper terminal grid and input handling.
        div()
            .flex()
            .items_start()
            .justify_start()
            .size_full()
            .p_4()
            .child(self.text.clone())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        // Create a centered window with reasonable default size.
        let bounds = Bounds::centered(None, size(px(900.0), px(600.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            // Root view factory
            |_, cx| cx.new(|_| AppModel::new()),
        )
        .unwrap();
    });
}
