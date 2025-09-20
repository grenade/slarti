use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

use anyhow::Result;
use gpui::{
    div, prelude::*, px, relative, App, Bounds, Context, Element, ElementId, FocusHandle,
    Focusable, GlobalElementId, LayoutId, MouseButton, MouseUpEvent, Pixels, SharedString, Style,
    TextRun, Window,
};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{Config, Term},
    vte::ansi::Processor,
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

/// Size adaptor for `alacritty_terminal::Term`.
struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Terminal engine: PTY + `alacritty_terminal::Term` + VTE processor and a reader thread.
pub struct Engine {
    term: Term<VoidListener>,
    processor: Option<Processor>,
    rx_buf: Arc<Mutex<Vec<u8>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

impl Engine {
    /// Create a new engine with an initial (cols, rows) size. Spawns the user's shell in a PTY and
    /// a background reader thread to accumulate PTY bytes into `rx_buf`.
    pub fn new(
        cols: usize,
        rows: usize,
    ) -> Result<(Self, Option<Arc<Mutex<Box<dyn Write + Send>>>>)> {
        let term = Term::new(
            Config::default(),
            &TermSize {
                columns: cols,
                screen_lines: rows,
            },
            VoidListener,
        );

        let processor = Some(Processor::new());
        let rx_buf = Arc::new(Mutex::new(Vec::new()));

        // Create PTY
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Spawn shell into PTY
        let mut cmd = if cfg!(target_os = "windows") {
            CommandBuilder::new("powershell.exe")
        } else {
            CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()))
        };
        let _ = cmd.cwd(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
        let _child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        // Hold master for resize and I/O
        let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));
        let mut reader = master.lock().unwrap().try_clone_reader()?;
        let writer = master
            .lock()
            .unwrap()
            .take_writer()
            .ok()
            .map(|w| Arc::new(Mutex::new(w)));

        // Reader thread: accumulate bytes into rx_buf
        {
            let rx_buf_clone = rx_buf.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut v) = rx_buf_clone.lock() {
                                v.extend_from_slice(&buf[..n]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        Ok((
            Self {
                term,
                processor,
                rx_buf,
                master,
            },
            writer,
        ))
    }

    /// Write bytes to the PTY via the provided writer (if present).
    pub fn write(&self, bytes: &[u8], writer: &Option<Arc<Mutex<Box<dyn Write + Send>>>>) {
        if let Some(w) = writer {
            if let Ok(mut guard) = w.lock() {
                let _ = guard.write_all(bytes);
                let _ = guard.flush();
            }
        }
    }

    /// Process a chunk of terminal bytes safely (no overlapping borrows).
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut processor = self.processor.take().expect("processor present");
        processor.advance(&mut self.term, bytes);
        self.processor.replace(processor);
    }

    /// Resize both the terminal and the PTY.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(TermSize {
            columns: cols,
            screen_lines: rows,
        });
        let _ = self.master.lock().ok().map(|m| {
            let _ = m.resize(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            });
        });
    }
}

/// A collapsible panel hosting a terminal canvas.
pub struct TerminalView {
    focus: FocusHandle,
    title: SharedString,
    collapsed: bool,
    theme: Theme,
    engine: Arc<Mutex<Engine>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
}

impl TerminalView {
    /// Construct a new `TerminalView`.
    pub fn new(cx: &mut Context<Self>, config: TerminalConfig) -> Self {
        let (engine, writer) = Engine::new(80, 24).expect("create terminal engine");
        Self {
            focus: cx.focus_handle(),
            title: config.title,
            collapsed: config.collapsed,
            theme: config.theme,
            engine: Arc::new(Mutex::new(engine)),
            writer,
        }
    }

    fn on_toggle(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        cx.notify();
    }

    /// Forward input bytes (e.g. typed characters or escape sequences) to the PTY.
    pub fn write_bytes(&self, bytes: &[u8]) {
        if let Ok(engine) = self.engine.lock() {
            engine.write(bytes, &self.writer);
        }
    }

