use gpui::{
    div, prelude::*, px, size, svg, App, Application, Bounds, Context, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Window, WindowBounds,
    WindowOptions,
};
use serde::{Deserialize, Serialize};
use slarti_host::{make_host_panel, HostPanel as HostInfoPanel, HostPanelProps as HostInfoProps};
use slarti_hosts::{make_hosts_panel, HostsPanel, HostsPanelProps};
use slarti_ssh::{check_agent, remote_user_is_root, run_agent};
use slarti_sshcfg as sshcfg;
use slarti_ui::{FsAssets, Vector as UiVector};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Persisted UI settings
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct UiSettings {
    /// Right column split top height in pixels
    split_top: f32,
    /// Last window bounds (windowed)
    last_window_bounds: Option<(i32, i32, u32, u32)>, // x, y, w, h
}

fn ui_settings_path() -> std::path::PathBuf {
    let mut dir = slarti_state_dir();
    dir.push("ui");
    let _ = std::fs::create_dir_all(&dir);
    dir.push("settings.json");
    dir
}

fn load_ui_settings() -> UiSettings {
    let path = ui_settings_path();
    if let Ok(s) = std::fs::read_to_string(path) {
        if let Ok(cfg) = serde_json::from_str::<UiSettings>(&s) {
            return cfg;
        }
    }
    UiSettings {
        split_top: 240.0,
        last_window_bounds: None,
    }
}

fn save_ui_settings(mut cfg: UiSettings) {
    // Clamp split_top to sane bounds before saving
    cfg.split_top = cfg.split_top.clamp(120.0, 600.0);
    let _ = std::fs::write(
        ui_settings_path(),
        serde_json::to_vec_pretty(&cfg).unwrap_or_else(|_| serde_json::to_vec(&cfg).unwrap()),
    );
}

/// Persistent agent deployment information for a host alias.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDeploymentState {
    pub alias: String,
    pub last_deployed_version: Option<String>,
    pub last_deployed_at: Option<String>, // RFC3339
    pub remote_path: Option<PathBuf>,
    pub remote_checksum: Option<String>,
    pub last_seen_ok: bool,
}

/// Live/known remote agent status for a host.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteAgentStatus {
    Unknown,
    NotPresent,
    Outdated { remote_version: Option<String> },
    Connecting,
    Connected { agent_version: String },
    Error { message: String },
}

/// Local persisted state store (per-app) keyed by host alias.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostStateStore {
    pub agents: HashMap<String, AgentDeploymentState>,
}

/// Basic state directory helpers for per-host agent state persistence.
fn slarti_state_dir() -> std::path::PathBuf {
    if let Some(mut dir) = dirs_next::data_local_dir() {
        dir.push("slarti");
        return dir;
    }
    // Fallback: ~/.local/state/slarti
    let mut home = dirs_next::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home.push(".local");
    home.push("state");
    home.push("slarti");
    home
}

fn slarti_agents_state_dir() -> std::path::PathBuf {
    let mut dir = slarti_state_dir();
    dir.push("agents");
    dir
}

fn agent_state_path(alias: &str) -> std::path::PathBuf {
    let mut p = slarti_agents_state_dir();
    p.push(format!("{}.json", alias));
    p
}

/// Load persisted deployment state for a host alias (if present).
fn load_agent_state(alias: &str) -> Option<AgentDeploymentState> {
    let path = agent_state_path(alias);
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<AgentDeploymentState>(&data).ok()
}

/// Save/update persisted deployment state for a host alias.
fn save_agent_state(state: &AgentDeploymentState) -> std::io::Result<()> {
    let dir = slarti_agents_state_dir();
    std::fs::create_dir_all(&dir)?;
    let path = agent_state_path(&state.alias);
    let data =
        serde_json::to_vec_pretty(state).unwrap_or_else(|_| serde_json::to_vec(state).unwrap());
    std::fs::write(path, data)
}

