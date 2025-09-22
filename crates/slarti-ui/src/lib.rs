use std::sync::Arc;

use gpui::{prelude::*, px, svg, Hsla, Pixels};
use std::{
    env,
    path::{Path, PathBuf},
};
use tracing::debug;

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

/// Returns true when SLARTI_UI_DEBUG is enabled (e.g. "1", "true", "yes", "on").
// Note: SLARTI_UI_DEBUG has been removed.
// Use standard Rust logging (RUST_LOG) with tracing instead.

fn resolve_asset_path(path: &Arc<str>) -> Arc<str> {
    let s: &str = path.as_ref();
    let rel = Path::new(s);

    // Absolute path that exists
    if rel.is_absolute() && rel.exists() {
        return path.clone();
    }

    // If the provided path starts with "assets", also keep a handle to the remainder.
    let rel_after_assets = rel.strip_prefix("assets").ok();

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1) Current working directory and its parents
    if let Ok(mut cwd) = env::current_dir() {
        for _ in 0..8 {
            candidates.push(cwd.join(rel));
            if let Some(after) = rel_after_assets {
                candidates.push(cwd.join("assets").join(after));
            }
            if let Some(parent) = cwd.parent() {
                cwd = parent.to_path_buf();
            } else {
                break;
            }
        }
    }

    // 2) Executable directory and its parents (e.g., target/debug)
    if let Ok(mut exe_dir) = env::current_exe() {
        exe_dir.pop(); // drop the executable filename
        let mut dir = Some(exe_dir);
        for _ in 0..8 {
            if let Some(d) = dir.clone() {
                candidates.push(d.join(rel));
                if let Some(after) = rel_after_assets {
                    candidates.push(d.join("assets").join(after));
                }
                dir = d.parent().map(|p| p.to_path_buf());
            } else {
                break;
            }
        }
    }

    // 3) This crate's manifest directory and its parents (workspace layouts)
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut md = Some(manifest_dir);
    for _ in 0..8 {
        if let Some(d) = md.clone() {
            candidates.push(d.join(rel));
            if let Some(after) = rel_after_assets {
                candidates.push(d.join("assets").join(after));
            }
            md = d.parent().map(|p| p.to_path_buf());
        } else {
            break;
        }
    }

    // 4) Environment overrides for robustness in dev/prod
    if let Ok(dir) = env::var("SLARTI_ASSETS_DIR") {
        let base = PathBuf::from(dir);
        candidates.push(base.join(rel));
        if let Some(after) = rel_after_assets {
            candidates.push(base.join("assets").join(after));
        }
    }
    if let Ok(dir) = env::var("SLARTI_WORKSPACE_DIR") {
        let base = PathBuf::from(dir);
        candidates.push(base.join(rel));
        if let Some(after) = rel_after_assets {
            candidates.push(base.join("assets").join(after));
        }
    }

    // First existing candidate wins
    for cand in candidates {
        if cand.exists() {
            return Arc::from(cand.to_string_lossy().into_owned());
        }
    }

    // Fallback to the original (let gpui attempt its own resolution).
    path.clone()
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
        // Determine tint and resolve asset path.
        let tint = self.color.unwrap_or_else(gpui::white);
        let resolved = resolve_asset_path(&self.path);
        let exists = Path::new(resolved.as_ref()).exists();
        debug!(
            target: "slarti_ui::vector",
            "path='{}' resolved='{}' exists={} size=({:?},{:?}) color={:?}",
            self.path, resolved, exists, self.width, self.height, tint
        );

        // Prepare the SVG icon element (may render empty if asset is missing).
        let icon = svg()
            .flex_none()
            .w(self.width)
            .h(self.height)
            .path(resolved)
            .text_color(tint);

        // Return the icon directly; use tracing (RUST_LOG) for diagnostics above.
        icon
    }
}

// Re-export commonly used items so consumers of `slarti-ui` can avoid importing gpui directly.
pub use gpui::{px as pixels, Hsla as VectorColor, Pixels as VectorPixels};

/// Filesystem-backed AssetSource to load assets from disk.
///
/// Use this with `Application::with_assets(FsAssets::new().with_root(...))`
/// so GPUI can resolve SVG/image assets directly from the filesystem.
///
/// By default this loader only uses the roots you supply. It attempts:
/// - Absolute path as-is (if it exists)
/// - Each configured root joined with the requested `path`
/// - Each configured root joined with `"assets"` then the requested `path`
#[derive(Default, Clone)]
pub struct FsAssets {
    roots: Vec<std::path::PathBuf>,
}

impl FsAssets {
    /// Create an empty asset source. Call `.with_root`/`.add_root` to register roots.
    pub fn new() -> Self {
        Self { roots: Vec::new() }
    }

    /// Returns a new instance with the given root added.
    pub fn with_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.roots.push(root.into());
        self
    }

    /// Adds a root directory to search for assets.
    pub fn add_root(&mut self, root: impl Into<std::path::PathBuf>) {
        self.roots.push(root.into());
    }

    fn resolve(&self, path: &str) -> Option<std::path::PathBuf> {
        use std::path::Path;
        let requested = Path::new(path);

        // Absolute path that exists
        if requested.is_absolute() && requested.exists() {
            return Some(requested.to_path_buf());
        }

        // Search configured roots
        for root in &self.roots {
            let candidate = root.join(requested);
            if candidate.exists() {
                return Some(candidate);
            }
            // Also try an `assets/` subdirectory underneath the root as a convenience.
            let candidate_assets = root.join("assets").join(requested);
            if candidate_assets.exists() {
                return Some(candidate_assets);
            }
        }

        None
    }
}

impl gpui::AssetSource for FsAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        use std::{borrow::Cow, fs};

        if let Some(abs) = self.resolve(path) {
            let bytes = fs::read(abs)?;
            Ok(Some(Cow::Owned(bytes)))
        } else {
            Ok(None)
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        use std::fs;
        use std::path::Path;

        let mut out = Vec::new();

        // List using each root
        for root in &self.roots {
            let dir = root.join(path);
            if dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let fname = entry.file_name();
                        let name = fname.to_string_lossy().into_owned();
                        out.push(name.into());
                    }
                }
            }
            // Also try root/assets/path for convenience
            let dir_assets = root.join("assets").join(path);
            if dir_assets.is_dir() {
                if let Ok(entries) = fs::read_dir(&dir_assets) {
                    for entry in entries.flatten() {
                        let fname = entry.file_name();
                        let name = fname.to_string_lossy().into_owned();
                        out.push(name.into());
                    }
                }
            }
        }

        // If `path` itself is absolute or relative to CWD and is a dir, list directly too.
        let p = Path::new(path);
        if p.is_dir() {
            if let Ok(entries) = fs::read_dir(p) {
                for entry in entries.flatten() {
                    let fname = entry.file_name();
                    let name = fname.to_string_lossy().into_owned();
                    out.push(name.into());
                }
            }
        }

        Ok(out)
    }
}
