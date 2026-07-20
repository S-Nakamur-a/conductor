//! Building and navigating `DiffState`'s flattened explorer display list: the
//! merged committed/uncommitted directory tree, section collapse/expand, and
//! resolving between display-list indices and file references.

use std::collections::{BTreeMap, HashSet};

use super::model::{DiffListEntry, DiffSection, DiffState, DiffViewMode, FileDiff};

impl DiffState {
    /// Create a new, empty `DiffState`.
    pub fn new(base_branch: &str, view_mode: DiffViewMode) -> Self {
        let mut state = Self {
            committed_files: Vec::new(),
            uncommitted_files: Vec::new(),
            display_list: Vec::new(),
            collapsed_dirs: HashSet::new(),
            scroll: 0,
            view_mode,
            base_branch: base_branch.to_string(),
            error: None,
            has_summary: false,
        };
        state.rebuild_display_list();
        state
    }

    /// Rebuild the flattened display list, merging committed and uncommitted
    /// changes into a single directory tree. Files keep their origin
    /// (`DiffSection`) for the C/U marker and resolution; directories are merged
    /// across origins so `src/` appears once even with both kinds of change.
    pub fn rebuild_display_list(&mut self) {
        self.display_list.clear();

        // Change-summary pseudo-file, pinned at the very top when present.
        if self.has_summary {
            self.display_list.push(DiffListEntry::Summary {});
        }

        Self::build_tree_entries(
            &self.committed_files,
            &self.uncommitted_files,
            &self.collapsed_dirs,
            &mut self.display_list,
        );
    }

    /// Build one directory tree over both origins' files. A file that changed
    /// in both committed and uncommitted form appears twice (once per origin)
    /// under the same merged directory node, distinguished by its marker.
    fn build_tree_entries(
        committed: &[FileDiff],
        uncommitted: &[FileDiff],
        collapsed_dirs: &HashSet<String>,
        display_list: &mut Vec<DiffListEntry>,
    ) {
        // A leaf of the merged tree: which origin list + index it resolves to.
        struct Leaf {
            section: DiffSection,
            index: usize,
            path: String,
        }
        let mut leaves: Vec<Leaf> = Vec::with_capacity(committed.len() + uncommitted.len());
        for (i, f) in committed.iter().enumerate() {
            leaves.push(Leaf {
                section: DiffSection::Committed,
                index: i,
                path: f.path.clone(),
            });
        }
        for (i, f) in uncommitted.iter().enumerate() {
            leaves.push(Leaf {
                section: DiffSection::Uncommitted,
                index: i,
                path: f.path.clone(),
            });
        }
        // Group siblings together; stable so committed precedes uncommitted at
        // the same path.
        leaves.sort_by(|a, b| a.path.cmp(&b.path));

        // Collect directory paths and the leaf indices living directly in each.
        let mut dir_set: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut top_level: Vec<usize> = Vec::new();
        for (li, leaf) in leaves.iter().enumerate() {
            if let Some(slash) = leaf.path.rfind('/') {
                dir_set
                    .entry(leaf.path[..slash].to_string())
                    .or_default()
                    .push(li);
            } else {
                top_level.push(li);
            }
        }

        // Ensure all ancestor directories exist as nodes.
        let all_dirs: Vec<String> = dir_set.keys().cloned().collect();
        for dir in &all_dirs {
            let mut current = dir.as_str();
            while let Some(slash) = current.rfind('/') {
                let parent = &current[..slash];
                dir_set.entry(parent.to_string()).or_default();
                current = parent;
            }
        }

        struct TreeNode {
            child_dirs: Vec<String>,
            leaves: Vec<usize>,
        }
        let mut nodes: BTreeMap<String, TreeNode> = BTreeMap::new();
        for dir_path in dir_set.keys() {
            nodes.entry(dir_path.clone()).or_insert_with(|| TreeNode {
                child_dirs: Vec::new(),
                leaves: Vec::new(),
            });
        }
        for (dir_path, leaf_indices) in &dir_set {
            if let Some(node) = nodes.get_mut(dir_path) {
                node.leaves = leaf_indices.clone();
            }
        }
        let dir_paths: Vec<String> = nodes.keys().cloned().collect();
        let mut root_dirs: Vec<String> = Vec::new();
        for dir_path in &dir_paths {
            if let Some(slash) = dir_path.rfind('/') {
                let parent = &dir_path[..slash];
                if let Some(parent_node) = nodes.get_mut(parent) {
                    parent_node.child_dirs.push(dir_path.clone());
                } else {
                    root_dirs.push(dir_path.clone());
                }
            } else {
                root_dirs.push(dir_path.clone());
            }
        }
        root_dirs.sort();
        for node in nodes.values_mut() {
            node.child_dirs.sort();
            // `leaves` already in path/origin order from the global sort.
        }

        fn emit_dir(
            dir_path: &str,
            depth: usize,
            leaves: &[Leaf],
            nodes: &BTreeMap<String, TreeNode>,
            collapsed_dirs: &HashSet<String>,
            display_list: &mut Vec<DiffListEntry>,
        ) {
            let name = dir_path.rsplit('/').next().unwrap_or(dir_path).to_string();
            let is_collapsed = collapsed_dirs.contains(dir_path);
            display_list.push(DiffListEntry::Directory {
                path: dir_path.to_string(),
                name,
                depth,
                collapsed: is_collapsed,
            });
            if is_collapsed {
                return;
            }
            if let Some(node) = nodes.get(dir_path) {
                for child_dir in &node.child_dirs {
                    emit_dir(child_dir, depth + 1, leaves, nodes, collapsed_dirs, display_list);
                }
                for &li in &node.leaves {
                    display_list.push(DiffListEntry::File {
                        section: leaves[li].section,
                        file_index: leaves[li].index,
                        depth: depth + 1,
                    });
                }
            }
        }

        for dir_path in &root_dirs {
            emit_dir(dir_path, 0, &leaves, &nodes, collapsed_dirs, display_list);
        }
        for &li in &top_level {
            display_list.push(DiffListEntry::File {
                section: leaves[li].section,
                file_index: leaves[li].index,
                depth: 0,
            });
        }
    }

