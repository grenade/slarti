use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, App, Bounds, Context, FocusHandle, Focusable, MouseButton,
    MouseUpEvent, Pixels, Window,
};
use slarti_sshcfg::model::{ConfigTree, FileNode, HostEntry};

/// Input properties for the HostsPanel.
pub struct HostsPanelProps {
    /// Parsed SSH configuration tree (typically loaded from ~/.ssh/config).
    pub tree: ConfigTree,
    /// Callback invoked when a concrete host alias is selected.
    /// Parameters: (alias, &mut Window, &mut Context<HostsPanel>)
    pub on_select: Arc<dyn Fn(String, &mut Window, &mut Context<HostsPanel>) + Send + Sync>,
}

/// Renders an expandable tree of SSH hosts from an SSH config.
/// - Top-level label is "hosts".
/// - Each included file forms a group in the tree; hosts declared directly in the file appear as leaves.
/// - Clicking a host leaf invokes the provided `on_select(alias)` callback.
pub struct HostsPanel {
    focus: FocusHandle,
    tree: ConfigTree,
    on_select: Arc<dyn Fn(String, &mut Window, &mut Context<HostsPanel>) + Send + Sync>,
    // In a follow-up pass we can persist/restore expansion state by keying these with canonical paths.
    expanded_groups: std::collections::HashSet<String>,
}

impl HostsPanel {
    pub fn new(cx: &mut Context<Self>, props: HostsPanelProps) -> Self {
        let mut expanded = std::collections::HashSet::new();
        // Expand the root "hosts" node by default.
        expanded.insert("__root__".into());
        // Expand first-level groups by default to make discovery easier.
        for group in &props.tree.root.includes {
            expanded.insert(group_key(&group.path));
        }
        Self {
            focus: cx.focus_handle(),
            tree: props.tree,
            on_select: props.on_select,
            expanded_groups: expanded,
        }
    }

    fn on_toggle_group(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
        key: String,
    ) {
        if self.expanded_groups.contains(&key) {
            self.expanded_groups.remove(&key);
        } else {
            self.expanded_groups.insert(key);
        }
        cx.notify();
    }

    fn on_select_host(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
        alias: String,
    ) {
        (self.on_select)(alias, _window, _cx);
    }

    fn render_tree(
        &self,
        root: &FileNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Visual constants
        let bg = gpui::rgb(0x0b0b0b);
        let fg = gpui::white();
        let border = gpui::opaque_grey(0.2, 0.7);

        // Render root label and its children
        let mut children: Vec<AnyElement> = Vec::new();

        // Root header
        let root_key = "__root__".to_string();
        let root_expanded = self.expanded_groups.contains(&root_key);
        children.push(
            div()
                .flex()
                .items_center()
                .h(px(28.0))
                .px(px(8.0))
                .bg(bg)
                .border_b_1()
                .border_color(border)
                .text_color(fg)
                .cursor_pointer()
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener({
                        let key = root_key.clone();
                        move |this, ev, win, cx| this.on_toggle_group(ev, win, cx, key.clone())
                    }),
                )
                .child(if root_expanded {
                    "▾ hosts"
                } else {
                    "▸ hosts"
                })
                .into_any_element(),
        );

        // Root children
        if root_expanded {
            // Hosts declared directly in ~/.ssh/config (rare, but supported)
            if !root.hosts.is_empty() {
                children.push(
                    render_group_block(
                        "~/.ssh/config",
                        &group_key(&root.path),
                        &root.hosts,
                        &[],
                        1,
                        self,
                        window,
                        cx,
                    )
                    .into_any_element(),
                );
            }

            // Groups from includes
            for inc in &root.includes {
                children.push(
                    render_group_block(
                        &display_group_name(&inc.path),
                        &group_key(&inc.path),
                        &inc.hosts,
                        &inc.includes,
                        1,
                        self,
                        window,
                        cx,
                    )
                    .into_any_element(),
                );
            }
        }

        // Container
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .children(children)
    }
}

impl Focusable for HostsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for HostsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_tree(&self.tree.root, window, cx)
    }
}

// -----------------
// Render utilities
// -----------------

