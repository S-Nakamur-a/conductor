//! File tree construction and navigation — walking the filesystem into a
//! flattened `Vec<FileTreeEntry>`, lazy-loading directory children, expand /
//! collapse, and revealing a path in the tree.

use std::path::Path;
use std::rc::Rc;

use super::file_tree::{FileTreeEntry, file_icon};
use super::state::ViewerState;

impl ViewerState {
    /// Build the file tree by walking the filesystem under `worktree_path`.
    ///
    /// Directories named `.git` are skipped. The tree is sorted so that
    /// directories come before files at each level, and entries are
    /// alphabetically ordered within each group.
    ///
    /// Preserves the currently open file, scroll position, and directory
    /// expansion state so that file-watcher refreshes don't disrupt the
    /// user's view. If the previously open file was deleted, the viewer
    /// naturally resets to "no file selected".
    ///
    /// Returns `true` when the set of visible entries changed, so callers can
    /// skip a repaint when a periodic refresh found nothing new.
    pub fn load_file_tree(&mut self, worktree_path: &Path, tab_width: usize) -> bool {
        // Save state before clearing.
        let prev_file = self.content.current_file.clone();
        let prev_file_scroll = self.content.file_scroll;
        let prev_h_scroll = self.content.h_scroll;
        let expanded_dirs: Vec<String> = self
            .tree
            .file_tree
            .iter()
            .filter(|e| e.is_dir && e.is_expanded)
            .map(|e| e.path.clone())
            .collect();
        // Remember the cursor's entry and the full path set so we can restore
        // the cursor and detect whether the rebuilt tree actually changed.
        let prev_selected_path = self
            .tree
            .file_tree
            .get(self.tree.tree_selected)
            .map(|e| e.path.clone());
        let prev_paths: Vec<String> =
            self.tree.file_tree.iter().map(|e| e.path.clone()).collect();

        // Rebuild the tree from disk.
        self.tree.file_tree.clear();
        self.invalidate_visible_cache();
        Self::walk_dir(worktree_path, worktree_path, 0, &mut self.tree.file_tree);

        // Restore directory expansion state. For lazily-loaded dirs, also
        // load their children so the tree looks the same as before the refresh.
        let mut idx = 0;
        while idx < self.tree.file_tree.len() {
            if self.tree.file_tree[idx].is_dir
                && expanded_dirs.contains(&self.tree.file_tree[idx].path)
            {
                self.tree.file_tree[idx].is_expanded = true;
                if !self.tree.file_tree[idx].children_loaded {
                    self.ensure_children_loaded(idx, worktree_path);
                }
            }
            idx += 1;
        }

        // Re-open the previously viewed file if it still exists.
        if let Some(ref rel_path) = prev_file {
            let full = worktree_path.join(rel_path);
            if full.is_file() {
                // Preserve the viewer's *mode* across tree refreshes so that
                // file-watcher / periodic refreshes don't kick the user out of
                // the unified diff view or the SUMMARY pseudo-file. `open_file`
                // below goes through `exit_diff_mode`, which clears both, so
                // every mode flag has to be captured here and put back after.
                let was_diff_mode = self.diff_view.diff_mode;
                let prev_diff_lines = if was_diff_mode {
                    std::mem::take(&mut self.diff_view.diff_view_lines)
                } else {
                    Vec::new()
                };
                let prev_diff_scroll = self.diff_view.diff_view_scroll;
                let prev_diff_max_line_no = self.diff_view.diff_view_max_line_no;
                let was_summary = self.show_summary;
                let prev_summary_scroll = self.summary_scroll;
                // `open_file` resets the rendered-markdown scroll (it indexes a
                // specific document), which is right when the *user* opens a
                // file and wrong here — a watcher or the 3s poll would yank a
                // reader back to the top of the prose mid-read.
                let prev_md_scroll = self.md_scroll;

                self.open_file(worktree_path, rel_path, tab_width);
                self.content.file_scroll = prev_file_scroll;
                self.content.h_scroll = prev_h_scroll;
                self.md_scroll = prev_md_scroll;

                if was_diff_mode {
                    self.diff_view.diff_mode = true;
                    self.diff_view.diff_view_lines = prev_diff_lines;
                    self.diff_view.diff_view_scroll = prev_diff_scroll;
                    self.diff_view.diff_view_max_line_no = prev_diff_max_line_no;
                }
                // Mutually exclusive with diff mode (see `enter_summary_view`),
                // so this can never fight the branch above.
                if was_summary {
                    self.show_summary = true;
                    self.summary_scroll = prev_summary_scroll;
                }

                // Try to restore tree_selected to point at the file entry.
                if let Some(idx) = self.tree.file_tree.iter().position(|e| e.path == *rel_path) {
                    self.tree.tree_selected = idx;
                }
            }
            // If the file was deleted, we naturally stay at "no file selected".
        }

        // Keep the tree cursor anchored across rebuilds. When a file is open the
        // block above already pointed the cursor at it; otherwise restore the
        // previously selected entry so watcher/periodic refreshes don't snap the
        // cursor back to the top.
        let anchored_to_open_file = prev_file.as_ref().is_some_and(|f| {
            self.tree.file_tree.get(self.tree.tree_selected).map(|e| &e.path) == Some(f)
        });
        if !anchored_to_open_file
            && let Some(path) = prev_selected_path
            && let Some(idx) = self.tree.file_tree.iter().position(|e| e.path == path)
        {
            self.tree.tree_selected = idx;
        }
        if self.tree.tree_selected >= self.tree.file_tree.len() {
            self.tree.tree_selected = self.tree.file_tree.len().saturating_sub(1);
        }

        self.tree
            .file_tree
            .iter()
            .map(|e| &e.path)
            .ne(prev_paths.iter())
    }

