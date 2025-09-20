use gpui::{
    div, prelude::*, px, relative, size, App, Application, Bounds, Context, Element, ElementId,
    Entity, FocusHandle, Focusable, GlobalElementId, LayoutId, Pixels, ShapedLine, Style, TextRun,
    Timer, UnderlineStyle, Window, WindowBounds, WindowOptions,
};

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{Config, Term},
    vte::ansi::Processor,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

/// Fixed terminal size for now. In a follow-up, wire this to window resize events.
struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl TermSize {
    fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
        }
    }
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

struct AppModel {
    focus_handle: FocusHandle,
    term: Term<VoidListener>,
    processor: Processor,
    rx_buf: Arc<Mutex<Vec<u8>>>,
    pty_writer: Option<Arc<Mutex<Box<dyn std::io::Write + Send>>>>,
    // Keep a handle to the master PTY so we can resize it later.
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    // Set by PTY reader when bytes arrive; polled by a 60Hz ticker to notify UI.
    pending_rx: Arc<AtomicBool>,
    // UI theme (light text on dark background by default).
    theme: Theme,
}

impl AppModel {
    fn new(focus_handle: FocusHandle) -> Self {
        let size = TermSize::new(80, 24);
        let term = Term::new(Config::default(), &size, VoidListener);
        let processor = Processor::new();
        let rx_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

        // Spawn a local shell in a PTY and stream its output into rx_buf.
        let pty_system = native_pty_system();
        let pty_size = PtySize {
            rows: size.screen_lines as u16,
            cols: size.columns as u16,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(pty_size).expect("openpty");

        let mut cmd = if cfg!(target_os = "windows") {
            CommandBuilder::new("powershell.exe")
        } else {
            CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()))
        };
        let _ = cmd.cwd(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
        let _child = pair.slave.spawn_command(cmd).expect("spawn shell");
        drop(pair.slave);

        // Hold onto the master for future resizing.
        let master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>> =
            Arc::new(Mutex::new(pair.master));

        // Create a dedicated reader for PTY output and take a writer for input.
        let mut reader = master
            .lock()
            .expect("lock master")
            .try_clone_reader()
            .expect("reader");
        let writer = master.lock().expect("lock master").take_writer().ok();
        let pty_writer = writer.map(|w| Arc::new(Mutex::new(w)));

        let rx_buf_clone = rx_buf.clone();
        let pending_rx = Arc::new(AtomicBool::new(false));
        let pending_rx_clone = pending_rx.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        eprintln!(
                            "[PTY->RX] read {} bytes: {}",
                            n,
                            String::from_utf8_lossy(&buf[..n]).replace('\n', "\\n")
                        );
                        if let Ok(mut v) = rx_buf_clone.lock() {
                            v.extend_from_slice(&buf[..n]);
                        }
                        pending_rx_clone.store(true, Ordering::Release);
                    }
                    Err(e) => {
                        eprintln!("[PTY->RX] read error: {:?}", e);
                        break;
                    }
                }
            }
        });

        Self {
            focus_handle,
            term,
            processor,
            rx_buf,
            pty_writer,
            master,
            pending_rx,
            theme: Theme::default_dark(),
        }
    }

    /// Write bytes to the PTY if available.
    fn write_to_pty(&self, bytes: &[u8]) {
        if let Some(w) = &self.pty_writer {
            if let Ok(mut w) = w.lock() {
                let _ = w.write_all(bytes);
                let _ = w.flush();
            }
        }
    }

    /// Resize the terminal and the PTY if the size has changed.
    fn resize_if_needed(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 {
            return;
        }
        if cols != self.term.columns() || rows != self.term.screen_lines() {
            self.term.resize(TermSize::new(cols, rows));
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
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;

    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.0;

    let d = max - min;
    let s = if d == 0.0 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs())
    };

    let mut h = 0.0;
    if d != 0.0 {
        if max == rf {
            h = ((gf - bf) / d) % 6.0;
        } else if max == gf {
            h = (bf - rf) / d + 2.0;
        } else {
            h = (rf - gf) / d + 4.0;
        }
        h *= 60.0;
        if h < 0.0 {
            h += 360.0;
        }
        h /= 360.0;
    }

    (h, s, l)
}