    /// Resolve a display list index to a file reference and its section.
    ///
    /// Returns `None` for section headers or out-of-range indices.
    pub fn resolve_file(&self, display_idx: usize) -> Option<(&FileDiff, DiffSection)> {
        match self.display_list.get(display_idx)? {
            DiffListEntry::File {
                section,
                file_index,
                ..
            } => {
                let files = match section {
                    DiffSection::Committed => &self.committed_files,
                    DiffSection::Uncommitted => &self.uncommitted_files,
                };
                files.get(*file_index).map(|f| (f, *section))
            }
            DiffListEntry::Directory { .. } | DiffListEntry::Summary {} => None,
        }
    }

    /// Find the display list index for a file by its repo-relative path
    /// (the reverse of [`Self::resolve_file`]). Used to keep the diff list's
    /// cursor in sync when a file is opened by path rather than by list
    /// index (e.g. jumping to a walkthrough step's file).
    pub fn display_index_for_path(&self, path: &str) -> Option<usize> {
        (0..self.display_list.len()).find(|&idx| self.resolve_file(idx).is_some_and(|(f, _)| f.path == path))
    }

    /// Toggle the collapsed state of the directory at the given display index.
    /// Returns `true` if a directory was toggled (so the caller knows the list
    /// changed). Non-directory rows (files, the summary) are a no-op.
    pub fn toggle_section(&mut self, display_idx: usize) -> bool {
        if let Some(DiffListEntry::Directory { path, .. }) = self.display_list.get(display_idx) {
            let key = path.clone();
            if self.collapsed_dirs.contains(&key) {
                self.collapsed_dirs.remove(&key);
            } else {
                self.collapsed_dirs.insert(key);
            }
            self.rebuild_display_list();
            true
        } else {
            false
        }
    }

    /// Collapse the directory at the given display index (no-op for other rows).
    pub fn collapse_section(&mut self, display_idx: usize) {
        if let Some(DiffListEntry::Directory { path, collapsed, .. }) =
            self.display_list.get(display_idx)
            && !collapsed
        {
            let key = path.clone();
            self.collapsed_dirs.insert(key);
            self.rebuild_display_list();
        }
    }

    /// Expand the directory at the given display index (no-op for other rows).
    pub fn expand_section(&mut self, display_idx: usize) {
        if let Some(DiffListEntry::Directory { path, collapsed, .. }) =
            self.display_list.get(display_idx)
            && *collapsed
        {
            let key = path.clone();
            self.collapsed_dirs.remove(&key);
            self.rebuild_display_list();
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────

    /// Expand tab characters to spaces, matching the viewer's tab expansion.
    pub fn expand_tabs(line: &str, tab_width: usize) -> String {
        if !line.contains('\t') {
            return line.to_string();
        }
        let mut result = String::with_capacity(line.len());
        let mut col = 0;
        for ch in line.chars() {
            if ch == '\t' {
                let spaces = tab_width - (col % tab_width);
                for _ in 0..spaces {
                    result.push(' ');
                }
                col += spaces;
            } else {
                result.push(ch);
                col += 1;
            }
        }
        result
    }
}