    /// Toggle expand / collapse of the directory at index `idx` in
    /// `file_tree`.
    pub fn toggle_dir(&mut self, idx: usize) {
        if let Some(entry) = self.tree.file_tree.get_mut(idx)
            && entry.is_dir
        {
            entry.is_expanded = !entry.is_expanded;
            self.invalidate_visible_cache();
        }
    }

    /// Expand the directory at index `idx` (no-op if already expanded or if
    /// the entry is a file).
    pub fn expand_dir(&mut self, idx: usize) {
        if let Some(entry) = self.tree.file_tree.get_mut(idx)
            && entry.is_dir
            && !entry.is_expanded
        {
            entry.is_expanded = true;
            self.invalidate_visible_cache();
        }
    }

    /// Collapse the directory at index `idx` (no-op if already collapsed or if
    /// the entry is a file).
    pub fn collapse_dir(&mut self, idx: usize) {
        if let Some(entry) = self.tree.file_tree.get_mut(idx)
            && entry.is_dir
            && entry.is_expanded
        {
            entry.is_expanded = false;
            self.invalidate_visible_cache();
        }
    }

    /// Invalidate the cached visible indices. Must be called whenever the
    /// tree structure changes (expand/collapse, load children, reload tree).
    pub fn invalidate_visible_cache(&mut self) {
        self.tree.cached_visible_indices = None;
    }

    /// Return indices into `file_tree` that are currently visible, taking
    /// collapsed directories into account. Results are cached (as `Rc`) until
    /// `invalidate_visible_cache()` is called, so repeated calls within
    /// the same frame are essentially free.
    pub fn visible_indices(&mut self) -> Rc<Vec<usize>> {
        if let Some(ref cached) = self.tree.cached_visible_indices {
            return Rc::clone(cached);
        }

        let mut result = Vec::with_capacity(self.tree.file_tree.len());
        let mut skip_depth: Option<usize> = None;

        for (i, entry) in self.tree.file_tree.iter().enumerate() {
            if let Some(sd) = skip_depth {
                if entry.depth > sd {
                    continue;
                } else {
                    skip_depth = None;
                }
            }

            result.push(i);

            if entry.is_dir && !entry.is_expanded {
                skip_depth = Some(entry.depth);
            }
        }

        let rc = Rc::new(result);
        self.tree.cached_visible_indices = Some(Rc::clone(&rc));
        rc
    }