fn color_from_ansi(
    color: &alacritty_terminal::vte::ansi::Color,
    palette: &alacritty_terminal::term::color::Colors,
) -> Option<(u8, u8, u8)> {
    use alacritty_terminal::vte::ansi::Color as AnsiColor;
    use alacritty_terminal::vte::ansi::Rgb as AnsiRgb;
    match color {
        AnsiColor::Spec(AnsiRgb { r, g, b }) => Some((*r, *g, *b)),
        AnsiColor::Named(named) => palette[*named].map(|AnsiRgb { r, g, b }| (r, g, b)),
        AnsiColor::Indexed(i) => palette[*i as usize].map(|AnsiRgb { r, g, b }| (r, g, b)),
    }
}
struct Theme {
    // Stored as HSLA components; convert with gpui::hsla on use.
    fg: (f32, f32, f32, f32),
    bg: (f32, f32, f32, f32),
    cursor: (f32, f32, f32, f32),
}
impl Theme {
    fn default_dark() -> Self {
        // fg: white, bg: near-black, cursor: blue-ish
        Self {
            fg: (0.0, 0.0, 1.0, 1.0),
            bg: (0.0, 0.0, 0.05, 1.0),
            cursor: (0.66, 1.0, 0.5, 1.0),
        }
    }
}

struct TerminalSizer {
    app: Entity<AppModel>,
    cell_width: f32,
    cell_height: f32,
    cache: Vec<Option<ShapedLine>>,
    last_cols: usize,
    last_rows: usize,
}

impl IntoElement for TerminalSizer {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalSizer {
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
        _window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let width = (bounds.right() - bounds.left()).0;
        let height = (bounds.bottom() - bounds.top()).0;
        let cols = (width / self.cell_width).floor().max(1.0) as usize;
        let rows = (height / self.cell_height).floor().max(1.0) as usize;

