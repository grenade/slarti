use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

use anyhow::Result;
use gpui::{
    div, prelude::*, px, relative, App, Bounds, Context, Element, ElementId, FocusHandle,
    Focusable, GlobalElementId, LayoutId, MouseUpEvent, Pixels, SharedString, Style, TextRun,
    Window,
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

        // Content fills remaining space and always shows the canvas
        let engine = self.engine.clone();
        let content = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .child(TerminalCanvasElement {
                engine,
                theme,
                cell_w: 8.0,
                cell_h: 16.0,
                cache: Vec::new(),
                last_cols: 0,
                last_rows: 0,
            });

        // Footer removed from terminal panel; collapse/expand belongs to outer container

        div()
            .key_context("TerminalView")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .child(header)
            .child(content)
    }
}

/// A simple canvas element that renders the terminal grid as text and draws a cursor.
struct TerminalCanvasElement {
    engine: Arc<Mutex<Engine>>,
    theme: Theme,
    // Measured cell metrics
    cell_w: f32,
    cell_h: f32,
    // Cache of shaped lines for damage-aware rendering
    cache: Vec<Option<gpui::ShapedLine>>,
    // Last known terminal dimensions
    last_cols: usize,
    last_rows: usize,
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
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        // Measure cell size with current font
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let ref_line = window.text_system().shape_line(
            SharedString::from("W"),
            font_size,
            &[TextRun {
                len: 1,
                font: window.text_style().font(),
                color: window.text_style().color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        self.cell_w = ref_line.x_for_index(1).0.max(1.0);
        self.cell_h = window.line_height().0.max(1.0);

        // Compute desired cols/rows from bounds and cell size
        let width = (bounds.right() - bounds.left()).0;
        let height = (bounds.bottom() - bounds.top()).0;
        let cols = (width / self.cell_w).floor().max(1.0) as usize;
        let rows = (height / self.cell_h).floor().max(1.0) as usize;

        // Resize engine and reset cache if dimensions changed
        if self.last_cols != cols || self.last_rows != rows {
            self.last_cols = cols;
            self.last_rows = rows;
            self.cache = vec![None; rows];
            if let Ok(mut eng) = self.engine.lock() {
                eng.resize(cols, rows);
            }
        } else if self.cache.len() != rows {
            self.cache.resize(rows, None);
        }

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
        // Paint panel background
        window.paint_quad(gpui::fill(
            bounds,
            gpui::hsla(
                self.theme.bg.0,
                self.theme.bg.1,
                self.theme.bg.2,
                self.theme.bg.3,
            ),
        ));

        // Shape and paint all rows each frame to ensure visibility (temporary simplification)
        let (rows, cols, cursor_line, cursor_col) = if let Ok(eng) = self.engine.lock() {
            (
                eng.term.screen_lines(),
                eng.term.columns(),
                eng.term.grid().cursor.point.line.0.max(0) as usize,
                eng.term.grid().cursor.point.column.0,
            )
        } else {
            return;
        };

        // Ensure we have a valid font setup for shaping
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let fg = gpui::hsla(
            self.theme.fg.0,
            self.theme.fg.1,
            self.theme.fg.2,
            self.theme.fg.3,
        );

        // Paint lines
        let mut origin = bounds.origin;
        // Track pixel-accurate cursor placement using shaped metrics
        let mut cursor_px: Option<f32> = None;
        let mut cursor_py: Option<f32> = None;
        for y in 0..rows {
            // Build plain text for the line
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

            // Compute cursor byte index before shaping to avoid moving line_text
            let byte_idx_opt = if y == cursor_line {
                let mut idx = 0usize;
                for (xi, ch) in line_text.chars().enumerate() {
                    if xi >= cursor_col {
                        break;
                    }
                    idx += ch.len_utf8();
                }
                Some(idx)
            } else {
                None
            };

            // Shape the line with explicit theme foreground color
            let shaped = window.text_system().shape_line(
                SharedString::from(line_text),
                font_size,
                &[TextRun {
                    len: cols,
                    font: window.text_style().font(),
                    color: fg,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );

            // If this is the cursor line, compute pixel x from shaped metrics
            if let Some(byte_idx) = byte_idx_opt {
                let rel_x = shaped.x_for_index(byte_idx).0;
                cursor_px = Some(bounds.left().0 + rel_x);
                cursor_py = Some(origin.y.0);
            }

            let _ = shaped.paint(origin, window.line_height(), window, cx);

            origin.y += gpui::px(self.cell_h);
            if origin.y > bounds.bottom() {
                break;
            }
        }

        // Draw a solid cursor block using shaped metrics when available
        let (cursor_x, cursor_y) = if let (Some(px), Some(py)) = (cursor_px, cursor_py) {
            (px, py)
        } else {
            (
                bounds.left().0 + cursor_col as f32 * self.cell_w,
                bounds.top().0 + cursor_line as f32 * self.cell_h,
            )
        };
        let cursor_bounds = Bounds::new(
            gpui::point(gpui::px(cursor_x), gpui::px(cursor_y)),
            gpui::size(gpui::px(self.cell_w), gpui::px(self.cell_h)),
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
