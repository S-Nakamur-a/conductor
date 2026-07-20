//! `SearchResultTree` — the directory→file→match hierarchy itself: building
//! it from a flat match list, flattening it into visible rows, and the
//! expand/collapse/navigation operations the search-results panel drives.

use std::collections::BTreeMap;

use crate::grep_search::GrepMatch;

use super::helpers::{NestedDir, build_nested_dirs, split_dir_file};
use super::model::{DirNode, SearchTreeRow};

/// Tree-structured search results with expand/collapse state.
#[derive(Default)]
pub struct SearchResultTree {
    /// All matches (the original flat list, kept for reference).
    matches: Vec<GrepMatch>,
    /// Internal tree structure: directory path → files.
    dirs: BTreeMap<String, DirNode>,
    /// Expand/collapse state for directories (key: dir path).
    dir_expanded: BTreeMap<String, bool>,
    /// Expand/collapse state for files (key: file path).
    file_expanded: BTreeMap<String, bool>,
    /// Cached flattened visible rows.
    cached_rows: Option<Vec<SearchTreeRow>>,
}

impl SearchResultTree {
    /// Build the tree from a flat list of grep matches.
    pub fn build(matches: &[GrepMatch]) -> Self {
        let mut dirs: BTreeMap<String, DirNode> = BTreeMap::new();

        for (i, m) in matches.iter().enumerate() {
            let (dir, file) = split_dir_file(&m.file_path);
            let dir_node = dirs.entry(dir.clone()).or_insert_with(|| DirNode {
                files: BTreeMap::new(),
            });
            dir_node.files.entry(file).or_default().push(i);
        }

        // All directories and files start expanded.
        let mut dir_expanded = BTreeMap::new();
        let mut file_expanded = BTreeMap::new();
        for (dir_path, dir_node) in &dirs {
            dir_expanded.insert(dir_path.clone(), true);
            for file_name in dir_node.files.keys() {
                let full_path = if dir_path == "." {
                    file_name.clone()
                } else {
                    format!("{dir_path}/{file_name}")
                };
                file_expanded.insert(full_path, true);
            }
        }

        Self {
            matches: matches.to_vec(),
            dirs,
            dir_expanded,
            file_expanded,
            cached_rows: None,
        }
    }

    /// Return the original matches.
    pub fn matches(&self) -> &[GrepMatch] {
        &self.matches
    }

    /// Total match count.
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Get the flattened visible rows, computing if needed.
    pub fn visible_rows(&mut self) -> &[SearchTreeRow] {
        if self.cached_rows.is_none() {
            self.rebuild_rows();
        }
        self.cached_rows.as_deref().unwrap_or(&[])
    }

    /// Invalidate the cached rows (call after expand/collapse changes).
    fn invalidate_cache(&mut self) {
        self.cached_rows = None;
    }

    /// Rebuild the flattened visible rows from the tree structure.
    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();

        // We need to render a nested directory tree. We split multi-component
        // dir paths into segments and group by prefix.
        let mut tree: BTreeMap<String, Vec<(String, &DirNode)>> = BTreeMap::new();
        for (dir_path, dir_node) in &self.dirs {
            let segments: Vec<&str> = if dir_path == "." {
                vec![]
            } else {
                dir_path.split('/').collect()
            };

            if segments.is_empty() {
                // Root-level files (no directory).
                tree.entry(String::new())
                    .or_default()
                    .push((dir_path.clone(), dir_node));
            } else {
                tree.entry(dir_path.clone())
                    .or_default()
                    .push((dir_path.clone(), dir_node));
            }
        }

        // Collect all unique directory prefixes for proper nesting.
        let dir_paths: Vec<String> = self.dirs.keys().cloned().collect();
        let nested = build_nested_dirs(&dir_paths);

        self.render_nested_dir(&nested, &mut rows, 0);

