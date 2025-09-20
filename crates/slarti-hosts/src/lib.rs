pub struct HostsPanelProps {
    pub tree: slarti_sshcfg::model::ConfigTree,
    pub on_select: Box<dyn Fn(String) + Send + Sync>, // alias selected
}

pub fn make_hosts_panel(props: HostsPanelProps) -> gpui::View<HostsPanel> { /* ... */
}

// Renders a tree like:
// hosts
//   ├─ dimitar-talev-rack-1
//   │    ├─ mitko (10.9.1.101)
//   │    ├─ hawalius (10.9.1.102)
//   │    └─ ...
//   └─ hetzner
//        ├─ marvin (...)
//        └─ zaphod (...)
