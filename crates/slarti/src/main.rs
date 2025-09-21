use gpui::{
    div, prelude::*, px, size, svg, App, Application, Bounds, Context, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Size, Window, WindowBounds, WindowOptions,
};

// Terminal panel from the slarti-term crate
use slarti_term::{TerminalConfig, TerminalView};

struct ContainerView {
    focus: FocusHandle,
    // Panels
    terminal: gpui::Entity<TerminalView>,
    terminal_collapsed: bool,
    // Window state
    dragging_window: bool,
    resizing_edge: Option<gpui::ResizeEdge>,
    saved_windowed_bounds: Option<Bounds<Pixels>>,
    is_maximized: bool,
}

impl ContainerView {
    fn new(cx: &mut Context<Self>, terminal: gpui::Entity<TerminalView>) -> Self {
        Self {
            focus: cx.focus_handle(),
            terminal,
            terminal_collapsed: false,
            dragging_window: false,
            resizing_edge: None,
            saved_windowed_bounds: None,
            is_maximized: false,
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

    fn on_maximize(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Toggle maximize using stored bounds. For Wayland we simulate by resizing to display bounds.
        if self.is_maximized {
            if let Some(bounds) = self.saved_windowed_bounds.take() {
                window.resize(bounds.size);
                // Attempt to place origin back if supported
                // Fallback: content-only resize already positions reasonably
            }
            self.is_maximized = false;
        } else {
            // Save current bounds and maximize to primary display
            let current = window.bounds();
            self.saved_windowed_bounds = Some(current);
            let display_bounds = Bounds::centered(None, current.size, cx); // fallback
                                                                           // If available, use primary display bounds; otherwise use current centered bounds
            let size = Size {
                width: display_bounds.size.width,
                height: display_bounds.size.height,
            };
            window.resize(size);
            self.is_maximized = true;
        }
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

    // Start dragging window from custom titlebar (Wayland compat)
    fn on_titlebar_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dragging_window = true;
        window.start_window_move();
    }

    fn on_titlebar_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dragging_window = false;
    }

    // Edge resize handlers (Wayland compat)
    fn on_resize_left(&mut self, _: &MouseDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        window.start_window_resize(gpui::ResizeEdge::Left);
    }
    fn on_resize_right(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.start_window_resize(gpui::ResizeEdge::Right);
    }
    fn on_resize_top(&mut self, _: &MouseDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        window.start_window_resize(gpui::ResizeEdge::Top);
    }
    fn on_resize_bottom(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.start_window_resize(gpui::ResizeEdge::Bottom);
    }
    fn on_resize_tl(&mut self, _: &MouseDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        window.start_window_resize(gpui::ResizeEdge::TopLeft);
    }
    fn on_resize_tr(&mut self, _: &MouseDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        window.start_window_resize(gpui::ResizeEdge::TopRight);
    }
    fn on_resize_bl(&mut self, _: &MouseDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        window.start_window_resize(gpui::ResizeEdge::BottomLeft);
    }
    fn on_resize_br(&mut self, _: &MouseDownEvent, window: &mut Window, _cx: &mut Context<Self>) {
        window.start_window_resize(gpui::ResizeEdge::BottomRight);
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

        // Header: custom titlebar with drag-to-move and icon buttons
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
            // Left: app/menu placeholder
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
            // Center: draggable region
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_titlebar_mouse_up))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_titlebar_mouse_down))
                    .text_color(text_color)
                    .child("Slarti"),
            )
            // Right: window controls (icons) - force white for dark header
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        svg()
                            .path("assets/generic_minimize.svg")
                            .size(px(14.0))
                            .text_color(text_color)
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_minimize)),
                    )
                    .child(
                        svg()
                            .path("assets/generic_maximize.svg")
                            .size(px(14.0))
                            .text_color(text_color)
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_maximize)),
                    )
                    .child(
                        svg()
                            .path("assets/generic_close.svg")
                            .size(px(14.0))
                            .text_color(text_color)
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_close)),
                    ),
            );

        // Content: make the terminal panel fill remaining space in the container.
        let content = {
            // Container background
            let bg = gpui::rgb(0x0b0b0b);
            // The terminal panel fills all remaining height
            let panel = div()
                .flex()
                .flex_col()
                .size_full() // take all remaining vertical space
                .w_full()
                .border_b_1()
                .border_color(chrome_border)
                .when(!self.terminal_collapsed, |d| d.child(self.terminal.clone()));

            // The content area itself also fills the available space
            div().flex().flex_col().size_full().bg(bg).child(panel)
        };

        // Footer: terminal toggle button uses icon instead of text.
        let footer = {
            let terminal_button = svg()
                .path("assets/terminal.svg")
                .size(px(18.0))
                .text_color(text_color)
                .cursor_pointer()
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_toggle_terminal));

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
                .child(terminal_button)
        };

        // Edge/corner resize hit zones (Wayland)
        let resize_overlay = div()
            .absolute()
            .inset(px(0.))
            .child(
                // Top edge
                div()
                    .absolute()
                    .top(px(0.))
                    .left(px(8.))
                    .right(px(8.))
                    .h(px(6.))
                    .cursor_n_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_top)),
            )
            .child(
                // Bottom edge
                div()
                    .absolute()
                    .bottom(px(0.))
                    .left(px(8.))
                    .right(px(8.))
                    .h(px(6.))
                    .cursor_s_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_bottom)),
            )
            .child(
                // Left edge
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(8.))
                    .bottom(px(8.))
                    .w(px(6.))
                    .cursor_w_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_left)),
            )
            .child(
                // Right edge
                div()
                    .absolute()
                    .right(px(0.))
                    .top(px(8.))
                    .bottom(px(8.))
                    .w(px(6.))
                    .cursor_e_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_right)),
            )
            .child(
                // Top-left corner
                div()
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .size(px(10.))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_tl)),
            )
            .child(
                // Top-right corner
                div()
                    .absolute()
                    .top(px(0.))
                    .right(px(0.))
                    .size(px(10.))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_tr)),
            )
            .child(
                // Bottom-left corner
                div()
                    .absolute()
                    .bottom(px(0.))
                    .left(px(0.))
                    .size(px(10.))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_bl)),
            )
            .child(
                // Bottom-right corner
                div()
                    .absolute()
                    .bottom(px(0.))
                    .right(px(0.))
                    .size(px(10.))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_resize_br)),
            );

        div()
            .key_context("SlartiContainer")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(content)
            .child(resize_overlay)
            .child(footer)
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_focus_click))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.0), px(700.0)), cx);

        let window = cx
            .open_window(
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

        // Capture the container entity to forward keystrokes to the terminal panel.
        let container = window.update(cx, |_, _, cx| cx.entity()).unwrap();

        cx.observe_keystrokes(move |ev, _window, cx| {
            if let Some(ch) = ev.keystroke.key_char.clone() {
                let bytes = ch.to_string().into_bytes();
                let _ = container.update(cx, |cv, cx| {
                    cv.terminal.update(cx, |term, _| term.write_bytes(&bytes));
                    // Request an immediate repaint after sending input
                    cx.notify();
                });
            } else {
                let name = ev.keystroke.unparse();
                let seq: Option<&[u8]> = match name.as_str() {
                    "enter" => Some(b"\r\n"), // send CRLF for immediate command submission on some shells
                    "backspace" => Some(b"\x7f"),
                    "left" => Some(b"\x1b[D"),
                    "right" => Some(b"\x1b[C"),
                    "up" => Some(b"\x1b[A"),
                    "down" => Some(b"\x1b[B"),
                    _ => None,
                };
                if let Some(bytes) = seq {
                    let _ = container.update(cx, |cv, cx| {
                        cv.terminal.update(cx, |term, _| term.write_bytes(bytes));
                        cx.notify();
                    });
                }
            }
        })
        .detach();

        cx.activate(true);
    });
}
