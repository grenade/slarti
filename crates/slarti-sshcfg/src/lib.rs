/*!
SSH config parser for Slarti.

Features:
- Parses ~/.ssh/config and recursively resolves Include directives.
- Handles tilde (~) expansion and glob patterns in Include paths.
- Builds a hierarchical tree of config files and their Host entries.
- Exposes a simple utility to list concrete (non-wildcard) host aliases.

This is not a fully-compliant OpenSSH parser, but supports the common subset:
- Host blocks: `Host alias1 alias2 ...`
- Parameters within Host blocks: `Param value` (quotes allowed)
- Include directives: `Include path/glob [path2/glob2 ...]` resolved relative to the including file
- Comments starting with '#' (outside of quotes)
- Ignores `Match` blocks (treated as non-host parsing context)

*/

use anyhow::{anyhow, Context, Result};
use glob::glob;
use regex::Regex;
use shellexpand::tilde;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub mod model {
    use super::*;

    /// A tree of SSH configuration files, starting from ~/.ssh/config by default.
    #[derive(Clone, Debug)]
    pub struct ConfigTree {
        pub root: FileNode, // usually ~/.ssh/config
    }

    /// A single parsed SSH config file.
    #[derive(Clone, Debug)]
    pub struct FileNode {
        pub path: PathBuf,           // canonicalized if possible
        pub hosts: Vec<HostEntry>,   // hosts declared directly in this file
        pub includes: Vec<FileNode>, // resolved Include targets
    }

    /// A single host entry as parsed from a `Host` block.
    #[derive(Clone, Debug)]
    pub struct HostEntry {
        pub patterns: Vec<String>,            // e.g. ["mitko", "mitko.thgttg.com"]
        pub params: BTreeMap<String, String>, // normalized param names, last occurrence wins
        pub source: PathBuf,                  // which file this entry came from
        pub line: usize, // at what line the Host declaration occurred (1-based)
    }

    impl HostEntry {
        /// Returns the parameter if present (case-insensitive key).
        pub fn get(&self, key: &str) -> Option<&str> {
            let k = key.to_ascii_lowercase();
            self.params.get(&k).map(|s| s.as_str())
        }
    }
}

pub mod load {
    use super::*;
    use crate::model::{ConfigTree, FileNode, HostEntry};

    /// Load and parse the user's SSH config (~/.ssh/config), resolving Include directives.
    pub fn load_user_config_tree() -> Result<ConfigTree> {
        let home =
            dirs_next::home_dir().ok_or_else(|| anyhow!("could not determine home directory"))?;
        let path = home.join(".ssh").join("config");
        load_from_path(&path)
    }

    /// Load and parse a config starting from a specific path.
    pub fn load_from_path(path: &Path) -> Result<ConfigTree> {
        let mut visited = HashSet::new();
        let root = parse_file_recursive(path, None, &mut visited)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(ConfigTree { root })
    }

    /// Returns a flat, sorted, unique list of concrete aliases (no wildcards) found in the tree.
    pub fn list_aliases(tree: &ConfigTree) -> Vec<String> {
        let mut set = BTreeSet::new();
        fn walk(node: &FileNode, set: &mut BTreeSet<String>) {
            for h in &node.hosts {
                for pat in &h.patterns {
                    if !is_glob_pattern(pat) {
                        set.insert(pat.clone());
                    }
                }
            }
            for inc in &node.includes {
                walk(inc, set);
            }
        }
        walk(&tree.root, &mut set);
        set.into_iter().collect()
    }

    // ----------------------
    // Parsing implementation
    // ----------------------

