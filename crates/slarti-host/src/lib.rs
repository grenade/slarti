use gpui::{
    div, prelude::*, px, App, Context, FocusHandle, Focusable, Pixels, SharedString, Window,
};

/// Properties for constructing a HostPanel.
///
/// Initially, this panel renders placeholders for various observability
/// facets (identity, services, metrics, etc). In the near future, this
/// panel will be populated with real data queried from the selected host.
pub struct HostPanelProps {
    /// The currently selected host alias (from the hosts panel), if any.
    pub selected_alias: Option<String>,
}

/// HostPanel shows high-level information and observations about the
/// currently selected host. For now it renders a set of placeholder
/// sections to guide future observability work.
pub struct HostPanel {
    focus: FocusHandle,
    selected_alias: Option<String>,
}

impl HostPanel {
    /// Create a new HostPanel.
    pub fn new(cx: &mut Context<Self>, props: HostPanelProps) -> Self {
        Self {
            focus: cx.focus_handle(),
            selected_alias: props.selected_alias,
        }
    }

    /// Update the selected host alias displayed by the panel.
    /// Call this from outside via entity.update to reflect host selection.
    pub fn set_selected_host(&mut self, alias: Option<String>, cx: &mut Context<Self>) {
        self.selected_alias = alias;
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

        // Placeholder sections to be wired with real data later:
        // - Identity: alias, hostname, user, proxy jump chain, ssh port
        // - Services/Workloads: systemd services not in baseline, containers, ports
        // - Metrics: CPU, memory, disk, network utilization
        // - Notes/Tags: user annotations to aid orchestration planning
        let identity = self.render_section(
            "Identity",
            match self.selected_alias.as_ref() {
                Some(a) => format!(
                    "alias: {}\nhostname: (pending)\nuser: (pending)\nproxy: (pending)\nport: (pending)",
                    a
                ),
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
            // Content uses a simple vertical stack for now; in the future we can move to a
            // grid if we want equal-height cards or multi-column layout.
            .child(identity)
            .child(services)
            .child(metrics)
            .child(planning)
    }
}

/// Helper for constructing a HostPanel view within a window builder.
///
/// Usage:
///   let host_panel = cx.new(make_host_panel(HostPanelProps { selected_alias: None }));
pub fn make_host_panel(props: HostPanelProps) -> impl FnOnce(&mut Context<HostPanel>) -> HostPanel {
    move |cx| HostPanel::new(cx, props)
}
