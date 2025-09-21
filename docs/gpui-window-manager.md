# GPUI window manager notes

This document summarizes how GPUI/Zed initiates window drag/resize from custom chrome, and how it reads/sets window bounds and interacts with displays across platforms.

Scope:
- Initiating a window drag from a custom titlebar
- Initiating a resize from edges/corners
- Getting/setting bounds and working with monitors (including maximize/restore)

The references below show the concrete method names and where the logic lives.

--------------------------------------------------------------------------------

High-level building blocks

- Marking draggable/control regions in the UI
  - `InteractiveElement::window_control_area(WindowControlArea::Drag)`
  - During painting, GPUI records these areas via `Window::insert_window_control_hitbox(...)`.
  - The platform layer can consult these hitboxes when deciding how to treat mouse events in those regions.
  - Example marking of drag region in the title bar:
```zed/crates/title_bar/src/platform_title_bar.rs#L76-83
        let title_bar = h_flex()
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(height)
            .map(|this| {
                if window.is_fullscreen() {
                    this.pl_2()
                } else if self.platform_style == PlatformStyle::Mac {
```

- Programmatic window movement/resize API
  - `Window::start_window_move()` → delegates to the platform’s `PlatformWindow::start_window_move()`
  - `Window::start_window_resize(edge: ResizeEdge)` → delegates to `PlatformWindow::start_window_resize(edge)`
```zed/crates/gpui/src/window.rs#L1738-1755
    /// Opens the native title bar context menu, useful when implementing client side decorations (Wayland and X11)
    pub fn show_window_menu(&self, position: Point<Pixels>) {
        self.platform_window.show_window_menu(position)
    }

    /// Tells the compositor to take control of window movement (Wayland and X11)
    ///
    /// Events may not be received during a move operation.
    pub fn start_window_move(&self) {
        self.platform_window.start_window_move()
    }
```

- Bounds and display APIs
  - Read current window geometry: `Window::bounds()`
  - Persistable open-state: `Window::window_bounds()` returns `WindowBounds::{Windowed|Maximized|Fullscreen}(restore_bounds)`
  - Client-area bounds (Wayland/X11): `Window::inner_window_bounds()`
  - Resize content area: `Window::resize(Size<Pixels>)`
  - Display queries: `App::{displays(), primary_display(), find_display(DisplayId)}`
  - Display geometry: `PlatformDisplay::bounds()`
  - Helpers for initial placement: `Bounds::{centered(display_id, size, cx), maximized(display_id, cx)}`
```zed/crates/gpui/src/window.rs#L1686-1695
    /// Returns the bounds of the current window in the global coordinate space, which could span across multiple displays.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.platform_window.bounds()
    }

    /// Set the content size of the window.
    pub fn resize(&mut self, size: Size<Pixels>) {
        self.platform_window.resize(size);
    }
```

--------------------------------------------------------------------------------

Starting a window drag from a custom titlebar

Cross-platform UI:
- Mark the custom titlebar region as draggable:
  - In the titlebar view (e.g. `PlatformTitleBar`), GPUI sets `.window_control_area(WindowControlArea::Drag)` on the titlebar container.
  - This generates a window-control hitbox that the platform layer can query.
```zed/crates/title_bar/src/platform_title_bar.rs#L120-158
            .when(!window.is_fullscreen(), |title_bar| {
                match self.platform_style {
                    PlatformStyle::Mac => title_bar,
                    PlatformStyle::Linux => {
                        if matches!(decorations, Decorations::Client { .. }) {
                            title_bar
                                .child(platform_linux::LinuxWindowControls::new(close_action))
                                .when(supported_controls.window_menu, |titlebar| {
                                    titlebar
                                        .on_mouse_down(MouseButton::Right, move |ev, window, _| {
                                            window.show_window_menu(ev.position)
                                        })
                                })
                                .on_mouse_move(cx.listener(move |this, _ev, window, _| {
                                    if this.should_move {
                                        this.should_move = false;
                                        window.start_window_move();
                                    }
                                }))
                                .on_mouse_down_out(cx.listener(move |this, _ev, _window, _cx| {
                                    this.should_move = false;
                                }))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _window, _cx| {
                                        this.should_move = false;
                                    }),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _window, _cx| {
                                        this.should_move = true;
                                    }),
                                )
                        } else {
                            title_bar
                        }
                    }
                    PlatformStyle::Windows => {
                        title_bar.child(platform_windows::WindowsWindowControls::new(height))
                    }
```

Windows:
- Windows uses non-client hit testing in the platform layer:
  - `on_hit_test_window_control` installs a callback GPUI uses to check if the mouse is currently over a recorded window-control hitbox and to identify the area (Drag/Close/Max/Min).