/// Minimal Vector wrapper around gpui::svg() to support Vector::color() like Zed.
///
/// Usage:
/// Vector::new("assets/icon.svg", px(14.0))
///     .color(gpui::hsla(...))
///     .render()

/// Minimal Vector wrapper around gpui::svg() to support Vector::color(...).render() like Zed.
///
/// Usage:
/// Vector::new("assets/icon.svg", px(14.0))
///     .color(gpui::hsla(...))
///     .render()
// Terminal panel from the slarti-term crate
use slarti_term::{TerminalConfig, TerminalView};

struct ContainerView {
    focus: FocusHandle,
    // Panels
    terminal: gpui::Entity<TerminalView>,
    hosts: gpui::Entity<HostsPanel>,
    host_info: gpui::Entity<HostInfoPanel>,
    terminal_collapsed: bool,
    ui_fg: (f32, f32, f32, f32),
    // Split state for right column (top host info vs bottom terminal)
    split_top: f32,
    dragging_split: bool,
    last_split_y: f32,
    // Remote/selection state
    selected_alias: Option<String>,
    agent_status: RemoteAgentStatus,
    // Window state for custom titlebar behavior
    dragging_window: bool,
    saved_windowed_bounds: Option<Bounds<Pixels>>,
    is_maximized: bool,
}

