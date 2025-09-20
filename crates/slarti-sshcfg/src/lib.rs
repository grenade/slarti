pub mod model {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[derive(Clone, Debug)]
    pub struct ConfigTree {
        pub root: FileNode, // usually ~/.ssh/config
    }

    #[derive(Clone, Debug)]
    pub struct FileNode {
        pub path: PathBuf,           // canonicalized
        pub hosts: Vec<HostEntry>,   // hosts declared directly in this file
        pub includes: Vec<FileNode>, // resolved Include targets
    }

    #[derive(Clone, Debug)]
    pub struct HostEntry {
        pub patterns: Vec<String>,            // ["mitko", "mitko.thgttg.com"]
        pub params: BTreeMap<String, String>, // hostname, user, port, identityfile, ...
        pub source: PathBuf,                  // which file
        pub line: usize,                      // defined line (for tooltips)
    }
}

pub mod load {
    use crate::model::ConfigTree;
    use anyhow::Result;

    /// Parse ~/.ssh/config and recursively resolve `Include`.
    pub fn load_user_config_tree() -> Result<ConfigTree> { /* ... */
    }

    /// Utility for a flat list of concrete aliases (no wildcards).
    pub fn list_aliases(tree: &ConfigTree) -> Vec<String> { /* ... */
    }
}
