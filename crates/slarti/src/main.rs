use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Window, WindowBounds,
    WindowOptions,
};

use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{Config, Term},
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
    term: Term<VoidListener>,
}

impl AppModel {
    fn new() -> Self {
        let size = TermSize::new(80, 24);
        let mut term = Term::new(Config::default(), &size, VoidListener);

        // Seed some visible content in the top line.
        let msg = "slarti terminal demo (alacritty_terminal)";
        {
            let grid = term.grid_mut();
            let cols = grid.columns().min(msg.chars().count());
            for (i, ch) in msg.chars().take(cols).enumerate() {
                grid[Line(0)][Column(i)].c = ch;
            }
        }

        Self { term }
    }
}

impl Render for AppModel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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

        div().flex().size_full().p_2().child(text)
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.0), px(600.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| AppModel::new()),
        )
        .unwrap();
    });
}