```zed/crates/gpui/src/window.rs#L1131-1151
        platform_window.on_hit_test_window_control({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, window, _cx| {
                        for (area, hitbox) in &window.rendered_frame.window_control_hitboxes {
                            if window.mouse_hit_test.ids.contains(&hitbox.id) {
                                return Some(*area);
                            }
                        }
                        None
                    })
                    .log_err()
                    .unwrap_or(None)
            })
        });
```
  - In the Windows platform event handling, `handle_hit_test_msg` maps areas to native non-client hit codes:
```zed/crates/gpui/src/platform/windows/events.rs#L859-867
                return match area {
                    WindowControlArea::Drag => Some(HTCAPTION as _),
                    WindowControlArea::Close => Some(HTCLOSE as _),
                    WindowControlArea::Max => Some(HTMAXBUTTON as _),
                    WindowControlArea::Min => Some(HTMINBUTTON as _),
                };
```
- No explicit call to `start_window_move()` is necessary on Windows; the OS performs the move when hit testing returns `HTCAPTION`.

Wayland/X11 (Linux):
- For client-side decorations, GPUI calls the compositor to move the window:
  - UI code (e.g. in `PlatformTitleBar`) wires mouse events so that on left down/move in the titlebar:
    - `window.start_window_move()` is invoked.
  - Wayland implementation:
```zed/crates/gpui/src/platform/linux/wayland/window.rs#L1032-1042
    fn start_window_move(&self) {
        let state = self.borrow();
        let serial = state.client.get_serial(SerialKind::MousePress);
        state.toplevel.move_(&state.globals.seat, serial);
    }

    fn start_window_resize(&self, edge: crate::ResizeEdge) {
        let state = self.borrow();
        state.toplevel.resize(
```
  - X11 implementation:
```zed/crates/gpui/src/platform/linux/x11/window.rs#L1479-1488
    fn start_window_move(&self) {
        const MOVERESIZE_MOVE: u32 = 8;
        self.send_moveresize(MOVERESIZE_MOVE).log_err();
    }

    fn start_window_resize(&self, edge: ResizeEdge) {
        self.send_moveresize(edge.to_moveresize()).log_err();
    }
```

macOS:
- The hit-test callback for window-control areas is currently a no-op in the macOS platform module.
- Dragging by custom background/titlebar is handled by AppKit when using standard title bars.
- GPUI does not currently route a custom `start_window_move()` on macOS.

--------------------------------------------------------------------------------

Starting a resize from an edge or corner

Cross-platform UI:
- Compute which edge/corner is being grabbed and call:
  - `Window::start_window_resize(edge: ResizeEdge)`

Edges enumeration:
- `ResizeEdge` = Top, TopRight, Right, BottomRight, Bottom, BottomLeft, Left, TopLeft
```zed/crates/gpui/src/platform.rs#L356-372
pub enum ResizeEdge {
    /// The top edge
    Top,
    /// The top right corner
    TopRight,
    /// The right edge
    Right,
    /// The bottom right corner
    BottomRight,
    /// The bottom edge
    Bottom,
    /// The bottom left corner
    BottomLeft,
    /// The left edge
    Left,
    /// The top left corner
    TopLeft,
}
```

Windows:
- Resizing is handled by non-client hit testing via `HT*` edge codes returned from the platform window procedure.
- `Window::start_window_resize(...)` is not used for Windows; it’s handled entirely by the OS via hit testing.

Wayland:
- `Window::start_window_resize(edge)` calls `PlatformWindow::start_window_resize(edge)` → `xdg_toplevel.resize(...)` (see snippet above).

X11:
- `_NET_WM_MOVERESIZE` action mapping:
```zed/crates/gpui/src/platform/linux/x11/window.rs#L97-107
impl ResizeEdge {
    fn to_moveresize(self) -> u32 {
        match self {
            ResizeEdge::TopLeft => 0,
            ResizeEdge::Top => 1,
            ResizeEdge::TopRight => 2,
            ResizeEdge::Right => 3,
            ResizeEdge::BottomRight => 4,
            ResizeEdge::Bottom => 5,
            ResizeEdge::BottomLeft => 6,
            ResizeEdge::Left => 7,
```
- Example usage in GPUI (edge detection and action):
```zed/crates/gpui/examples/window_shadow.rs#L84-87
                        match resize_edge(pos, shadow_size, size) {
                            Some(edge) => window.start_window_resize(edge),
                            None => window.start_window_move(),
                        };
```

--------------------------------------------------------------------------------

Getting/setting window bounds or monitor work area (for maximize/restore)