    // -- Tree reveal ----------------------------------------------------------

    /// Reveal and select a file in the explorer tree by its relative path.
    ///
    /// Walks the path segments, expanding (and lazy-loading) each parent
    /// directory along the way, then sets `tree_selected` to the target
    /// entry and adjusts scroll so it is visible.
    pub fn reveal_file_in_tree(&mut self, relative_path: &str, worktree_root: &Path) {
        let segments: Vec<&str> = relative_path.split('/').collect();
        if segments.is_empty() {
            return;
        }

        let mut parent_path = String::new();

        for (seg_idx, segment) in segments.iter().enumerate() {
            let is_last = seg_idx == segments.len() - 1;
            let target_path = if parent_path.is_empty() {
                segment.to_string()
            } else {
                format!("{parent_path}/{segment}")
            };

            // Find the entry with matching path.
            let Some(idx) = self
                .tree
                .file_tree
                .iter()
                .position(|e| e.path == target_path)
            else {
                return; // Entry not found — cannot reveal.
            };

            if is_last {
                // Select the target file/dir.
                self.tree.tree_selected = idx;
                // Adjust scroll so the item is visible.
                let visible = self.visible_indices();
                if let Some(vis_pos) = visible.iter().position(|&vi| vi == idx) {
                    let height = self.explorer.explorer_tree_height;
                    if vis_pos < self.tree.tree_scroll || vis_pos >= self.tree.tree_scroll + height
                    {
                        self.tree.tree_scroll = vis_pos.saturating_sub(height / 3);
                    }
                }
            } else {
                // Intermediate directory — ensure children are loaded and expand.
                self.ensure_children_loaded(idx, worktree_root);
                if let Some(entry) = self.tree.file_tree.get_mut(idx)
                    && !entry.is_expanded
                {
                    entry.is_expanded = true;
                    self.invalidate_visible_cache();
                }
            }

            parent_path = target_path;
        }
    }

    // -- Internal helpers -----------------------------------------------------

    /// Maximum recursion depth for the file tree walker.
    const MAX_DEPTH: usize = 8;