impl ContainerView {
    fn new(
        cx: &mut Context<Self>,
        terminal: gpui::Entity<TerminalView>,
        hosts: gpui::Entity<HostsPanel>,
        host_info: gpui::Entity<HostInfoPanel>,
        ui_fg: (f32, f32, f32, f32),
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            terminal,
            hosts,
            host_info,
            terminal_collapsed: false,
            ui_fg,
            // load persisted UI settings (split position)
            split_top: load_ui_settings().split_top,
            dragging_split: false,
            last_split_y: 0.0,
            selected_alias: None,
            agent_status: RemoteAgentStatus::Unknown,
            dragging_window: false,
            saved_windowed_bounds: None,
            is_maximized: false,
        }
    }

    // Header controls: left menu is a placeholder for now.
    fn on_close(&mut self, _: &MouseUpEvent, window: &mut Window, _cx: &mut Context<Self>) {
        // Close just removes the current window. A multi-window shell can intercept differently.
        window.remove_window();
    }

    fn on_minimize(&mut self, _: &MouseUpEvent, window: &mut Window, _cx: &mut Context<Self>) {
        // Minimize using the platform window control.
        window.minimize_window();
    }

    fn on_maximize(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Toggle platform zoom/maximize and request repaint so the icon swaps dynamically.
        window.zoom_window();
        cx.notify();
    }
    // Custom titlebar drag-to-move (Wayland-friendly)
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
        ev: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dragging_window = false;
        // Double-click to toggle maximize/restore
        if ev.click_count >= 2 {
            window.zoom_window();
            cx.notify();
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

    // Split drag handlers
    fn on_split_mouse_down(
        &mut self,
        _ev: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dragging_split = true;
        // Use window-space Y to avoid local-coordinate jitter as layout changes.
        self.last_split_y = window.mouse_position().y.0;
    }

    fn on_split_mouse_up(
        &mut self,
        _ev: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dragging_split {
            self.dragging_split = false;
            // persist split position
            let mut ui = load_ui_settings();
            ui.split_top = self.split_top;
            save_ui_settings(ui);
            cx.notify();
        }
    }

    fn on_split_mouse_move(
        &mut self,
        _ev: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dragging_split {
            // Use window-space Y delta for stable dragging regardless of layout changes.
            let y = window.mouse_position().y.0;
            let dy = y - self.last_split_y;
            self.last_split_y = y;
            // Adjust split height and clamp to sane bounds
            let min_h = 120.0f32;
            let max_h = 600.0f32;
            self.split_top = (self.split_top + dy).clamp(min_h, max_h);
            cx.notify();
        }
    }

    fn on_focus_click(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle(cx));
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
        let text_color = gpui::hsla(self.ui_fg.0, self.ui_fg.1, self.ui_fg.2, self.ui_fg.3);
        let debug_icons = std::env::var("SLARTI_UI_DEBUG")
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            })
            .unwrap_or(false);

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
                        div()
                            .size(px(14.0))
                            .when(debug_icons, |d| {
                                d.bg(gpui::opaque_grey(0.4, 0.5))
                                    .border_1()
                                    .border_color(gpui::yellow())
                            })
                            .window_control_area(gpui::WindowControlArea::Min)
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_minimize))
                            .child(
                                UiVector::new("assets/generic_minimize.svg")
                                    .square(px(14.0))
                                    .color(text_color)
                                    .render(),
                            ),
                    )
                    .child(
                        div()
                            .size(px(14.0))
                            .when(debug_icons, |d| {
                                d.bg(gpui::opaque_grey(0.4, 0.5))
                                    .border_1()
                                    .border_color(gpui::yellow())
                            })
                            .window_control_area(gpui::WindowControlArea::Max)
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_maximize))
                            .child(
                                UiVector::new(if window.is_maximized() {
                                    "assets/generic_restore.svg"
                                } else {
                                    "assets/generic_maximize.svg"
                                })
                                .square(px(14.0))
                                .color(text_color)
                                .render(),
                            ),
                    )
                    .child(
                        div()
                            .size(px(14.0))
                            .when(debug_icons, |d| {
                                d.bg(gpui::opaque_grey(0.4, 0.5))
                                    .border_1()
                                    .border_color(gpui::yellow())
                            })
                            .window_control_area(gpui::WindowControlArea::Close)
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_close))
                            .child(
                                UiVector::new("assets/generic_close.svg")
                                    .square(px(14.0))
                                    .color(text_color)
                                    .render(),
                            ),
                    ),
            );

        // Content: two columns - hosts (left), terminal (right).
        let content = {
            let bg = gpui::rgb(0x0b0b0b);

            // Left: hosts tree sidebar
            let sidebar = div()
                .flex()
                .flex_col()
                .w(px(260.0))
                .border_r_1()
                .border_color(chrome_border)
                .bg(bg)
                .child(self.hosts.clone());

            // Right: terminal panel fills remaining space
            let right_inner = div()
                .flex()
                .flex_col()
                .size_full()
                .relative()
                // Top half: host observability panel (placeholder content for now)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .h(px(self.split_top.clamp(120.0, 600.0)))
                        .border_b_1()
                        .border_color(chrome_border)
                        // Simple remote status header above the Host panel
                        .child(
                            div()
                                .h(px(24.0))
                                .px(px(8.0))
                                .text_color(gpui::opaque_grey(1.0, 0.85))
                                .child("Remote: unknown"),
                        )
                        .child(self.host_info.clone()),
                )
                // Draggable split handle between top and bottom
                .child(
                    div()
                        .h(px(6.0))
                        .cursor_ns_resize()
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::on_split_mouse_down))
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::on_split_mouse_up))
                        .on_mouse_up(MouseButton::Left, {
                            let top_ref = &mut self.split_top;
                            cx.listener(
                                move |this: &mut Self,
                                      _ev: &MouseUpEvent,
                                      _w: &mut Window,
                                      _cx: &mut Context<Self>| {
                                    let mut ui = load_ui_settings();
                                    ui.split_top = this.split_top;
                                    save_ui_settings(ui);
                                },
                            )
                        })
                        .on_mouse_move(cx.listener(Self::on_split_mouse_move))
                        .bg(gpui::opaque_grey(0.2, 0.2)),
                )
                // Full overlay to capture mouse while dragging over the entire right pane
                .when(self.dragging_split, |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset(px(0.0))
                            .cursor_ns_resize()
                            .on_mouse_move(cx.listener(Self::on_split_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_split_mouse_up)),
                    )
                })
                // Bottom half: terminal fills remaining space
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .size_full()
                        .when(!self.terminal_collapsed, |d| d.child(self.terminal.clone())),
                );

            let right = div()
                .flex()
                .flex_col()
                .size_full()
                .bg(bg)
                .child(right_inner);

            div()
                .flex()
                .flex_row()
                .size_full()
                .child(sidebar)
                .child(right)
        };

        // Footer: terminal toggle button uses icon instead of text.
        let footer = {
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
                    div()
                        .size(px(16.0))
                        .when(debug_icons, |d| {
                            d.bg(gpui::opaque_grey(0.4, 0.5))
                                .border_1()
                                .border_color(gpui::yellow())
                        })
                        .cursor_pointer()
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::on_toggle_terminal))
                        .child(
                            UiVector::new("assets/terminal.svg")
                                .square(px(16.0))
                                .color(if !self.terminal_collapsed {
                                    gpui::Hsla::from(gpui::rgba(0x74ace6ff))
                                } else {
                                    text_color
                                })
                                .render(),
                        ),
                )
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
    Application::new()
        .with_assets(
            FsAssets::new().with_root(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"),
            ),
        )
        .run(|cx: &mut App| {
            // Load last UI settings to restore window bounds if available
            let ui = load_ui_settings();
            let default_bounds = Bounds::centered(None, size(px(1000.0), px(700.0)), cx);
            let restored_bounds = ui.last_window_bounds.as_ref().map(|(x, y, w, h)| Bounds {
                origin: gpui::point(px(*x as f32), px(*y as f32)),
                size: gpui::size(px(*w as f32), px(*h as f32)),
            });
            let open_bounds = restored_bounds.unwrap_or(default_bounds);

            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(open_bounds)),
                        ..Default::default()
                    },
                    |_, cx| {
                        // Build the terminal panel from slarti-term.
                        let term_cfg = TerminalConfig::default();
                        let ui_fg = term_cfg.theme.fg;
                        let terminal = cx.new(|cx| TerminalView::new(cx, term_cfg));

                        // Build the host info panel (top half of right column).
                        let host_info = cx.new(make_host_panel(HostInfoProps {
                            selected_alias: None,
                        }));

                        // Build the hosts panel from parsed SSH config.
                        let host_info_handle = host_info.clone();
                        let on_select = Arc::new(
                            move |alias: String,
                                  window: &mut Window,
                                  hosts_cx: &mut Context<HostsPanel>| {
                                // Update the Host panel with the selected alias immediately.
                                let _ = host_info_handle.update(hosts_cx, |panel, cx| {
                                    panel.set_selected_host(Some(alias.clone()), cx);
                                    // Make the Host panel instantaneous: show progress immediately.
                                    panel.set_status("checking", cx);
                                    panel.set_checking(true, cx);
                                    panel.clear_progress(cx);
                                    panel.push_progress("probing agent…", cx);
                                });

                                // Spawn an async task to check agent presence/version and persist state.
                                let target = alias.clone();
                                let version = env!("CARGO_PKG_VERSION").to_string();
                                let host_handle = host_info_handle.clone();
                                window
                                    .spawn(hosts_cx, async move |acx| {
                                        // Create a dedicated Tokio runtime to run ssh/process IO.
                                        let _ = tokio::runtime::Builder::new_current_thread()
                                            .enable_all()
                                            .build()
                                            .map(|rt| {
                                                rt.block_on(async {
                                                    let timeout = Duration::from_secs(3);

                                                    // Decide remote install path based on remote user (root vs non-root).
                                                    let is_root =
                                                        remote_user_is_root(&target, timeout)
                                                            .await
                                                            .unwrap_or(false);
                                                    let remote_dir = if is_root {
                                                        format!(
                                                            "/usr/local/lib/slarti/agent/{}",
                                                            version
                                                        )
                                                    } else {
                                                        format!(
                                                            "$HOME/.local/share/slarti/agent/{}",
                                                            version
                                                        )
                                                    };
                                                    let remote_path =
                                                        format!("{}/slarti-remote", remote_dir);

                                                    // Initialize a state record for this host.
                                                    let mut state = AgentDeploymentState {
                                                        alias: target.clone(),
                                                        last_deployed_version: None,
                                                        last_deployed_at: None,
                                                        remote_path: Some(
                                                            std::path::PathBuf::from(
                                                                remote_path.clone(),
                                                            ),
                                                        ),
                                                        remote_checksum: None,
                                                        last_seen_ok: false,
                                                    };

                                                    // Check agent presence/version, then attempt a Hello handshake.
                                                    match check_agent(
                                                        &target,
                                                        &remote_path,
                                                        timeout,
                                                    )
                                                    .await
                                                    {
                                                        Ok(status)
                                                            if status.present && status.can_run =>
                                                        {
                                                            // Try to connect and perform Hello/HelloAck.
                                                            if let Ok(mut client) =
                                                                run_agent(&target, &remote_path)
                                                                    .await
                                                            {
                                                                if let Ok(hello) = client
                                                                    .hello(
                                                                        env!("CARGO_PKG_VERSION"),
                                                                        Some(timeout),
                                                                    )
                                                                    .await
                                                                {
                                                                    state.last_deployed_version =
                                                                        Some(
                                                                            hello
                                                                                .agent_version
                                                                                .clone(),
                                                                        );
                                                                    state.last_seen_ok = true;
                                                                }
                                                                let _ = client.terminate().await;
                                                            }
                                                        }
                                                        Ok(_) => {
                                                            // Not present or not runnable; leave last_seen_ok = false and keep path for future deploy.
                                                        }
                                                        Err(e) => {
                                                            eprintln!(
                                                                "agent check failed for {}: {}",
                                                                target, e
                                                            );
                                                        }
                                                    }

                                                    let _ = save_agent_state(&state);
                                                    // Compute status text and update HostPanel
                                                    let status_text = if state.last_seen_ok {
                                                        match &state.last_deployed_version {
                                                            Some(v) => format!("connected v{}", v),
                                                            None => "connected".to_string(),
                                                        }
                                                    } else {
                                                        "not present or incompatible".to_string()
                                                    };
                                                    let progress_done = "check complete";
                                                    // Schedule UI update on the UI thread
                                                    let _ = acx.update(|_window, cx| {
                                                        let _ =
                                                            host_handle.update(cx, |panel, cx| {
                                                                panel.set_status(
                                                                    status_text.clone(),
                                                                    cx,
                                                                );
                                                                panel.push_progress(
                                                                    progress_done,
                                                                    cx,
                                                                );
                                                                panel.set_checking(false, cx);
                                                            });
                                                    });
                                                })
                                            });
                                    })
                                    .detach();
                            },
                        );
                        let cfg_tree = sshcfg::load::load_user_config_tree().unwrap_or_else(|_| {
                            sshcfg::model::ConfigTree {
                                root: sshcfg::model::FileNode {
                                    path: std::path::PathBuf::from("~/.ssh/config"),
                                    hosts: vec![],
                                    includes: vec![],
                                },
                            }
                        });
                        let hosts = cx.new(make_hosts_panel(HostsPanelProps {
                            tree: cfg_tree,
                            on_select: on_select.clone(),
                        }));
                        // Build the container that will host panels (hosts + host_info + terminal).
                        cx.new(|cx| ContainerView::new(cx, terminal, hosts, host_info, ui_fg))
                    },
                )
                .unwrap();

            // Save window bounds on every next frame (cheap sampling), and also on app quit.
            let window_clone = window;
            window_clone
                .update(cx, |_, win, cx| {
                    win.on_next_frame(move |w, _cx| {
                        let b = w.bounds();
                        let mut ui = load_ui_settings();
                        ui.last_window_bounds = Some((
                            b.origin.x.0 as i32,
                            b.origin.y.0 as i32,
                            b.size.width.0 as u32,
                            b.size.height.0 as u32,
                        ));
                        save_ui_settings(ui);
                    });
                })
                .ok();

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
                        "enter" => Some(b"\r"), // normalize to CR to avoid extra blank prompts across shells
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
