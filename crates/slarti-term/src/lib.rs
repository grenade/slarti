use gpui::{
    div, prelude::*, px, relative, App, Bounds, Context, FocusHandle, Focusable, LayoutId,
    MouseButton, MouseUpEvent, SharedString, Style, Window, WindowBounds, WindowOptions,
};

/// Theme colors for the terminal panel.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Foreground HSLA (h, s, l, a) with each component in [0.0, 1.0]
    pub fg: (f32, f32, f32, f32),
    /// Background HSLA (h, s, l, a) with each component in [0.0, 1.0]
    pub bg: (f32, f32, f32, f32),
    /// Cursor HSLA (h, s, l, a) with each component in [0.0, 1.0]
    pub cursor: (f32, f32, f32, f32),
}

impl Theme {
    /// Default: light text on dark background, blue-ish cursor.
    pub fn default_dark() -> Self {
        Self {
            fg: (0.0, 0.0, 1.0, 1.0),      // white
            bg: (0.0, 0.0, 0.05, 1.0),     // near-black
            cursor: (0.66, 1.0, 0.5, 1.0), // blue-ish
        }
    }
}

/// Configuration for the terminal panel.
#[derive(Clone, Debug)]
pub struct TerminalConfig {
    /// Panel title to display in the header.
    pub title: SharedString,
    /// Initial collapsed state.
    pub collapsed: bool,
    /// Theme to use for the panel and fallback text/cursor colors.
    pub theme: Theme,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            title: "Terminal".into(),
            collapsed: false,
            theme: Theme::default_dark(),
        }
    }
}

/// A collapsible terminal panel. This minimal version intentionally does not
/// embed the terminal engine yet so the workspace builds while we finalize
/// the engine migration. The public API is kept stable.
pub struct TerminalView {
    focus: FocusHandle,
    title: SharedString,
    collapsed: bool,
    theme: Theme,
}

impl TerminalView {
    /// Construct a new `TerminalView`.
    pub fn new(cx: &mut Context<Self>, config: TerminalConfig) -> Self {
        Self {
            focus: cx.focus_handle(),
            title: config.title,
            collapsed: config.collapsed,
            theme: config.theme,
        }
    }

    fn on_toggle(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        cx.notify();
    }

    /// Forward input bytes (e.g. typed characters or escape sequences) to the terminal.
    ///
    /// Minimal placeholder: this implementation intentionally does nothing to keep
    /// the API stable while the engine is temporarily removed.
    pub fn write_bytes(&self, _bytes: &[u8]) {
        // no-op for now
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let bg = gpui::hsla(theme.bg.0, theme.bg.1, theme.bg.2, theme.bg.3);
        let fg = gpui::hsla(theme.fg.0, theme.fg.1, theme.fg.2, theme.fg.3);

        // Header: left placeholder menu, centered title
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(28.))
            .px(px(8.))
            .bg(bg)
            .child(
                // Left: collapsible menu placeholder
                div()
                    .w(px(80.))
                    .h(px(20.))
                    .rounded_sm()
                    .border_1()
                    .border_color(gpui::opaque_grey(0.2, 0.7))
                    .cursor_default()
                    .child("≡"),
            )
            .child(
                // Center: title
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .text_color(fg)
                    .child(self.title.clone()),
            );

        // Content: displays a placeholder while the engine is not yet wired.
        let content = div()
            .flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .when(!self.collapsed, |d| {
                d.child(
                    div()
                        .flex()
                        .size_full()
                        .px(px(12.))
                        .py(px(8.))
                        .child("Terminal (engine initializing…)"),
                )
            });

        // Footer: button to collapse/expand the panel.
        let footer = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .h(px(28.))
            .px(px(8.))
            .bg(bg)
            .child(
                div()
                    .px_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(gpui::opaque_grey(0.2, 0.7))
                    .text_color(fg)
                    .cursor_pointer()
                    .child(if self.collapsed { "Expand" } else { "Collapse" })
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_toggle)),
            );

        div()
            .key_context("TerminalView")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .child(header)
            .child(content)
            .child(footer)
    }
}

/// (Optional) A convenience to preview the panel standalone in a window
/// when developing this crate in isolation.
/// Not used from the workspace, but harmless to keep around for local runs.
#[allow(dead_code)]
pub fn preview() {
    gpui::Application::new().run(|cx: &mut gpui::App| {
        let bounds = Bounds::centered(None, gpui::size(px(800.0), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| TerminalView::new(cx, TerminalConfig::default())),
        )
        .unwrap();
        cx.activate(true);
    });
}
