use gpui::{
    div, prelude::*, px, App, Context, FocusHandle, Focusable, MouseButton, SharedString, Window,
};
use slarti_ui::Vector as UiVector;
use std::sync::Arc;

/// Properties for constructing a HostPanel.
///
/// Initially, this panel renders placeholders for various observability
/// facets (identity, services, metrics, etc). In the near future, this
/// panel will be populated with real data queried from the selected host.
pub struct HostPanelProps {
    /// The currently selected host alias (from the hosts panel), if any.
    pub selected_alias: Option<String>,
    /// Optional deploy callback invoked when the 'Deploy agent' button is clicked.
    pub on_deploy: Option<Arc<dyn Fn(&mut Window, &mut Context<HostPanel>) + Send + Sync>>,
}

/// HostPanel shows high-level information and observations about the
/// currently selected host. For now it renders a set of placeholder
/// sections to guide future observability work.
pub struct HostPanel {
    focus: FocusHandle,
    selected_alias: Option<String>,
    // Remote status and lightweight progress
    status: SharedString,
    checking: bool,
    last_progress: Option<SharedString>,
    // Optional deploy callback
    on_deploy: Option<Arc<dyn Fn(&mut Window, &mut Context<HostPanel>) + Send + Sync>>,
    // Deployment state for button behavior/animation
    deploy_running: bool,
    has_deployed: bool,
    // Recently selected hosts (most-recent first, unique)
    recent_hosts: Vec<String>,
}

impl HostPanel {
    /// Create a new HostPanel.
    pub fn new(cx: &mut Context<Self>, props: HostPanelProps) -> Self {
        Self {
            focus: cx.focus_handle(),
            selected_alias: props.selected_alias,
            status: SharedString::from("unknown"),
            checking: false,
            last_progress: None,
            on_deploy: props.on_deploy,
            deploy_running: false,
            has_deployed: false,
            recent_hosts: Self::load_recent_hosts(),
        }
    }

    /// Update the selected host alias displayed by the panel.
    /// Call this from outside via entity.update to reflect host selection.
    pub fn set_selected_host(&mut self, alias: Option<String>, cx: &mut Context<Self>) {
        if let Some(a) = alias.as_ref() {
            self.push_recent(a);
            let _ = Self::save_recent_hosts(&self.recent_hosts);
        }
        self.selected_alias = alias;
        cx.notify();
    }

    /// Update the remote status text (e.g., "connected vX", "not present", "outdated").
    pub fn set_status(&mut self, status: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = status.into();
        cx.notify();
    }

    /// Toggle a lightweight "checking..." indicator.
    pub fn set_checking(&mut self, on: bool, cx: &mut Context<Self>) {
        self.checking = on;
        cx.notify();
    }

    /// Update the last progress message shown in the banner (optional).
    pub fn push_progress(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.last_progress = Some(msg.into());
        cx.notify();
    }

    /// Clear any progress message.
    pub fn clear_progress(&mut self, cx: &mut Context<Self>) {
        self.last_progress = None;
        cx.notify();
    }

    /// Append an alias to the MRU list (dedupe, cap at 5).
    fn push_recent(&mut self, alias: &str) {
        self.recent_hosts.retain(|h| h != alias);
        self.recent_hosts.insert(0, alias.to_string());
        if self.recent_hosts.len() > 5 {
            self.recent_hosts.truncate(5);
        }
    }