    /// Directories that are skipped during the file tree walk because they
    /// tend to contain a very large number of files and are rarely useful to
    /// browse interactively.
    const SKIP_DIRS: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        "vendor",
        ".next",
        "dist",
        "build",
        "__pycache__",
        ".cache",
        "coverage",
        ".venv",
        "venv",
        "bower_components",
        ".tox",
        ".mypy_cache",
        ".pytest_cache",
        ".turbo",
        ".nuxt",
        ".output",
    ];

    /// Lazily load the immediate children of the directory at `idx` in
    /// `file_tree`. No-op if the entry is not a directory or if children are
    /// already loaded.
    pub fn ensure_children_loaded(&mut self, idx: usize, worktree_root: &Path) {
        let entry = match self.tree.file_tree.get(idx) {
            Some(e) if e.is_dir && !e.children_loaded => e,
            _ => return,
        };

        let full_path = worktree_root.join(&entry.path);
        let child_depth = entry.depth + 1;

        let mut children: Vec<FileTreeEntry> = Vec::new();
        Self::read_dir_entries(worktree_root, &full_path, child_depth, &mut children);

        self.tree.file_tree[idx].children_loaded = true;

        if children.is_empty() {
            return;
        }

        let insert_pos = idx + 1;
        let count = children.len();

        // Adjust tree_selected if it's at or after the insertion point.
        if self.tree.tree_selected >= insert_pos {
            self.tree.tree_selected += count;
        }

        self.tree.file_tree.splice(insert_pos..insert_pos, children);
        self.invalidate_visible_cache();
    }

    /// Read the immediate children of `dir` and append them to `entries`.
    /// Does not recurse — children directories will have
    /// `children_loaded: false`.
    fn read_dir_entries(root: &Path, dir: &Path, depth: usize, entries: &mut Vec<FileTreeEntry>) {
        if depth > Self::MAX_DEPTH {
            return;
        }

        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };

        let mut children: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();

        children.sort_by(|a, b| {
            let a_is_dir = a.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            let b_is_dir = b.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().cmp(&b.file_name()),
            }
        });

        for child in &children {
            let name = child.file_name().to_string_lossy().to_string();

            let child_path = child.path();
            let is_dir = child_path.is_dir();

            if is_dir && Self::SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }

            let rel_path = child_path
                .strip_prefix(root)
                .unwrap_or(&child_path)
                .to_string_lossy()
                .to_string();

            let icon = if is_dir {
                "\u{1f4c1}"
            } else {
                file_icon(&name)
            };
            entries.push(FileTreeEntry {
                path: rel_path,
                name,
                depth,
                is_dir,
                is_expanded: false,
                children_loaded: false,
                icon,
            });
        }
    }

    /// Populate the filename search cache by walking the entire filesystem
    /// tree under the given worktree root.
    pub fn populate_filename_search_cache(&mut self, worktree_root: &Path) {
        self.filename_search.filename_search_all_files.clear();
        Self::collect_all_file_paths(
            worktree_root,
            worktree_root,
            0,
            &mut self.filename_search.filename_search_all_files,
        );
    }

    /// Recursively collect all file paths under `dir`, skipping the same
    /// directories as `walk_dir` / `SKIP_DIRS`.  Only file paths (not
    /// directories) are appended to `paths`, stored as relative paths from
    /// `root`.
    fn collect_all_file_paths(root: &Path, dir: &Path, depth: usize, paths: &mut Vec<String>) {
        if depth > Self::MAX_DEPTH {
            return;
        }
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_path = entry.path();
            let is_dir = child_path.is_dir();
            if is_dir && Self::SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let rel_path = child_path
                .strip_prefix(root)
                .unwrap_or(&child_path)
                .to_string_lossy()
                .to_string();
            if is_dir {
                Self::collect_all_file_paths(root, &child_path, depth + 1, paths);
            } else {
                paths.push(rel_path);
            }
        }
    }

    /// Walk `dir` and append its immediate children to `entries`.
    /// All directories start collapsed with `children_loaded: false`;
    /// their contents are loaded lazily when the user expands them.
    pub fn walk_dir(root: &Path, dir: &Path, depth: usize, entries: &mut Vec<FileTreeEntry>) {
        if depth > Self::MAX_DEPTH {
            return;
        }

        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };

        // Collect and sort: directories first, then files, alphabetically.
        let mut children: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();

        children.sort_by(|a, b| {
            let a_is_dir = a.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            let b_is_dir = b.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().cmp(&b.file_name()),
            }
        });

        for child in &children {
            let name = child.file_name().to_string_lossy().to_string();

            let child_path = child.path();
            let is_dir = child_path.is_dir();

            // Skip known heavy directories.
            if is_dir && Self::SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }

            let rel_path = child_path
                .strip_prefix(root)
                .unwrap_or(&child_path)
                .to_string_lossy()
                .to_string();

            let icon = if is_dir {
                "\u{1f4c1}"
            } else {
                file_icon(&name)
            };
            entries.push(FileTreeEntry {
                path: rel_path,
                name,
                depth,
                is_dir,
                is_expanded: false,
                children_loaded: false,
                icon,
            });
        }
    }
}

#[cfg(test)]
#[path = "tree/tests.rs"]
mod tests;
