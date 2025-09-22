use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Window, WindowBounds,
    WindowOptions,
};
use serde::{Deserialize, Serialize};
use slarti_host::{make_host_panel, HostPanel as HostInfoPanel, HostPanelProps as HostInfoProps};
use slarti_hosts::{make_hosts_panel, HostsPanel, HostsPanelProps};
use slarti_ssh::{check_agent, deploy_agent, remote_user_is_root, run_agent};
use slarti_sshcfg as sshcfg;
use slarti_ui::{FsAssets, Vector as UiVector};
use std::collections::HashMap;
use std::path::PathBuf;

use std::sync::{Arc, OnceLock};

use std::time::Duration;

static BG_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn bg_rt() -> &'static tokio::runtime::Runtime {
    BG_RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("init background runtime")
    })
}

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
    _selected_alias: Option<String>,
    _agent_status: RemoteAgentStatus,
    // Window state for custom titlebar behavior
    dragging_window: bool,
    _saved_windowed_bounds: Option<Bounds<Pixels>>,
    _is_maximized: bool,
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
            _selected_alias: None,
            _agent_status: RemoteAgentStatus::Unknown,
            dragging_window: false,
            _saved_windowed_bounds: None,
            _is_maximized: false,
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
    // Initialize logging via tracing-subscriber to respect RUST_LOG
    {
        // Avoid initializing multiple times in tests or hot-reload scenarios.
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .try_init();
        });
    }

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

                        // Shared current alias for actions like Deploy
                        let current_alias = Arc::new(std::sync::Mutex::new(None::<String>));
                        let current_alias_for_deploy = current_alias.clone();

                        // Build the host info panel (top half of right column) with a simplified Deploy callback.
                        // For now, we only surface a confirmation-style status update and progress note,
                        // deferring the actual deploy/run logic to a later change to unblock the build.
                        let host_info = cx.new(make_host_panel(HostInfoProps {
                            selected_alias: None,
                            on_deploy: None,
                        }));

                        // Wire deploy callback now that we have the entity handle
                        {
                            let host_info_handle2 = host_info.clone();
                            let current_alias_for_deploy = current_alias_for_deploy.clone();
                            host_info.update(cx, |panel, cx| {
                                let cb = {
                                    let host_handle = host_info_handle2.clone();
                                    let current_alias_sel = current_alias_for_deploy.clone();
                                    Arc::new(move |window: &mut Window, cxp: &mut Context<HostInfoPanel>| {
                                        // Initial UI state is handled by the HostPanel button handler to avoid re-entrant/private updates.

                                        // Spawn background deployment without blocking UI.
                                        let host_handle2 = host_handle.clone();
                                        let current_alias_sel2 = current_alias_sel.clone();
                                        let _ = window.spawn(cxp, async move |acx| {
                                            let _ = tokio::runtime::Builder::new_current_thread()
                                                .enable_all()
                                                .build()
                                                .map(|rt| {
                                                    rt.block_on(async {
                                                        // Determine target alias
                                                        let target = current_alias_sel2
                                                            .lock()
                                                            .ok()
                                                            .and_then(|g| g.clone());
                                                        if let Some(target) = target {
                                                            let version = env!("CARGO_PKG_VERSION").to_string();
                                                            let timeout = Duration::from_secs(10);

                                                            // Decide remote install path based on remote user.
                                                            let is_root = remote_user_is_root(&target, timeout)
                                                                .await
                                                                .unwrap_or(false);
                                                            let remote_dir = if is_root {
                                                                format!("/usr/local/lib/slarti/agent/{}", version)
                                                            } else {
                                                                format!("$HOME/.local/share/slarti/agent/{}", version)
                                                            };
                                                            let remote_path = format!("{}/slarti-remote", remote_dir);

                                                            // Resolve local artifact (prefer release, fallback to debug).
                                                            let mut artifact = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                                                            artifact.push("../../target/release/slarti-remote");
                                                            if !artifact.exists() {
                                                                let mut dbg = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                                                                dbg.push("../../target/debug/slarti-remote");
                                                                artifact = dbg;
                                                            }

                                                            if !artifact.exists() {
                                                                let _ = acx.update(|_w, cxu| {
                                                                    let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                        panel.set_status("deploy failed: local agent binary not found", cxu);
                                                                        panel.push_progress("build slarti-remote first", cxu);
                                                                        panel.set_deploy_running(false, cxu);
                                                                    });
                                                                });
                                                                return;
                                                            }

                                                            // Upload/install
                                                            let _ = acx.update(|_w, cxu| {
                                                                let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                    panel.push_progress("uploading agent", cxu);
                                                                });
                                                            });

                                                            match deploy_agent(&target, &artifact, &version, timeout).await {
                                                                Ok(_res) => {
                                                                    // Verify agent
                                                                    let _ = acx.update(|_w, cxu| {
                                                                        let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                            panel.push_progress("verifying agent", cxu);
                                                                        });
                                                                    });

                                                                    match check_agent(&target, &remote_path, timeout).await {
                                                                        Ok(status) if status.present && status.can_run => {
                                                                            // Handshake
                                                                            if let Ok(mut client) = run_agent(&target, &remote_path).await {
                                                                                if let Ok(hello) = client.hello(env!("CARGO_PKG_VERSION"), Some(timeout)).await {
                                                                                    let _ = acx.update(|_w, cxu| {
                                                                                        let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                                            panel.set_status(format!("connected v{}", hello.agent_version), cxu);
                                                                                            panel.set_deploy_running(false, cxu);
                                                                                            panel.mark_deployed(cxu);
                                                                                            panel.set_checking(false, cxu);
                                                                                        });
                                                                                    });
                                                                                } else {
                                                                                    let _ = acx.update(|_w, cxu| {
                                                                                        let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                                            panel.set_status("agent responded, handshake failed", cxu);
                                                                                            panel.set_deploy_running(false, cxu);
                                                                                            panel.mark_deployed(cxu);
                                                                                        });
                                                                                    });
                                                                                }
                                                                                let _ = client.terminate().await;
                                                                            } else {
                                                                                let _ = acx.update(|_w, cxu| {
                                                                                    let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                                        panel.set_status("agent started but could not open session", cxu);
                                                                                        panel.set_deploy_running(false, cxu);
                                                                                        panel.mark_deployed(cxu);
                                                                                    });
                                                                                });
                                                                            }
                                                                        }
                                                                        Ok(_) => {
                                                                            let _ = acx.update(|_w, cxu| {
                                                                                let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                                    panel.set_status("agent deployed but not runnable", cxu);
                                                                                    panel.set_deploy_running(false, cxu);
                                                                                    panel.mark_deployed(cxu);
                                                                                });
                                                                            });
                                                                        }
                                                                        Err(e) => {
                                                                            let msg = format!("agent verification failed: {}", e);
                                                                            let _ = acx.update(|_w, cxu| {
                                                                                let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                                    panel.set_status(msg, cxu);
                                                                                    panel.set_deploy_running(false, cxu);
                                                                                });
                                                                            });
                                                                        }
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    let msg = format!("deploy failed: {}", e);
                                                                    let _ = acx.update(|_w, cxu| {
                                                                        let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                            panel.set_status(msg, cxu);
                                                                            panel.set_deploy_running(false, cxu);
                                                                        });
                                                                    });
                                                                }
                                                            }
                                                        } else {
                                                            let _ = acx.update(|_w, cxu| {
                                                                let _ = host_handle2.update(cxu, |panel, cxu| {
                                                                    panel.set_status("no target selected", cxu);
                                                                    panel.set_deploy_running(false, cxu);
                                                                });
                                                            });
                                                        }
                                                    })
                                                });
                                        });
                                    })
                                };
                                panel.set_on_deploy(Some(cb), cx);
                            });
                        }

                        // Build the hosts panel from parsed SSH config.
                        let host_info_handle = host_info.clone();
                        let host_info_handle_for_recent = host_info_handle.clone();
                        let current_alias_sel = current_alias.clone();

                        // Load SSH config once and reuse for both tree rendering and selection path.
                        let cfg_tree = sshcfg::load::load_user_config_tree().unwrap_or_else(|_| {
                            sshcfg::model::ConfigTree {
                                root: sshcfg::model::FileNode {
                                    path: std::path::PathBuf::from("~/.ssh/config"),
                                    hosts: vec![],
                                    includes: vec![],
                                    matches: vec![],
                                },
                            }
                        });
                        let cfg_tree_for_select = cfg_tree.clone();

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
                                // Track the most recent alias for actions like Deploy
                                if let Ok(mut g) = current_alias_sel.lock() {
                                    *g = Some(alias.clone());
                                }

                                // Spawn an async task to check agent presence/version and persist state.
                                let target = alias.clone();
                                let version = env!("CARGO_PKG_VERSION").to_string();
                                let host_handle = host_info_handle.clone();
                                // Compute effective user locally from SSH config to avoid moving cfg_tree_for_select into the async closure,
                                // keeping this on_select closure Fn rather than FnOnce.
                                let user_is_root =
                                    sshcfg::load::effective_user_for_alias(&cfg_tree_for_select, &target)
                                        .as_deref()
                                        == Some("root");
                                window
                                    .spawn(hosts_cx, async move |acx| {
                                        // Run SSH/process IO on the global background runtime.
                                        let mut sys_summary: Option<String> = None;
                                        bg_rt().block_on(async {
                                            // NOTE: rsync/scp deployment will respect your SSH config (including ProxyJump)
                                            // because we invoke the system ssh/rsync binaries and inherit environment.
                                            // Increase SSH operation timeout for slower or multi-hop (ProxyJump) connections.
                                            let timeout = {
                                                // Per-host timeout precedence:
                                                // 1) SLARTI_SSH_TIMEOUT_SECS_<ALIAS_IN_UPPERCASE>
                                                // 2) SLARTI_SSH_TIMEOUT_SECS
                                                // 3) default 3s
                                                let env_key = format!(
                                                    "SLARTI_SSH_TIMEOUT_SECS_{}",
                                                    target.to_uppercase()
                                                );
                                                let per_host = std::env::var(&env_key)
                                                    .ok()
                                                    .and_then(|s| s.parse::<u64>().ok());
                                                let global = std::env::var("SLARTI_SSH_TIMEOUT_SECS")
                                                    .ok()
                                                    .and_then(|s| s.parse::<u64>().ok());
                                                Duration::from_secs(per_host.or(global).unwrap_or(3))
                                            };

                                            // Choose remote install path from SSH config (avoid SSH roundtrip).
                                            // If the configured User is "root" for this alias, use the system path; otherwise use user-level path.
                                            // user_is_root computed before spawn to avoid moving cfg_tree_for_select into this closure.
                                                    let remote_dir = if user_is_root {
                                                        format!("/usr/local/lib/slarti/agent/{}", version)
                                                    } else {
                                                        format!("$HOME/.local/share/slarti/agent/{}", version)
                                                    };
                                                    let remote_path = format!("{}/slarti-remote", remote_dir);

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
                                                    tracing::debug!(
                                                        target: "slarti_ssh",
                                                        "[slarti/select] check_agent target={} timeout={:?} remote_path={}",
                                                        target,
                                                        timeout,
                                                        remote_path
                                                    );
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

                                                                    // Request SysInfo and persist a snapshot
                                                                    // Import protocol types locally and track sys_info summary
                                                                    use slarti_proto::{Command as ProtoCommand, Response as ProtoResponse};

                                                                    let _ = client
                                                                        .send_command(&ProtoCommand::SysInfo { id: 2 })
                                                                        .await;

                                                                    if let Ok(resp) = client.read_response_line().await {
                                                                        if let ProtoResponse::SysInfoOk { id: _, info } = resp {
                                                                            // Build a short summary for the HostPanel banner
                                                                            sys_summary = Some(format!(
                                                                                "{} {} {} host:{} uptime:{}s",
                                                                                info.os,
                                                                                info.kernel,
                                                                                info.arch,
                                                                                info.hostname,
                                                                                info.uptime_secs
                                                                            ));
                                                                            // Persist snapshot under state dir
                                                                            let mut snap_dir = slarti_state_dir();
                                                                            snap_dir.push("hosts");
                                                                            let _ = std::fs::create_dir_all(&snap_dir);
                                                                            let mut snap_path = snap_dir.clone();
                                                                            snap_path.push(format!("{}-sys_info.json", target));
                                                                            let _ = std::fs::write(
                                                                                snap_path,
                                                                                serde_json::to_vec_pretty(&info)
                                                                                    .unwrap_or_else(|_| serde_json::to_vec(&info).unwrap()),
                                                                            );
                                                                        }
                                                                    }
                                                                }
                                                                let _ = client.terminate().await;
                                                            }
                                                        }
                                                        Ok(_) => {
                                                            // Not present or not runnable; leave last_seen_ok = false and keep path for future deploy.
                                                        }
                                                        Err(e) => {
                                                            eprintln!(
                                                                "agent check failed for {}: {}. Hint: we inherit your SSH config (including ProxyJump). If this is a timeout, try increasing the app SSH timeout for this host (SLARTI_SSH_TIMEOUT_SECS or SLARTI_SSH_TIMEOUT_SECS_{}). Context: timeout={:?}, remote_path={}",
                                                                target,
                                                                e,
                                                                target.to_uppercase(),
                                                                timeout,
                                                                remote_path
                                                            );
                                                            // Surface error to HostPanel immediately
                                                            let msg = format!("error: {}", e);
                                                            let _ = acx.update(|_window, cx| {
                                                                let _ = host_handle.update(cx, |panel, cx| {
                                                                    panel.set_status(msg.clone(), cx);
                                                                    panel.push_progress("check failed", cx);
                                                                    panel.set_checking(false, cx);
                                                                });
                                                            });
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
                                                    let progress_done = sys_summary
                                                        .clone()
                                                        .unwrap_or_else(|| "check complete".to_string());
                                                    // Schedule UI update on the UI thread
                                                    let _ = acx.update(|_window, cx| {
                                                        let _ =
                                                            host_handle.update(cx, |panel, cx| {
                                                                panel.set_status(
                                                                    status_text.clone(),
                                                                    cx,
                                                                );
                                                                panel.push_progress(
                                                                    progress_done.clone(),
                                                                    cx,
                                                                );
                                                                panel.set_checking(false, cx);
                                                            });
                                                    });
                                            });
                                    })
                                    .detach();
                            },
                        );

                        // Wire recent selection in HostPanel to reuse the same selection flow.
                        host_info.update(cx, |panel, cx| {
                            let host_info_handle_recent = host_info_handle_for_recent.clone();
                            let on_select_recent = {
                                let current_alias_sel = current_alias.clone();
                                let on_select_clone = on_select.clone();
                                Arc::new(move |alias: String, window: &mut Window, _cxp: &mut Context<HostInfoPanel>| {
                                    // Mirror HostsPanel selection: update HostPanel immediately, then run the same background flow.
                                    let _ = host_info_handle_recent.update(_cxp, |panel2, cx2| {
                                        panel2.set_selected_host(Some(alias.clone()), cx2);
                                        panel2.set_status("checking", cx2);
                                        panel2.set_checking(true, cx2);
                                        panel2.clear_progress(cx2);
                                        panel2.push_progress("probing agent…", cx2);
                                    });
                                    if let Ok(mut g) = current_alias_sel.lock() {
                                        *g = Some(alias.clone());
                                    }
                                    (on_select_clone)(alias, window, unsafe { std::mem::transmute(_cxp) });
                                })
                            };
                            panel.set_on_select_recent(Some(on_select_recent), cx);
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
                .update(cx, |_, win, _cx| {
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

            // Deploy callback is wired earlier via host_info.set_on_deploy; no additional wiring needed here.

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