    /// Drain any pending PTY bytes and advance the terminal processor.
    /// Locks are explicitly scoped to avoid overlapping borrows:
    /// 1) Clone rx_buf under a short engine lock.
    /// 2) Drain bytes under only the rx_buf lock into a local Vec.
    /// 3) Advance the processor under a separate mutable engine lock.
    fn drain_and_advance(&self) -> bool {
        // 1) Clone rx_buf Arc under a short engine lock, then drop that lock.
        let rx_buf_arc = match self.engine.lock() {
            Ok(engine_guard) => engine_guard.rx_buf.clone(),
            Err(_) => return false,
        };

        // 2) Drain bytes into a local Vec while holding only the rx_buf lock.
        let pending_bytes = {
            match rx_buf_arc.lock() {
                Ok(mut rx_guard) => {
                    if rx_guard.is_empty() {
                        None
                    } else {
                        Some(rx_guard.split_off(0))
                    }
                }
                Err(_) => None,
            }
        };

        // 3) Advance the terminal with a separate mutable engine lock.
        if let Some(bytes) = pending_bytes {
            if let Ok(mut engine_guard) = self.engine.lock() {
                engine_guard.process_bytes(&bytes);
                return true;
            }
        }

        false
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain any pending bytes and request repaint if terminal advanced.
        if self.drain_and_advance() {
            cx.notify();
        }

        let theme = self.theme;
        let bg = gpui::hsla(theme.bg.0, theme.bg.1, theme.bg.2, theme.bg.3);
        let fg = gpui::hsla(theme.fg.0, theme.fg.1, theme.fg.2, theme.fg.3);

        // Header
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(28.))
            .px(px(8.))
            .bg(bg)
            .child(
                div()
                    .w(px(28.))
                    .h(px(18.))
                    .rounded_sm()
                    .border_1()
                    .border_color(gpui::opaque_grey(0.2, 0.7))
                    .cursor_default()
                    .child("≡"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .text_color(fg)
                    .child(self.title.clone()),
            );

        // Content
        let engine = self.engine.clone();
        let content = div()
            .flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .when(!self.collapsed, |d| {
                d.child(TerminalCanvasElement { engine, theme })
            });

        // Footer
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

/// A simple canvas element that renders the terminal grid as text and draws a cursor.
struct TerminalCanvasElement {
    engine: Arc<Mutex<Engine>>,
    theme: Theme,
}

impl IntoElement for TerminalCanvasElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalCanvasElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        ()
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Background
        window.paint_quad(gpui::fill(
            bounds,
            gpui::hsla(
                self.theme.bg.0,
                self.theme.bg.1,
                self.theme.bg.2,
                self.theme.bg.3,
            ),
        ));

        // Measure a representative cell size
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let ref_line = window.text_system().shape_line(
            SharedString::from("W"),
            font_size,
            &[TextRun {
                len: 1,
                font: style.font(),
                color: gpui::hsla(
                    self.theme.fg.0,
                    self.theme.fg.1,
                    self.theme.fg.2,
                    self.theme.fg.3,
                ),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let cell_w = ref_line.x_for_index(1).0.max(1.0);
        let cell_h = window.line_height().0.max(1.0);

        // Snapshot dimensions and cursor position
        let (rows, cols, cursor_line, cursor_col) = if let Ok(eng) = self.engine.lock() {
            (
                eng.term.screen_lines(),
                eng.term.columns(),
                eng.term.grid().cursor.point.line.0.max(0) as usize,
                eng.term.grid().cursor.point.column.0,
            )
        } else {
            (0, 0, 0, 0)
        };
        if rows == 0 || cols == 0 {
            return;
        }

        // Render each row as shaped text
        let mut origin = bounds.origin;
        for y in 0..rows {
            let line_text = if let Ok(eng) = self.engine.lock() {
                let mut s = String::with_capacity(cols);
                for x in 0..cols {
                    let ch = eng.term.grid()[Line(y as i32)][Column(x)].c;
                    s.push(ch);
                }
                s
            } else {
                String::new()
            };

            let shaped = window.text_system().shape_line(
                SharedString::from(line_text),
                font_size,
                &[TextRun {
                    len: cols,
                    font: style.font(),
                    color: gpui::hsla(
                        self.theme.fg.0,
                        self.theme.fg.1,
                        self.theme.fg.2,
                        self.theme.fg.3,
                    ),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let _ = shaped.paint(origin, window.line_height(), window, cx);

            origin.y += gpui::px(cell_h);
            if origin.y > bounds.bottom() {
                break;
            }
        }

        // Cursor block
        let cursor_x = bounds.left().0 + cursor_col as f32 * cell_w;
        let cursor_y = bounds.top().0 + cursor_line as f32 * cell_h;
        let cursor_bounds = Bounds::new(
            gpui::point(gpui::px(cursor_x), gpui::px(cursor_y)),
            gpui::size(gpui::px(cell_w), gpui::px(cell_h)),
        );
        window.paint_quad(gpui::fill(
            cursor_bounds,
            gpui::hsla(
                self.theme.cursor.0,
                self.theme.cursor.1,
                self.theme.cursor.2,
                self.theme.cursor.3,
            ),
        ));
    }
}