        self.app
            .update(cx, |app, _| app.resize_if_needed(cols, rows));
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
        // Measure cell metrics based on current font for precise sizing.
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let meas_run = TextRun {
            len: 1,
            font: window.text_style().font(),
            color: window.text_style().color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let meas_line = window.text_system().shape_line(
            gpui::SharedString::from("W"),
            font_size,
            &[meas_run],
            None,
        );
        self.cell_width = meas_line.x_for_index(1).0;
        self.cell_height = window.line_height().0;

        // Compute cols/rows for current bounds.
        let width = (bounds.right() - bounds.left()).0;
        let height = (bounds.bottom() - bounds.top()).0;
        let cols = (width / self.cell_width).floor().max(1.0) as usize;
        let rows = (height / self.cell_height).floor().max(1.0) as usize;

        // Reset cache if dimensions changed.
        if self.last_cols != cols || self.last_rows != rows {
            self.last_cols = cols;
            self.last_rows = rows;
            self.cache = vec![None; rows];
        } else if self.cache.len() != rows {
            self.cache.resize(rows, None);
        }

        // Collect which rows need re-shaping based on terminal damage.
        let mut rows_to_shape = vec![false; rows];
        let mut cursor_col = 0usize;
        let mut cursor_row = 0usize;
        let mut lines_to_shape: Vec<(
            usize,
            String,
            Vec<(usize, Option<(u8, u8, u8)>, Option<(u8, u8, u8)>, bool)>,
        )> = Vec::new();

        self.app.update(cx, |app, _| {
            // Determine damaged rows.
            match app.term.damage() {
                alacritty_terminal::term::TermDamage::Full => {
                    for y in 0..rows {
                        rows_to_shape[y] = true;
                    }
                }
                alacritty_terminal::term::TermDamage::Partial(mut iter) => {
                    while let Some(line) = iter.next() {
                        if line.line < rows {
                            rows_to_shape[line.line] = true;
                        }
                    }
                }
            }

            // Always shape missing cache entries at least once.
            for y in 0..rows {
                if self.cache.get(y).and_then(|o| o.as_ref()).is_none() {
                    rows_to_shape[y] = true;
                }
            }

            // Cursor position for painting.
            let content = app.term.renderable_content();
            let cursor = content.cursor;
            cursor_col = cursor.point.column.0;
            cursor_row = (cursor.point.line.0).max(0) as usize;

            // Build line text and basic color runs for damaged/missing rows.
            for y in 0..rows {
                if !rows_to_shape[y] {
                    continue;
                }
                let mut line_text = String::with_capacity(cols);
                let mut runs: Vec<(usize, Option<(u8, u8, u8)>, Option<(u8, u8, u8)>, bool)> =
                    Vec::new();

                // Helper to push or extend a run.
                let mut push_run =
                    |len: usize, fg: Option<(u8, u8, u8)>, bg: Option<(u8, u8, u8)>, ul: bool| {
                        if let Some(last) = runs.last_mut() {
                            if last.1 == fg && last.2 == bg && last.3 == ul {
                                last.0 += len;
                                return;
                            }
                        }
                        runs.push((len, fg, bg, ul));
                    };

                for x in 0..cols {
                    let cell = &app.term.grid()[alacritty_terminal::index::Line(y as i32)]
                        [alacritty_terminal::index::Column(x)];
                    let ch = cell.c;
                    line_text.push(ch);

                    // Resolve ANSI fg/bg using the terminal palette (handles Named/Indexed) or Spec RGB.
                    let fg_rgb: Option<(u8, u8, u8)> = color_from_ansi(&cell.fg, app.term.colors());
                    let bg_rgb: Option<(u8, u8, u8)> = color_from_ansi(&cell.bg, app.term.colors());
                    let underline = {
                        let f = cell.flags;
                        f.contains(Flags::UNDERLINE)
                            || f.contains(Flags::DOUBLE_UNDERLINE)
                            || f.contains(Flags::DOTTED_UNDERLINE)
                            || f.contains(Flags::DASHED_UNDERLINE)
                            || f.contains(Flags::UNDERCURL)
                    };

                    // Count by UTF-8 bytes to match TextRun len convention used elsewhere.
                    let char_len = ch.len_utf8();
                    push_run(char_len, fg_rgb, bg_rgb, underline);
                }

                lines_to_shape.push((y, line_text, runs));
            }

            // Reset terminal damage after capturing lines.
            app.term.reset_damage();
        });

        // Shape damaged/missing rows and update cache.
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        // Resolve theme fallback color once per paint.
        let mut theme_fg = (0.0, 0.0, 1.0, 1.0);
        self.app.update(cx, |app, _| {
            theme_fg = app.theme.fg;
        });

        for (row, text, runs_spec) in lines_to_shape.drain(..) {
            // Map run specs to gpui::TextRun entries using terminal cell foreground colors.
            let runs: Vec<TextRun> = runs_spec
                .into_iter()
                .map(|(len, fg_rgb, bg_rgb, underline)| {
                    let color = match fg_rgb {
                        Some((r, g, b)) => {
                            let (h, s, l) = rgb_to_hsl(r, g, b);
                            gpui::hsla(h, s, l, 1.0)
                        }
                        None => gpui::hsla(theme_fg.0, theme_fg.1, theme_fg.2, theme_fg.3),
                    };
                    let background_color = bg_rgb.map(|(r, g, b)| {
                        let (h, s, l) = rgb_to_hsl(r, g, b);
                        gpui::hsla(h, s, l, 1.0)
                    });
                    let underline_style = if underline {
                        Some(UnderlineStyle {
                            color: Some(color),
                            thickness: px(1.0),
                            wavy: false,
                        })
                    } else {
                        None
                    };
                    TextRun {
                        len,
                        font: window.text_style().font(),
                        color,
                        background_color,
                        underline: underline_style,
                        strikethrough: None,
                    }
                })
                .collect();

            let shaped = window.text_system().shape_line(
                gpui::SharedString::from(text),
                font_size,
                &runs,
                None,
            );
            if row < self.cache.len() {
                self.cache[row] = Some(shaped);
            }
        }

        // Paint terminal background using theme.
        let mut theme_bg = (0.0, 0.0, 0.05, 1.0);
        self.app.update(cx, |app, _| {
            theme_bg = app.theme.bg;
        });
        window.paint_quad(gpui::fill(
            bounds,
            gpui::hsla(theme_bg.0, theme_bg.1, theme_bg.2, theme_bg.3),
        ));

        // Paint each shaped line at its row position from cache.
        let origin_x = bounds.left();
        let mut line_origin = gpui::point(origin_x, bounds.top());
        for row in 0..rows {
            if let Some(shaped) = self.cache.get_mut(row).and_then(|o| o.take()) {
                let y = bounds.top().0 + (row as f32 * self.cell_height);
                line_origin.y = gpui::px(y);
                let _ = shaped.paint(line_origin, window.line_height(), window, cx);
                // Put it back for next frame reuse.
                if row < self.cache.len() {
                    self.cache[row] = Some(shaped);
                }
            }
        }

        // Paint a cursor block aligned using shaped line metrics when available.
        // Compute byte index for the cursor column (UTF-8 based for shaping).
        let mut cursor_byte_index = 0usize;
        self.app.update(cx, |app, _| {
            for x in 0..cursor_col.min(cols) {
                cursor_byte_index += app.term.grid()
                    [alacritty_terminal::index::Line(cursor_row as i32)]
                    [alacritty_terminal::index::Column(x)]
                .c
                .len_utf8();
            }
        });

        let cursor_x = if let Some(shaped) = self.cache.get(cursor_row).and_then(|o| o.as_ref()) {
            bounds.left().0 + shaped.x_for_index(cursor_byte_index).0
        } else {
            bounds.left().0 + (cursor_col as f32 * self.cell_width)
        };

        let cursor_y = bounds.top().0 + (cursor_row as f32 * self.cell_height);
        let cursor_bounds = Bounds::new(
            gpui::point(gpui::px(cursor_x), gpui::px(cursor_y)),
            size(gpui::px(self.cell_width), gpui::px(self.cell_height)),
        );
        // Cursor color from theme.
        let mut theme_cursor = (0.66, 1.0, 0.5, 1.0);
        self.app.update(cx, |app, _| {
            theme_cursor = app.theme.cursor;
        });
        window.paint_quad(gpui::fill(
            cursor_bounds,
            gpui::hsla(
                theme_cursor.0,
                theme_cursor.1,
                theme_cursor.2,
                theme_cursor.3,
            ),
        ));
    }
}

impl Focusable for AppModel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AppModel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain pending PTY bytes and advance the terminal state.
        if let Ok(mut buf) = self.rx_buf.lock() {
            if !buf.is_empty() {
                let pending = buf.split_off(0);
                eprintln!(
                    "[VTE] advancing with {} bytes: {}",
                    pending.len(),
                    String::from_utf8_lossy(&pending).replace('\n', "\\n")
                );
                self.processor.advance(&mut self.term, &pending);
                cx.notify();
            }
        }