fn render_group_block(
    label: &str,
    key: &str,
    hosts: &[HostEntry],
    includes: &[FileNode],
    depth: usize,
    panel: &HostsPanel,
    window: &mut Window,
    cx: &mut Context<HostsPanel>,
) -> impl IntoElement {
    let fg = gpui::white();
    let border = gpui::opaque_grey(0.2, 0.7);

    let expanded = panel.expanded_groups.contains(key);
    let pad = px((depth as f32) * 16.0);

    let mut items: Vec<AnyElement> = Vec::new();

    // Group header
    items.push(
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(24.0))
            .pl(pad)
            .pr(px(8.0))
            .text_color(fg)
            .cursor_pointer()
            .on_mouse_up(
                MouseButton::Left,
                cx.listener({
                    let k = key.to_string();
                    move |this, ev, win, cx| this.on_toggle_group(ev, win, cx, k.clone())
                }),
            )
            // status dot (placeholder color for now)
            .child(
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(gpui::opaque_grey(1.0, 0.5)),
            )
            .child(if expanded {
                format!("▾ {}", label)
            } else {
                format!("▸ {}", label)
            })
            .into_any_element(),
    );

    if expanded {
        // Hosts in this group
        for host in hosts {
            if let Some(alias) = first_concrete_alias(host) {
                let display = format!(
                    "{}{}",
                    alias,
                    host.params
                        .get("hostname")
                        .map(|h| format!(" ({})", h))
                        .unwrap_or_default()
                );
                items.push(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(22.0))
                        .pl(px((depth as f32 + 1.0) * 24.0))
                        .pr(px(8.0))
                        .text_color(gpui::opaque_grey(1.0, 0.95))
                        .cursor_pointer()
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener({
                                let alias = alias.to_string();
                                move |this, ev, win, cx| {
                                    this.on_select_host(ev, win, cx, alias.clone())
                                }
                            }),
                        )
                        // status dot (placeholder color for now)
                        .child({
                            // Determine status color from cached agent state:
                            // - green: last_seen_ok == true
                            // - yellow: last_seen_ok == false && last_deployed_version present and != expected
                            // - red: last_seen_ok == false && last_deployed_version present and == expected
                            // - gray: no state
                            let expected = env!("CARGO_PKG_VERSION");
                            let color = (|| {
                                if let Some(mut p) = dirs_next::data_local_dir() {
                                    p.push("slarti");
                                    p.push("agents");
                                    p.push(format!("{}.json", alias));
                                    if let Ok(s) = std::fs::read_to_string(p) {
                                        #[derive(serde::Deserialize)]
                                        struct AgentState {
                                            last_seen_ok: bool,
                                            last_deployed_version: Option<String>,
                                        }
                                        if let Ok(st) = serde_json::from_str::<AgentState>(&s) {
                                            if st.last_seen_ok {
                                                return gpui::green();
                                            }
                                            if let Some(ver) = st.last_deployed_version {
                                                if ver != expected {
                                                    return gpui::yellow();
                                                }
                                                return gpui::red();
                                            }
                                        }
                                    }
                                }
                                gpui::opaque_grey(1.0, 0.5)
                            })();
                            div().w(px(6.0)).h(px(6.0)).rounded_full().bg(color)
                        })
                        .child(display)
                        .into_any_element(),
                );
            }
        }

        // Nested includes as sub-groups
        for inc in includes {
            items.push(
                render_group_block(
                    &display_group_name(&inc.path),
                    &group_key(&inc.path),
                    &inc.hosts,
                    &inc.includes,
                    depth + 1,
                    panel,
                    window,
                    cx,
                )
                .into_any_element(),
            );
        }
    }

    div()
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(border)
        .children(items)
}

// -------------
// Misc helpers
// -------------

fn first_concrete_alias(entry: &HostEntry) -> Option<&str> {
    entry
        .patterns
        .iter()
        .find(|p| !is_glob_pattern(p.as_str()))
        .map(|s| s.as_str())
}

fn is_glob_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

fn display_group_name(path: &std::path::Path) -> String {
    // Prefer file or last path component; fall back to full path string
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .or_else(|| path.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "group".into())
}

fn group_key(path: &std::path::Path) -> String {
    // A stable key for expansion map; prefer canonical path if available
    std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

// -----------------------
// Public construction API
// -----------------------

/// Helper for constructing a HostsPanel view inside a window builder.
/// Usage:
///   let panel = cx.new(|cx| HostsPanel::new(cx, props));
pub fn make_hosts_panel(
    props: HostsPanelProps,
) -> impl FnOnce(&mut Context<HostsPanel>) -> HostsPanel {
    move |cx| HostsPanel::new(cx, props)
}
