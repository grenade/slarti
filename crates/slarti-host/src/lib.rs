use gpui::{
    div, prelude::*, px, App, Context, FocusHandle, Focusable, MouseButton, Pixels, SharedString,
    Window,
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
        }
    }

    /// Update the selected host alias displayed by the panel.
    /// Call this from outside via entity.update to reflect host selection.
    pub fn set_selected_host(&mut self, alias: Option<String>, cx: &mut Context<Self>) {
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

    fn render_cards<'a>(
        &self,
        title: impl Into<SharedString>,
        hints: &[&str],
        depth: f32,
    ) -> impl IntoElement {
        let border = gpui::opaque_grey(0.2, 0.7);
        let fg_dim = gpui::opaque_grey(1.0, 0.85);

        let mut rows = Vec::new();
        for hint in hints {
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
                    .text_color(fg_dim)
                    .child((*hint).to_string()),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .pl(px(depth))
            .pr(px(8.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(border)
            .child(div().text_color(gpui::white()).child(title.into()))
            .child(div().flex().flex_col().gap_2().children(rows))
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
                // While deployment is running, schedule continuous frames for blinking animation.
                // animation framing disabled to avoid re-entrant updates; icon alpha will update on other UI events
                // Visible icon button with fade animation while running.
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
                        UiVector::new("assets/deploy.svg")
                            .square(px(14.0))
                            .color(icon_color)
                            .render(),
                    )
                    // Trigger background deployment while keeping UI responsive.
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
                                this.set_status("deploying…", cx);
                                this.push_progress("uploading agent", cx);
                                // Spawn async task to invoke external deploy logic without re-entrant updates.
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

        // Placeholder sections to be wired with real data later:
        // - Identity: alias, hostname, user, proxy jump chain, ssh port
        // - Services/Workloads: systemd services not in baseline, containers, ports
        // - Metrics: CPU, memory, disk, network utilization
        // - Notes/Tags: user annotations to aid orchestration planning
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
                None => "No host selected.\nSelect a host from the left to view details.".into(),
            },
            8.0,
        );

        let services = self.render_cards(
            "Services & Workloads",
            &[
                "systemd: non-baseline services (pending)",
                "containers: running images (pending)",
                "listening ports (pending)",
            ],
            8.0,
        );

        let metrics = self.render_cards(
            "Metrics (live snapshots)",
            &[
                "cpu: — (pending)",
                "memory: — (pending)",
                "disk: — (pending)",
                "network: — (pending)",
            ],
            8.0,
        );

        let planning = self.render_section(
            "Planning Notes",
            "Add tags/notes to aid capacity and role assignment (pending).",
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
                    .child(identity)
                    .child(services)
                    .child(metrics)
                    .child(planning),
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