        // Render the visible terminal grid as plain text for now.
        let cols = self.term.columns();
        let rows = self.term.screen_lines();
        let mut text = String::with_capacity((cols + 1) * rows);

        for y in 0..rows {
            for x in 0..cols {
                let ch = self.term.grid()[Line(y as i32)][Column(x)].c;
                text.push(ch);
            }
            if y + 1 != rows {
                text.push('\n');
            }
        }

        div()
            .flex()
            .size_full()
            .p_2()
            .key_context("Terminal")
            .track_focus(&self.focus_handle(cx))
            .child(TerminalSizer {
                app: cx.entity(),
                cell_width: 8.0,
                cell_height: 16.0,
                cache: Vec::new(),
                last_cols: 0,
                last_rows: 0,
            })
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.0), px(600.0)), cx);

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| AppModel::new(cx.focus_handle())),
            )
            .unwrap();

        let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();

        // 60Hz ticker: if PTY bytes arrived, schedule a UI update on the next frame.
        let view_for_ticker = view.clone();
        cx.spawn(async move |cx| loop {
            Timer::after(std::time::Duration::from_millis(16)).await;
            let _ = view_for_ticker.update(cx, |app, cx| {
                if app.pending_rx.swap(false, Ordering::AcqRel) {
                    cx.notify();
                }
            });
        })
        .detach();

        // Send typed characters and special keys to the PTY.
        cx.observe_keystrokes(move |ev, _, cx| {
            if let Some(ch) = ev.keystroke.key_char.clone() {
                let s = ch.to_string();
                view.update(cx, |app, _| app.write_to_pty(s.as_bytes()));
            } else {
                let name = ev.keystroke.unparse();
                let seq: Option<&[u8]> = match name.as_str() {
                    "enter" => Some(b"\r"),
                    "backspace" => Some(b"\x7f"),
                    "left" => Some(b"\x1b[D"),
                    "right" => Some(b"\x1b[C"),
                    "up" => Some(b"\x1b[A"),
                    "down" => Some(b"\x1b[B"),
                    _ => None,
                };
                if let Some(bytes) = seq {
                    view.update(cx, |app, _| app.write_to_pty(bytes));
                }
            }
        })
        .detach();

        // Focus the terminal view and activate the app.
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle(cx));
                cx.activate(true);
            })
            .unwrap();
    });
}