Reading bounds and state:
- Current global bounds: `Window::bounds()`
- Persistable open-state and restore bounds:
```zed/crates/gpui/src/window.rs#L1445-1473
    /// Check if the platform window is maximized
    /// On some platforms (namely Windows) this is different than the bounds being the size of the display
    pub fn is_maximized(&self) -> bool {
        self.platform_window.is_maximized()
    }

    /// request a certain window decoration (Wayland)
    pub fn request_decorations(&self, decorations: WindowDecorations) {
        self.platform_window.request_decorations(decorations);
    }

    /// Start a window resize operation (Wayland)
    pub fn start_window_resize(&self, edge: ResizeEdge) {
        self.platform_window.start_window_resize(edge);
    }

    /// Return the `WindowBounds` to indicate that how a window should be opened
    /// after it has been closed
    pub fn window_bounds(&self) -> WindowBounds {
        self.platform_window.window_bounds()
    }

    /// Return the `WindowBounds` excluding insets (Wayland and X11)
    pub fn inner_window_bounds(&self) -> WindowBounds {
        self.platform_window.inner_window_bounds()
    }
```
- The stored type for persistable state and restore bounds:
```zed/crates/gpui/src/platform.rs#L1190-1216
pub enum WindowBounds {
    /// Indicates that the window should open in a windowed state with the given bounds.
    Windowed(Bounds<Pixels>),
    /// Indicates that the window should open in a maximized state.
    /// The bounds provided here represent the restore size of the window.
    Maximized(Bounds<Pixels>),
    /// Indicates that the window should open in fullscreen mode.
    /// The bounds provided here represent the restore size of the window.
    Fullscreen(Bounds<Pixels>),
}

impl WindowBounds {
    /// Retrieve the inner bounds
    pub fn get_bounds(&self) -> Bounds<Pixels> {
        match self {
            WindowBounds::Windowed(bounds) => *bounds,
            WindowBounds::Maximized(bounds) => *bounds,
            WindowBounds::Fullscreen(bounds) => *bounds,
        }
    }
}
```
- Content size and current bounds:
```zed/crates/gpui/src/window.rs#L1686-1718
    /// Returns the bounds of the current window in the global coordinate space, which could span across multiple displays.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.platform_window.bounds()
    }

    /// Set the content size of the window.
    pub fn resize(&mut self, size: Size<Pixels>) {
        self.platform_window.resize(size);
    }

    /// Returns whether or not the window is currently fullscreen
    pub fn is_fullscreen(&self) -> bool {
        self.platform_window.is_fullscreen()
    }
```

Setting size:
- Resize the content area: `Window::resize(Size<Pixels>)` (see above).

Maximize/minimize/fullscreen:
- Toggle zoom (macOS-like maximize): `Window::zoom_window()`
- Minimize: `Window::minimize_window()` (via platform)
- Fullscreen toggle: `Window::toggle_fullscreen()`
- Check maximized state: `Window::is_maximized()` (see above)
- Windows caption button handling (min/max/close) in non-client handlers:
```zed/crates/gpui/src/platform/windows/events.rs#L1031-1068
                (HTMAXBUTTON, HTMAXBUTTON) => {
                    if self.state.borrow().is_maximized() {
                        unsafe { ShowWindowAsync(handle, SW_NORMAL).ok().log_err() };
                    } else {
                        unsafe { ShowWindowAsync(handle, SW_MAXIMIZE).ok().log_err() };
                    }
                    true
                }
```

Displays/monitors:
- Display geometry and listing:
```zed/crates/gpui/src/platform/windows/display.rs#L185-197
impl PlatformDisplay for WindowsDisplay {
    fn id(&self) -> DisplayId {
        self.display_id
    }

    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}
```
- Placement helpers:
```zed/crates/gpui/src/geometry.rs#L769-788
impl Bounds<Pixels> {
    /// Generate a centered bounds for the given display or primary display if none is provided
    pub fn centered(display_id: Option<DisplayId>, size: Size<Pixels>, cx: &App) -> Self {
        let display = display_id
            .and_then(|id| cx.find_display(id))
            .or_else(|| cx.primary_display());

        display
            .map(|display| Bounds::centered_at(display.bounds().center(), size))
            .unwrap_or_else(|| Bounds {
                origin: point(px(0.), px(0.)),
                size,
            })
    }

    /// Generate maximized bounds for the given display or primary display if none is provided
    pub fn maximized(display_id: Option<DisplayId>, cx: &App) -> Self {
```

