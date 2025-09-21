use std::sync::Arc;

use gpui::{prelude::*, px, svg, Hsla, Pixels};

/// Vector is a tiny wrapper around `gpui::svg()` that makes it easy to:
/// - specify a path to an SVG,
/// - size it (square or custom width/height),
/// - tint it with a color (applied via `currentColor`).
///
/// Notes:
/// - Your SVGs should use `fill="currentColor"` (preferred) or `stroke="currentColor"`
///   so the tint supplied via `.color(...)` takes effect.
/// - This component intentionally returns a renderable element via `.render()`.
///   To add cursors or event handlers, wrap the rendered element with a container.
pub struct Vector {
    path: Arc<str>,
    width: Pixels,
    height: Pixels,
    color: Option<Hsla>,
}

impl Vector {
    /// Create a new vector for the given asset path.
    ///
    /// Use `.square(...)` or `.with_size(...)` to set the rendered size,
    /// and `.color(...)` to set its tint.
    pub fn new(path: impl Into<Arc<str>>) -> Self {
        Self {
            path: path.into(),
            // Sensible default to keep the element visible if neither `square` nor `with_size`
            // are called by the caller.
            width: px(16.0),
            height: px(16.0),
            color: None,
        }
    }

    /// Set a square size (width = height).
    pub fn square(mut self, size: Pixels) -> Self {
        self.width = size;
        self.height = size;
        self
    }

    /// Set an explicit width and height.
    pub fn with_size(mut self, width: Pixels, height: Pixels) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the tint color (applied via `currentColor` in the SVG).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Render the vector as a styled SVG element.
    ///
    /// The returned value implements `IntoElement`.
    /// If you need cursor styling or event handlers, wrap the result:
    ///
    /// div().child(
    ///     Vector::new("assets/icon.svg")
    ///         .square(px(14.0))
    ///         .color(gpui::white())
    ///         .render(),
    /// ).cursor_pointer()
    pub fn render(self) -> impl IntoElement {
        // Base element
        let el = svg()
            .flex_none()
            .w(self.width)
            .h(self.height)
            .path(self.path);

        // Always apply a text color to ensure `currentColor`-based SVGs are tinted.
        // Fall back to white if no explicit color was provided.
        let tint = self.color.unwrap_or_else(gpui::white);
        el.text_color(tint)
    }
}

// Re-export commonly used items so consumers of `slarti-ui` can avoid importing gpui directly.
pub use gpui::{px as pixels, Hsla as VectorColor, Pixels as VectorPixels};