        self.cached_rows = Some(rows);
    }

    fn render_nested_dir(&self, node: &NestedDir, rows: &mut Vec<SearchTreeRow>, depth: usize) {
        // Sort children: directories first, then files.
        let mut child_names: Vec<&String> = node.children.keys().collect();
        child_names.sort();

        for child_name in child_names {
            let child = &node.children[child_name];
            let child_path = if node.path.is_empty() {
                child_name.clone()
            } else {
                format!("{}/{child_name}", node.path)
            };

            if child.has_subdirs() || child.is_leaf_dir {
                let match_count = self.count_matches_under_dir(&child_path);
                let expanded = *self.dir_expanded.get(&child_path).unwrap_or(&true);

                rows.push(SearchTreeRow::Dir {
                    name: child_name.clone(),
                    depth,
                    expanded,
                    match_count,
                });

                if expanded {
                    // Render files directly in this directory.
                    if child.is_leaf_dir
                        && let Some(dir_node) = self.dirs.get(&child_path)
                    {
                        self.render_files(dir_node, &child_path, rows, depth + 1);
                    }
                    // Render subdirectories.
                    if child.has_subdirs() {
                        self.render_nested_dir(child, rows, depth + 1);
                    }
                }
            }
        }

        // Render files directly in this node (if it's a leaf dir in self.dirs).
        if node.is_leaf_dir {
            // Root node has path "" but root-level files are stored under key ".".
            let lookup_key = if node.path.is_empty() {
                "."
            } else {
                &node.path
            };
            let file_path_key = if node.path.is_empty() {
                ".".to_string()
            } else {
                node.path.clone()
            };
            if let Some(dir_node) = self.dirs.get(lookup_key) {
                self.render_files(dir_node, &file_path_key, rows, depth);
            }
        }
    }

    fn render_files(
        &self,
        dir_node: &DirNode,
        dir_path: &str,
        rows: &mut Vec<SearchTreeRow>,
        depth: usize,
    ) {
        let mut file_names: Vec<&String> = dir_node.files.keys().collect();
        file_names.sort();

        for file_name in file_names {
            let match_indices = &dir_node.files[file_name];
            let full_path = if dir_path == "." {
                file_name.clone()
            } else {
                format!("{dir_path}/{file_name}")
            };
            let expanded = *self.file_expanded.get(&full_path).unwrap_or(&true);

            rows.push(SearchTreeRow::File {
                name: file_name.clone(),
                path: full_path.clone(),
                depth,
                expanded,
                match_count: match_indices.len(),
            });

            if expanded {
                for &idx in match_indices {
                    rows.push(SearchTreeRow::Match {
                        depth: depth + 1,
                        match_index: idx,
                    });
                }
            }
        }
    }

    fn count_matches_under_dir(&self, dir_prefix: &str) -> usize {
        let mut count = 0;
        for (dir_path, dir_node) in &self.dirs {
            if dir_path == dir_prefix || dir_path.starts_with(&format!("{dir_prefix}/")) {
                for indices in dir_node.files.values() {
                    count += indices.len();
                }
            }
        }
        count
    }

    /// Toggle expand/collapse for the row at the given visible index.
    pub fn toggle_expand(&mut self, visible_idx: usize) {
        let rows = self.visible_rows().to_vec();
        if let Some(row) = rows.get(visible_idx) {
            match row {
                SearchTreeRow::Dir {
                    name,
                    depth,
                    expanded,
                    ..
                } => {
                    let path = self.resolve_dir_path(&rows, visible_idx, name, *depth);
                    let exp = self.dir_expanded.entry(path).or_insert(*expanded);
                    *exp = !*exp;
                }
                SearchTreeRow::File { path, .. } => {
                    if let Some(exp) = self.file_expanded.get_mut(path) {
                        *exp = !*exp;
                    }
                }
                SearchTreeRow::Match { .. } => {}
            }
        }
        self.invalidate_cache();
    }

    /// Expand the row at the given visible index.
    pub fn expand(&mut self, visible_idx: usize) {
        let rows = self.visible_rows().to_vec();
        if let Some(row) = rows.get(visible_idx) {
            match row {
                SearchTreeRow::Dir {
                    name,
                    depth,
                    expanded,
                    ..
                } => {
                    if !expanded {
                        let path = self.resolve_dir_path(&rows, visible_idx, name, *depth);
                        self.dir_expanded.insert(path, true);
                        self.invalidate_cache();
                    }
                }
                SearchTreeRow::File { path, expanded, .. } => {
                    if !expanded {
                        if let Some(exp) = self.file_expanded.get_mut(path) {
                            *exp = true;
                        }
                        self.invalidate_cache();
                    }
                }
                SearchTreeRow::Match { .. } => {}
            }
        }
    }

    /// Collapse the row at the given visible index.
    pub fn collapse(&mut self, visible_idx: usize) {
        let rows = self.visible_rows().to_vec();
        if let Some(row) = rows.get(visible_idx) {
            match row {
                SearchTreeRow::Dir {
                    name,
                    depth,
                    expanded,
                    ..
                } => {
                    if *expanded {
                        let path = self.resolve_dir_path(&rows, visible_idx, name, *depth);
                        self.dir_expanded.insert(path, false);
                        self.invalidate_cache();
                    }
                }
                SearchTreeRow::File { path, expanded, .. } => {
                    if *expanded {
                        if let Some(exp) = self.file_expanded.get_mut(path) {
                            *exp = false;
                        }
                        self.invalidate_cache();
                    }
                }
                SearchTreeRow::Match { .. } => {}
            }
        }
    }

    /// Check if the row at the given visible index is collapsed (for smart j/k navigation).
    pub fn is_collapsed(&mut self, visible_idx: usize) -> bool {
        let rows = self.visible_rows().to_vec();
        match rows.get(visible_idx) {
            Some(SearchTreeRow::Dir { expanded, .. }) => !expanded,
            Some(SearchTreeRow::File { expanded, .. }) => !expanded,
            _ => false,
        }
    }

    /// Find the next sibling at the same or lower depth (for skipping collapsed subtrees).
    pub fn next_sibling_index(&mut self, visible_idx: usize) -> Option<usize> {
        let rows = self.visible_rows().to_vec();
        let current_depth = match rows.get(visible_idx) {
            Some(SearchTreeRow::Dir { depth, .. }) => *depth,
            Some(SearchTreeRow::File { depth, .. }) => *depth,
            _ => return None,
        };

        for (offset, row) in rows[visible_idx + 1..].iter().enumerate() {
            let d = match row {
                SearchTreeRow::Dir { depth, .. } => *depth,
                SearchTreeRow::File { depth, .. } => *depth,
                SearchTreeRow::Match { depth, .. } => *depth,
            };
            if d <= current_depth {
                return Some(visible_idx + 1 + offset);
            }
        }
        None
    }

    /// Resolve the full directory path from a visible row's name and depth.
    fn resolve_dir_path(
        &self,
        rows: &[SearchTreeRow],
        idx: usize,
        name: &str,
        depth: usize,
    ) -> String {
        // Walk backwards to find parent dirs and reconstruct the full path.
        let mut segments = vec![name.to_string()];
        let mut target_depth = depth;

        for i in (0..idx).rev() {
            if target_depth == 0 {
                break;
            }
            if let SearchTreeRow::Dir {
                name: parent_name,
                depth: parent_depth,
                ..
            } = &rows[i]
                && *parent_depth == target_depth - 1
            {
                segments.push(parent_name.clone());
                target_depth = *parent_depth;
            }
        }

        segments.reverse();
        segments.join("/")
    }

    /// Get the GrepMatch for a Match row at the given visible index.
    pub fn get_match_at(&mut self, visible_idx: usize) -> Option<&GrepMatch> {
        let rows = self.visible_rows().to_vec();
        match rows.get(visible_idx) {
            Some(SearchTreeRow::Match { match_index, .. }) => self.matches.get(*match_index),
            _ => None,
        }
    }
}