Notes on “work area”
- GPUI’s cross-platform surface uses display `bounds()` rather than a distinct “work area” API.
- On Windows, maximize/minimize and caption buttons are delegated to the OS (e.g. `ShowWindowAsync(SW_MAXIMIZE)`), and GPUI listens for system parameter changes (e.g. `SPI_SETWORKAREA`) to keep internal settings (taskbar auto-hide) in sync:
```zed/crates/gpui/src/platform/windows/system_settings.rs#L45-51
        match wparam {
            // SPI_SETWORKAREA
            47 => self.update_taskbar_position(display),
            // SPI_GETWHEELSCROLLLINES, SPI_GETWHEELSCROLLCHARS
            104 | 108 => self.update_mouse_wheel_settings(),
            _ => {}
        }
```
- There is no public API in GPUI that returns the “work area” rectangle directly; placement helpers and display `bounds()` are the primary tools exposed.

--------------------------------------------------------------------------------

References (methods and where they are used)

UI-side (views/elements)
- `InteractiveElement::window_control_area(...)`
- Title bar view:
  - `crates/title_bar/src/platform_title_bar.rs`:
    - `.window_control_area(WindowControlArea::Drag)`
    - Linux CSD: mouse handlers call `window.start_window_move()`
    - Right-click menu: `window.show_window_menu(position)`

Core window API (GPUI)
- `crates/gpui/src/window.rs`:
  - `Window::start_window_move()`
  - `Window::start_window_resize(edge: ResizeEdge)`
  - `Window::bounds()`
  - `Window::window_bounds()` and `Window::inner_window_bounds()`
  - `Window::resize(Size<Pixels>)`
  - `Window::is_maximized()`, `Window::zoom_window()`, `Window::toggle_fullscreen()`
  - `Window::insert_window_control_hitbox(...)`
  - `Window::display(&App)`, `Window::scale_factor()`

Geometry and placement
- `crates/gpui/src/geometry.rs`:
  - `Bounds::centered(...)`
  - `Bounds::maximized(...)`

Platform abstraction
- `crates/gpui/src/platform.rs`:
  - Trait `PlatformWindow` (start move/resize, menu, etc.)
  - Trait `PlatformDisplay` (id/uuid/bounds)
  - `ResizeEdge` enum
  - `WindowBounds` enum and `get_bounds()`

Windows implementation
- `crates/gpui/src/platform/windows/events.rs`:
  - `handle_hit_test_msg` returns `HTCAPTION`, `HT*` edges, caption buttons
  - Non-client mouse handling for min/max/close and cursor
- `crates/gpui/src/platform/windows/window.rs`:
  - `WindowsWindowState::{is_maximized, window_bounds}` and restore bounds bookkeeping
- `crates/gpui/src/platform/windows/display.rs`:
  - Monitor enumeration and `PlatformDisplay::bounds()`
- `crates/gpui/src/platform/windows/system_settings.rs`:
  - Handling `SPI_SETWORKAREA` and other system parameter updates

Wayland (Linux) implementation
- `crates/gpui/src/platform/linux/wayland/window.rs`:
  - `PlatformWindow::start_window_move()` → compositor move
  - `PlatformWindow::start_window_resize(edge)` → `xdg_toplevel.resize`
  - `PlatformWindow::{window_bounds, inner_window_bounds}`

X11 (Linux) implementation
- `crates/gpui/src/platform/linux/x11/window.rs`:
  - `PlatformWindow::start_window_move()` → `_NET_WM_MOVERESIZE` (move)
  - `PlatformWindow::start_window_resize(edge)` → `_NET_WM_MOVERESIZE` (edge)
  - `ResizeEdge::to_moveresize()` mapping

Examples
- `crates/gpui/examples/window_shadow.rs`:
  - Demonstrates calling `window.start_window_resize(edge)` or `window.start_window_move()` based on hit testing around the edge.

--------------------------------------------------------------------------------

Quick answers

- Start window drag from custom titlebar:
  - Mark region: `window_control_area(WindowControlArea::Drag)`
  - Windows: platform hit-test returns `HTCAPTION` in that region → OS moves window
  - Wayland/X11 (CSD): call `Window::start_window_move()` from mouse handlers in that region

- Start resize from edge/corner:
  - Wayland/X11 (CSD): call `Window::start_window_resize(edge: ResizeEdge)`
  - Windows: let non-client hit testing return `HT*` edge codes; OS performs the resize

- Get/set bounds or monitor work area (for maximize):
  - Read: `Window::bounds()`, `Window::window_bounds()`, `Window::inner_window_bounds()`
  - Set content size: `Window::resize(Size<Pixels>)`
  - Displays: `App::displays()`, `App::primary_display()`, `PlatformDisplay::bounds()`
  - Helpers: `Bounds::centered(...)`, `Bounds::maximized(...)`
  - Maximize/restore behavior is platform-driven; GPUI tracks maximized/fullscreen state via `WindowBounds` and platform queries (e.g., `IsZoomed` on Windows)