    /// Load recent hosts from state dir.
    fn load_recent_hosts() -> Vec<String> {
        let path = Self::recent_state_path();
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(list) = serde_json::from_slice::<Vec<String>>(&bytes) {
                return list;
            }
        }
        Vec::new()
    }

    /// Save recent hosts to state dir.
    fn save_recent_hosts(list: &Vec<String>) -> std::io::Result<()> {
        if let Some(dir) = Self::state_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let mut p = dir;
            p.push("hosts_recent.json");
            let data = serde_json::to_vec_pretty(list)
                .unwrap_or_else(|_| serde_json::to_vec(list).unwrap());
            std::fs::write(p, data)
        } else {
            // Fallback: HOME not set; no-op
            Ok(())
        }
    }

    /// Determine state directory: $XDG_STATE_HOME/slarti or ~/.local/state/slarti
    fn state_dir() -> Option<std::path::PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            let mut p = std::path::PathBuf::from(xdg);
            p.push("slarti");
            return Some(p);
        }
        if let Ok(home) = std::env::var("HOME") {
            let mut p = std::path::PathBuf::from(home);
            p.push(".local");
            p.push("state");
            p.push("slarti");
            return Some(p);
        }
        None
    }

    fn recent_state_path() -> std::path::PathBuf {
        let mut p = Self::state_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let _ = std::fs::create_dir_all(&p);
        p.push("hosts_recent.json");
        p
    }

    /// Set or update the deploy callback used when clicking the "Deploy agent" button.
    pub fn set_on_deploy(
        &mut self,
        cb: Option<Arc<dyn Fn(&mut Window, &mut Context<HostPanel>) + Send + Sync>>,
        cx: &mut Context<Self>,
    ) {
        self.on_deploy = cb;
        cx.notify();
    }

    /// Update deployment running state (used to disable the button and animate the icon).
    pub fn set_deploy_running(&mut self, running: bool, cx: &mut Context<Self>) {
        self.deploy_running = running;
        cx.notify();
    }

    /// Mark that a deployment has completed at least once (changes button alt to Redeploy).
    pub fn mark_deployed(&mut self, cx: &mut Context<Self>) {
        self.has_deployed = true;
        cx.notify();
    }

    fn render_section<'a>(
        &self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        depth: f32,
    ) -> impl IntoElement {
        let border = gpui::opaque_grey(0.2, 0.7);
        let fg_dim = gpui::opaque_grey(1.0, 0.85);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .pl(px(depth))
            .pr(px(8.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(border)
            .child(div().text_color(gpui::white()).child(title.into()))
            .child(div().text_color(fg_dim).child(body.into()))
    }
}

impl Focusable for HostPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for HostPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Colors consistent with the rest of the app.
        let bg = gpui::rgb(0x0b0b0b);
        let border = gpui::opaque_grey(0.2, 0.7);
        let fg = gpui::white();
        let fg_dim = gpui::opaque_grey(1.0, 0.85);

        let header = {
            let title = match self.selected_alias.as_ref() {
                Some(a) => format!("Host • {}", a),
                None => "Host".to_string(),
            };

            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(28.0))
                .px(px(8.0))
                .bg(bg)
                .border_b_1()
                .border_color(border)
                .text_color(fg)
                .child(title)
        };

        // Status banner: instantaneous render; updated by background tasks via setters.
        let status_banner = {
            let base = if self.checking {
                format!("Remote: {} (checking…)", self.status)
            } else {
                format!("Remote: {}", self.status)
            };
            let text = if let Some(p) = &self.last_progress {
                format!("{} — {}", base, p)
            } else {
                base
            };
            let row = div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(22.0))
                .px(px(8.0))
                .border_b_1()
                .border_color(border)
                .text_color(fg_dim)
                .child(text);
            if !self.checking {
                // Visible icon button (deploy/redeploy)
                let ms = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    % 1000) as f32;
                let phase = ms / 1000.0;
                let icon_alpha = if self.deploy_running {
                    0.4 + 0.6 * ((phase * std::f32::consts::PI * 2.0).sin().abs())
                } else {
                    1.0
                };
                let icon_color = gpui::hsla(0.6, 0.7, 0.7, icon_alpha);
                let btn = div()
                    .px(px(8.0))
                    .h(px(18.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(border)
                    .text_color(gpui::white())
                    .when(!self.deploy_running, |d| d.cursor_pointer())
                    .child(
                        UiVector::new("assets/terminal_alt.svg")
                            .square(px(14.0))
                            .color(icon_color)
                            .render(),
                    )
                    .on_mouse_up(MouseButton::Left, {
                        let cb = self.on_deploy.clone();
                        _cx.listener(
                            move |this: &mut Self,
                                  _ev: &gpui::MouseUpEvent,
                                  window: &mut Window,
                                  cx: &mut Context<HostPanel>| {
                                if this.deploy_running {
                                    return;
                                }
                                this.set_deploy_running(true, cx);
                                this.set_status(
                                    if this.has_deployed {
                                        "redeploying…"
                                    } else {
                                        "deploying…"
                                    },
                                    cx,
                                );
                                this.push_progress("uploading agent", cx);
                                if let Some(cb) = cb.as_ref() {
                                    (cb)(window, cx);
                                }
                            },
                        )
                    });
                row.child(btn)
            } else {
                row
            }
        };

        // If no host selected, show invitation and recent hosts only.
        if self.selected_alias.is_none() {
            let invite = div()
                .flex()
                .items_center()
                .h(px(36.0))
                .px(px(8.0))
                .text_color(gpui::white())
                .child("No host selected. Select a host from the left to view details.");

            // Recent list (up to 5)
            let recent_list = {
                let mut rows = Vec::new();
                for alias in self.recent_hosts.iter().take(5) {
                    let a = alias.clone();
                    rows.push(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(28.0))
                            .px(px(8.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(border)
                            .cursor_pointer()
                            .text_color(gpui::opaque_grey(1.0, 0.85))
                            .child(a.clone())
                            .on_mouse_up(MouseButton::Left, {
                                let alias2 = a.clone();
                                _cx.listener(move |this: &mut Self, _ev: &gpui::MouseUpEvent, _w: &mut Window, cx: &mut Context<HostPanel>| {
                                    // Update selection locally and persist recents.
                                    this.set_selected_host(Some(alias2.clone()), cx);
                                })
                            }),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pl(px(8.0))
                    .pr(px(8.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(border)
                    .child(div().text_color(gpui::white()).child("Recent"))
                    .child(div().flex().flex_col().gap_2().children(rows))
            };

            return div()
                .flex()
                .flex_col()
                .size_full()
                .bg(bg)
                .text_color(fg_dim)
                .child(header)
                .child(status_banner)
                .child(invite)
                .child(recent_list);
        }

        // Default (host selected): keep existing layout for now.
        // Minimal identity section while selected (placeholder retained for selected state only).
        let identity = self.render_section(
            "Identity",
            match self.selected_alias.as_ref() {
                Some(a) => {
                    let mut s = format!(
                        "alias: {}\nhostname: (pending)\nuser: (pending)\nproxy: (pending)\nport: (pending)",
                        a
                    );
                    if let Some(p) = &self.last_progress {
                        s.push_str(&format!("\nsystem: {}", p));
                    }
                    s
                }
                None => "No host selected.".into(),
            },
            8.0,
        );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .text_color(fg_dim)
            .child(header)
            .child(status_banner)
            .child(
                div()
                    .id("HostPanelScroll")
                    .flex()
                    .flex_col()
                    .size_full()
                    .overflow_y_scroll()
                    .child(identity),
            )
    }
}

/// Helper for constructing a HostPanel view within a window builder.
///
/// Usage:
///   let host_panel = cx.new(make_host_panel(HostPanelProps { selected_alias: None }));
pub fn make_host_panel(props: HostPanelProps) -> impl FnOnce(&mut Context<HostPanel>) -> HostPanel {
    move |cx| HostPanel::new(cx, props)
}
