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
        pub matches: Vec<MatchRule>, // parsed Match blocks in this file
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

    /// A parsed Match rule with a set of conditions and parameter overrides.
    #[derive(Clone, Debug)]
    pub struct MatchRule {
        pub conditions: Vec<MatchCond>, // e.g. [Host([...]), User([...]), All]
        pub params: BTreeMap<String, String>, // parameters inside the Match block
        pub source: PathBuf,            // file of this match rule
        pub line: usize,                // starting line of the Match directive
    }

    /// Subset of OpenSSH Match conditions we support.
    #[derive(Clone, Debug)]
    pub enum MatchCond {
        Host(Vec<String>),
        User(Vec<String>),
        All,
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
    // Effective user resolution
    // ----------------------
    /// Resolve the effective User for a given alias by:
    /// - finding the most specific matching Host entry (exact match preferred over globs),
    /// - then applying any matching Match rules (Host/User/All) in a best-effort order.
    pub fn effective_user_for_alias(tree: &ConfigTree, alias: &str) -> Option<String> {
        use crate::model::{FileNode, HostEntry, MatchCond};
        // Flatten nodes depth-first
        fn collect<'a>(n: &'a FileNode, out: &mut Vec<&'a FileNode>) {
            out.push(n);
            for inc in &n.includes {
                collect(inc, out);
            }
        }
        fn glob_match_simple(pat: &str, s: &str) -> bool {
            // Support * and ? only.
            let mut pi = 0usize;
            let bytes_p = pat.as_bytes();
            let bytes_s = s.as_bytes();
            let mut si = 0usize;
            let mut star: Option<(usize, usize)> = None;
            while si < bytes_s.len() {
                if pi < bytes_p.len() {
                    match bytes_p[pi] {
                        b'?' => {
                            pi += 1;
                            si += 1;
                            continue;
                        }
                        b'*' => {
                            star = Some((pi, si));
                            pi += 1;
                            continue;
                        }
                        _ => {
                            if bytes_p[pi] == bytes_s[si] {
                                pi += 1;
                                si += 1;
                                continue;
                            }
                        }
                    }
                }
                if let Some((sp, ss)) = star {
                    pi = sp + 1;
                    si = ss + 1;
                    star = Some((sp, si));
                } else {
                    return false;
                }
            }
            while pi < bytes_p.len() && bytes_p[pi] == b'*' {
                pi += 1;
            }
            pi == bytes_p.len()
        }
        let mut nodes = Vec::new();
        collect(&tree.root, &mut nodes);
        // Pick host entry: exact match preferred; among equals, pick with greatest line.
        let mut best_exact: Option<(&HostEntry, usize)> = None;
        let mut best_glob: Option<(&HostEntry, usize)> = None;
        for n in &nodes {
            for h in &n.hosts {
                if h.patterns.iter().any(|p| p == alias) {
                    if best_exact.map(|(_, l)| h.line > l).unwrap_or(true) {
                        best_exact = Some((h, h.line));
                    }
                } else if h
                    .patterns
                    .iter()
                    .any(|p| is_glob_pattern(p) && glob_match_simple(p, alias))
                {
                    if best_glob.map(|(_, l)| h.line > l).unwrap_or(true) {
                        best_glob = Some((h, h.line));
                    }
                }
            }
        }
        let base = best_exact.or(best_glob).map(|(h, _)| h);
        let mut user = base.and_then(|h| h.get("user")).map(|s| s.to_string());
        // Apply match rules
        for n in &nodes {
            for m in &n.matches {
                let mut ok = true;
                for c in &m.conditions {
                    match c {
                        MatchCond::All => {}
                        MatchCond::Host(pats) => {
                            if !pats.iter().any(|p| {
                                if p == alias {
                                    true
                                } else if is_glob_pattern(p) {
                                    glob_match_simple(p, alias)
                                } else {
                                    false
                                }
                            }) {
                                ok = false;
                                break;
                            }
                        }
                        MatchCond::User(pats) => {
                            if let Some(ref u) = user {
                                if !pats.iter().any(|p| {
                                    if p == u {
                                        true
                                    } else if is_glob_pattern(p) {
                                        glob_match_simple(p, u)
                                    } else {
                                        false
                                    }
                                }) {
                                    ok = false;
                                    break;
                                }
                            } else {
                                ok = false;
                                break;
                            }
                        }
                    }
                }
                if ok {
                    if let Some(v) = m.params.get("user") {
                        user = Some(v.clone());
                    }
                }
            }
        }
        user
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
                    matches: vec![],
                });
            }
        }

        let text = fs::read_to_string(&resolved)
            .with_context(|| format!("reading SSH config {}", resolved.display()))?;
        let mut includes: Vec<FileNode> = Vec::new();
        let mut hosts: Vec<HostEntry> = Vec::new();
        let mut matches: Vec<crate::model::MatchRule> = Vec::new();

        // Current host block being assembled
        #[derive(Default)]
        struct CurrentHost {
            patterns: Vec<String>,
            params: BTreeMap<String, String>,
            start_line: usize,
        }
        #[derive(Default)]
        struct CurrentMatch {
            conditions: Vec<crate::model::MatchCond>,
            params: BTreeMap<String, String>,
            start_line: usize,
        }
        let mut cur: Option<CurrentHost> = None;
        let mut cur_match: Option<CurrentMatch> = None;

        let mut line_no = 0usize;

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

            // Handle Match blocks: parse conditions and collect parameters until next Host/Match.
            if key == "match" {
                // Finalize any previous match block.
                if let Some(prev) = cur_match.take() {
                    matches.push(crate::model::MatchRule {
                        conditions: prev.conditions,
                        params: prev.params,
                        source: canonicalize_best_effort(&resolved)
                            .unwrap_or_else(|| resolved.clone()),
                        line: prev.start_line,
                    });
                }
                // Parse conditions from tokens[1..]
                let mut conds: Vec<crate::model::MatchCond> = Vec::new();
                let mut i = 1usize;
                while i < tokens.len() {
                    let t = tokens[i].to_ascii_lowercase();
                    match t.as_str() {
                        "all" => {
                            conds.push(crate::model::MatchCond::All);
                            i += 1;
                        }
                        "host" => {
                            i += 1;
                            let mut pats = Vec::new();
                            while i < tokens.len() {
                                let peek = tokens[i].to_ascii_lowercase();
                                if peek == "user"
                                    || peek == "host"
                                    || peek == "all"
                                    || peek == "final"
                                {
                                    break;
                                }
                                pats.push(tokens[i].clone());
                                i += 1;
                            }
                            if !pats.is_empty() {
                                conds.push(crate::model::MatchCond::Host(pats));
                            }
                        }
                        "user" => {
                            i += 1;
                            let mut pats = Vec::new();
                            while i < tokens.len() {
                                let peek = tokens[i].to_ascii_lowercase();
                                if peek == "user"
                                    || peek == "host"
                                    || peek == "all"
                                    || peek == "final"
                                {
                                    break;
                                }
                                pats.push(tokens[i].clone());
                                i += 1;
                            }
                            if !pats.is_empty() {
                                conds.push(crate::model::MatchCond::User(pats));
                            }
                        }
                        // ignore unsupported criteria like "final", "exec", etc.
                        _ => {
                            i += 1;
                        }
                    }
                }
                cur_match = Some(CurrentMatch {
                    conditions: conds,
                    params: BTreeMap::new(),
                    start_line: line_no,
                });
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
                    // Finalize any pending match block before starting a new host block.
                    if let Some(prevm) = cur_match.take() {
                        matches.push(crate::model::MatchRule {
                            conditions: prevm.conditions,
                            params: prevm.params,
                            source: canonicalize_best_effort(&resolved)
                                .unwrap_or_else(|| resolved.clone()),
                            line: prevm.start_line,
                        });
                    }
                    // Push previous host
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
                    // Parameter line: populate current Match block if active, otherwise current Host.
                    if let Some(ref mut m) = cur_match {
                        if tokens.len() >= 2 {
                            let value = join_tokens(&tokens[1..]);
                            m.params.insert(key, value);
                        }
                    } else if let Some(ref mut h) = cur {
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

        // Push tail host and any pending match block
        if let Some(prev) = cur.take() {
            hosts.push(HostEntry {
                patterns: prev.patterns,
                params: prev.params,
                source: canonicalize_best_effort(&resolved).unwrap_or_else(|| resolved.clone()),
                line: prev.start_line,
            });
        }
        if let Some(prev) = cur_match.take() {
            matches.push(crate::model::MatchRule {
                conditions: prev.conditions,
                params: prev.params,
                source: canonicalize_best_effort(&resolved).unwrap_or_else(|| resolved.clone()),
                line: prev.start_line,
            });
        }

        Ok(FileNode {
            path: canonicalize_best_effort(&resolved).unwrap_or(resolved),
            hosts,
            includes,
            matches,
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