    fn parse_file_recursive(
        path: &Path,
        parent_dir: Option<&Path>,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<FileNode> {
        // Resolve path relative to parent if relative.
        let resolved = normalize_path(path, parent_dir);

        // Prevent cycles.
        let canon = canonicalize_best_effort(&resolved);
        if let Some(ref c) = canon {
            if !visited.insert(c.clone()) {
                // Already parsed
                return Ok(FileNode {
                    path: c.clone(),
                    hosts: vec![],
                    includes: vec![],
                });
            }
        }

        let text = fs::read_to_string(&resolved)
            .with_context(|| format!("reading SSH config {}", resolved.display()))?;
        let mut includes: Vec<FileNode> = Vec::new();
        let mut hosts: Vec<HostEntry> = Vec::new();

        // Current host block being assembled
        #[derive(Default)]
        struct CurrentHost {
            patterns: Vec<String>,
            params: BTreeMap<String, String>,
            start_line: usize,
        }
        let mut cur: Option<CurrentHost> = None;

        let mut line_no = 0usize;
        let mut skipping_match_block = false;

        for raw_line in text.lines() {
            line_no += 1;
            let line = strip_inline_comment(raw_line).trim().to_string();
            if line.is_empty() {
                continue;
            }

            // Tokenize (respecting quotes)
            let tokens = tokenize(&line);
            if tokens.is_empty() {
                continue;
            }

            let key = tokens[0].to_ascii_lowercase();

            // Handle Match blocks (ignore until next Host or end). This is a simplification.
            if key == "match" {
                skipping_match_block = true;
                continue;
            }
            if key == "host" {
                skipping_match_block = false;
            }
            if skipping_match_block {
                continue;
            }

            match key.as_str() {
                "include" => {
                    // Includes can list multiple patterns
                    let patterns = &tokens[1..];
                    if patterns.is_empty() {
                        continue;
                    }
                    for pat in patterns {
                        for inc_path in expand_include_pattern(pat, resolved.parent()) {
                            let sub = parse_file_recursive(&inc_path, None, visited)?;
                            includes.push(sub);
                        }
                    }
                }
                "host" => {
                    // Push previous
                    if let Some(prev) = cur.take() {
                        hosts.push(HostEntry {
                            patterns: prev.patterns,
                            params: prev.params,
                            source: canonicalize_best_effort(&resolved)
                                .unwrap_or_else(|| resolved.clone()),
                            line: prev.start_line,
                        });
                    }
                    // Start new
                    let patterns = tokens[1..]
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();
                    if patterns.is_empty() {
                        // malformed, ignore the block
                        cur = None;
                    } else {
                        cur = Some(CurrentHost {
                            patterns,
                            params: BTreeMap::new(),
                            start_line: line_no,
                        });
                    }
                }
                _ => {
                    // Parameter line inside a Host block
                    if let Some(ref mut h) = cur {
                        // params may have form: Key value(with spaces possibly quoted)
                        // tokens[1..] joined with single spaces as the value
                        if tokens.len() >= 2 {
                            let value = join_tokens(&tokens[1..]);
                            let k = key; // already lowercased
                                         // Keep last occurrence
                            h.params.insert(k, value);
                        }
                    }
                }
            }
        }

        // Push tail host
        if let Some(prev) = cur.take() {
            hosts.push(HostEntry {
                patterns: prev.patterns,
                params: prev.params,
                source: canonicalize_best_effort(&resolved).unwrap_or_else(|| resolved.clone()),
                line: prev.start_line,
            });
        }

        Ok(FileNode {
            path: canonicalize_best_effort(&resolved).unwrap_or(resolved),
            hosts,
            includes,
        })
    }

    // --------------------------------
    // Helpers: tokenization & includes
    // --------------------------------

    fn normalize_path(path: &Path, parent_dir: Option<&Path>) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        if path.starts_with("~") {
            let expanded = tilde(path.to_string_lossy().as_ref()).to_string();
            return PathBuf::from(expanded);
        }
        if let Some(base) = parent_dir {
            return base.join(path);
        }
        path.to_path_buf()
    }

    fn canonicalize_best_effort(path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok()
    }

    fn strip_inline_comment(line: &str) -> String {
        // Remove unquoted # and the rest of the line.
        // Handles both '...' and "..." quotes. No backslash escaping.
        let mut out = String::with_capacity(line.len());
        let mut in_squote = false;
        let mut in_dquote = false;
        for (i, ch) in line.chars().enumerate() {
            match ch {
                '\'' if !in_dquote => in_squote = !in_squote,
                '"' if !in_squote => in_dquote = !in_dquote,
                '#' if !in_squote && !in_dquote => {
                    // Comment starts here; ignore rest
                    let _ = i; // suppress unused
                    break;
                }
                _ => out.push(ch),
            }
        }
        out
    }

    fn tokenize(line: &str) -> Vec<String> {
        // Split by whitespace, respecting quotes (single/double).
        let mut tokens = Vec::new();
        let mut cur = String::new();
        let mut in_squote = false;
        let mut in_dquote = false;
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\'' if !in_dquote => {
                    in_squote = !in_squote;
                }
                '"' if !in_squote => {
                    in_dquote = !in_dquote;
                }
                c if c.is_whitespace() && !in_squote && !in_dquote => {
                    if !cur.is_empty() {
                        tokens.push(cur.clone());
                        cur.clear();
                    }
                }
                _ => cur.push(ch),
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
        tokens
    }

    fn join_tokens(tokens: &[String]) -> String {
        // Join by single spaces; tokens already de-quoted by tokenizer rules.
        tokens.join(" ")
    }

    fn expand_include_pattern(pattern: &str, parent_dir: Option<&Path>) -> Vec<PathBuf> {
        // Expand tilde, make relative to parent, then glob.
        let expanded = tilde(pattern).to_string();
        let candidate = PathBuf::from(expanded);
        let base_rel = if candidate.is_absolute() {
            candidate
        } else {
            match parent_dir {
                Some(base) => base.join(candidate),
                None => candidate,
            }
        };
        let pat_str = base_rel.to_string_lossy().into_owned();

        let mut paths = Vec::new();
        if let Ok(paths_iter) = glob(&pat_str) {
            for entry in paths_iter.flatten() {
                // Only include files that exist and are readable
                if entry.is_file() {
                    paths.push(entry);
                }
            }
        }

        // If glob matched nothing but a literal path exists, include it.
        if paths.is_empty() && base_rel.is_file() {
            paths.push(base_rel);
        }

        paths
    }

    fn is_glob_pattern(s: &str) -> bool {
        s.contains('*') || s.contains('?') || Regex::new(r"\[[^]]+\]").unwrap().is_match(s)
    }
}
