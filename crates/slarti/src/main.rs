use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, FocusHandle, Focusable,
    MouseButton, MouseUpEvent, SharedString, Window, WindowBounds, WindowOptions,
};

// Terminal panel from the slarti-term crate
use slarti_term::{TerminalConfig, TerminalView};

struct ContainerView {
    focus: FocusHandle,
    // Panels
    terminal: gpui::Entity<TerminalView>,
    terminal_collapsed: bool,
}

impl ContainerView {
    fn new(cx: &mut Context<Self>, terminal: gpui::Entity<TerminalView>) -> Self {
        Self {
            focus: cx.focus_handle(),
            terminal,
            terminal_collapsed: false,
        }
    }

    // Header controls: left menu is a placeholder for now.
    fn on_close(&mut self, _: &MouseUpEvent, window: &mut Window, _cx: &mut Context<Self>) {
        // Close just removes the current window. A multi-window shell can intercept differently.
        window.remove_window();
    }

    fn on_minimize(&mut self, _: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // Minimize by hiding the application; a timer or menu can re-activate later.
        cx.hide();
    }

    fn on_maximize(&mut self, _: &MouseUpEvent, window: &mut Window, _cx: &mut Context<Self>) {
        // Naive maximize: toggle a modest size bump relative to current bounds.
        let content_size = window.bounds().size;
        // Swap width/height the same way as the gpui example
        window.resize(size(content_size.height * 1.1, content_size.width * 1.1));
    }

    fn on_toggle_terminal(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_collapsed = !self.terminal_collapsed;
        cx.notify();
    }

    fn on_focus_click(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle(cx));
    }
}

impl Focusable for ContainerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for ContainerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar_bg = gpui::rgb(0x141414);
        let chrome_border = gpui::opaque_grey(0.2, 0.7);
        let text_color = gpui::white();

        // Header: menu placeholder on the left, centered title, window controls on the right.
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(32.))
            .px(px(8.))
            .bg(title_bar_bg)
            .border_b_1()
            .border_color(chrome_border)
            // Left: collapsible menu placeholder
            .child(
                div()
                    .w(px(28.))
                    .h(px(18.))
                    .rounded_sm()
                    .border_1()
                    .border_color(chrome_border)
                    .cursor_default()
                    .child("≡"),
            )
            // Center: app title
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .text_color(text_color)
                    .child("Slarti"),
            )
            // Right: window controls
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(14.))
                            .rounded_sm()
                            .bg(gpui::yellow())
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_minimize)),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(14.))
                            .rounded_sm()
                            .bg(gpui::green())
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_maximize)),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(14.))
                            .rounded_sm()
                            .bg(gpui::red())
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_close)),
                    ),
            );

        // Content: stack child panels vertically (for now only Terminal).
        let content = {
            // Ensure the content uses all available space, with a subtle bg.
            let bg = gpui::rgb(0x0b0b0b);
            let panel = div()
                .flex()
                .flex_col()
                .w_full()
                .border_b_1()
                .border_color(chrome_border)
                .when(!self.terminal_collapsed, |d| d.child(self.terminal.clone()));

            div().flex().flex_col().size_full().bg(bg).child(panel)
        };

        // Footer: buttons to expand/collapse child panels.
        let footer = {
            let button_style = |label: SharedString| {
                div()
                    .px_2()
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(chrome_border)
                    .text_color(text_color)
                    .cursor_pointer()
                    .child(label)
            };

            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .h(px(32.))
                .px(px(8.))
                .bg(title_bar_bg)
                .border_t_1()
                .border_color(chrome_border)
                .child(
                    button_style(SharedString::from(if self.terminal_collapsed {
                        "Expand Terminal"
                    } else {
                        "Collapse Terminal"
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_toggle_terminal)),
                )
        };

        div()
            .key_context("SlartiContainer")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(content)
            .child(footer)
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_focus_click))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.0), px(700.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                // Build the terminal panel from slarti-term.
                let terminal = cx.new(|cx| TerminalView::new(cx, TerminalConfig::default()));
                // Build the container that will host panels (terminal and future ones).
                cx.new(|cx| ContainerView::new(cx, terminal))
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
