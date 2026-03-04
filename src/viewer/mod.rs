//! Viewer state — file tree model and file content buffer.
//!
//! Manages the state for the Viewer mode: a hierarchical file tree built from
//! the filesystem (skipping `.git` directories) and the content of the
//! currently selected file.

mod file_tree;
mod file_view;

pub use file_tree::{FileTreeEntry, ScoredFile};
pub use file_view::UnifiedDiffEntry;

use std::fs;
use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::diff_state::FileDiff;
use crate::text_input::TextInput;

/// All state owned by the Viewer mode.
pub struct ViewerState {
    /// Flattened file tree (directories + files, pre-order).
    pub file_tree: Vec<FileTreeEntry>,
    /// Index of the selected row in the *full* (unfiltered) tree.
    pub tree_selected: usize,
    /// Vertical scroll offset for the tree pane.
    pub tree_scroll: usize,
    /// Lines of the currently open file.
    pub file_content: Vec<String>,
    /// Vertical scroll offset in the file-content pane.
    pub file_scroll: usize,
    /// Horizontal scroll offset (in characters) for the file-content pane.
    pub h_scroll: usize,
    /// Relative path of the file currently displayed (if any).
    pub current_file: Option<String>,
    /// Current search query (empty = no active search).
    pub search_query: TextInput,
    /// Line indices that match the current search query.
    pub search_matches: Vec<usize>,
    /// Index into search_matches for the current match.
    pub search_match_idx: usize,
    /// Whether the search input box is visible.
    pub search_active: bool,
    /// Start of the selected line range (1-indexed), or `None` if no selection.
    pub selected_line_start: Option<usize>,
    /// End of the selected line range (1-indexed), or `None` for a single-line
    /// selection. Always >= `selected_line_start` when set.
    pub selected_line_end: Option<usize>,
    /// Timestamp (ms) of the last line-number click for double-click detection.
    pub last_line_click_time: std::time::Instant,
    /// The 1-indexed line number that was last clicked on.
    pub last_line_click_line: usize,
    /// Index of the selected diff file in the diff list.
    pub diff_list_selected: usize,
    /// Vertical scroll offset for the diff list.
    pub diff_list_scroll: usize,
    /// Whether the explorer panel focus is on the diff list (bottom half).
    pub explorer_focus_on_diff_list: bool,
    /// Cached syntax-highlighted tokens per line (syntect output converted to ratatui styles).
    pub highlighted_lines: Vec<Vec<(ratatui::style::Style, String)>>,
    /// Last known inner height of the explorer file-tree pane (updated during render).
    pub explorer_tree_height: usize,
    /// Last known inner height of the explorer diff-list pane (updated during render).
    pub explorer_diff_list_height: usize,
    /// Whether the explorer bottom pane shows comments instead of the diff list.
    pub explorer_show_comments: bool,
    /// Index of the selected comment in the explorer comment list.
    pub comment_list_selected: usize,
    /// Vertical scroll offset for the explorer comment list.
    pub comment_list_scroll: usize,
    /// Line number (1-indexed) for comment preview triggered by single-clicking a comment marker.
    pub comment_preview_line: Option<usize>,
    /// Timestamp of the last file-tree click for double-click detection.
    pub last_tree_click_time: std::time::Instant,
    /// The tree index that was last clicked in the file tree.
    pub last_tree_click_idx: usize,
    /// Timestamp of the last comment-list click for double-click detection.
    pub last_comment_click_time: std::time::Instant,
    /// The index that was last clicked in the comment list.
    pub last_comment_click_idx: usize,
    /// Whether the viewer is in unified diff mode.
    pub diff_mode: bool,
    /// Unified diff view entries (populated when entering diff mode).
    pub diff_view_lines: Vec<UnifiedDiffEntry>,
    /// Vertical scroll offset for the diff view.
    pub diff_view_scroll: usize,
    /// Whether the filename search overlay is active.
    pub filename_search_active: bool,
    /// Current filename search query.
    pub filename_search_query: TextInput,
    /// Scored and sorted fuzzy search results.
    pub filename_search_results: Vec<ScoredFile>,
    /// Selected index within the search results list.
    pub filename_search_selected: usize,
    /// Cached list of all file paths for filename search (populated on search start).
    pub filename_search_all_files: Vec<String>,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self {
            file_tree: Vec::new(),
            tree_selected: 0,
            tree_scroll: 0,
            file_content: Vec::new(),
            file_scroll: 0,
            h_scroll: 0,
            current_file: None,
            search_query: TextInput::new(),
            search_matches: Vec::new(),
            search_match_idx: 0,
            search_active: false,
            selected_line_start: None,
            selected_line_end: None,
            last_line_click_time: std::time::Instant::now(),
            last_line_click_line: 0,
            diff_list_selected: 0,
            diff_list_scroll: 0,
            explorer_focus_on_diff_list: false,
            highlighted_lines: Vec::new(),
            explorer_tree_height: 20,
            explorer_diff_list_height: 20,
            explorer_show_comments: false,
            comment_list_selected: 0,
            comment_list_scroll: 0,
            comment_preview_line: None,
            last_tree_click_time: std::time::Instant::now(),
            last_tree_click_idx: usize::MAX,
            last_comment_click_time: std::time::Instant::now(),
            last_comment_click_idx: usize::MAX,
            diff_mode: false,
            diff_view_lines: Vec::new(),
            diff_view_scroll: 0,
            filename_search_active: false,
            filename_search_query: TextInput::new(),
            filename_search_results: Vec::new(),
            filename_search_selected: 0,
            filename_search_all_files: Vec::new(),
        }
    }
}

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
    pub fn load_file_tree(&mut self, worktree_path: &Path) {
        // Save state before clearing.
        let prev_file = self.current_file.clone();
        let prev_file_scroll = self.file_scroll;
        let prev_h_scroll = self.h_scroll;
        let expanded_dirs: Vec<String> = self
            .file_tree
            .iter()
            .filter(|e| e.is_dir && e.is_expanded)
            .map(|e| e.path.clone())
            .collect();

        // Rebuild the tree from disk.
        self.file_tree.clear();
        Self::walk_dir(worktree_path, worktree_path, 0, &mut self.file_tree);

        // Restore directory expansion state.
        for entry in &mut self.file_tree {
            if entry.is_dir && expanded_dirs.contains(&entry.path) {
                entry.is_expanded = true;
            }
        }

        // Re-open the previously viewed file if it still exists.
        if let Some(ref rel_path) = prev_file {
            let full = worktree_path.join(rel_path);
            if full.is_file() {
                // Preserve diff mode state across tree refreshes so that
                // file-watcher / periodic refreshes don't kick the user
                // out of the unified diff view.
                let was_diff_mode = self.diff_mode;
                let prev_diff_lines = if was_diff_mode {
                    std::mem::take(&mut self.diff_view_lines)
                } else {
                    Vec::new()
                };
                let prev_diff_scroll = self.diff_view_scroll;

                self.open_file(worktree_path, rel_path);
                self.file_scroll = prev_file_scroll;
                self.h_scroll = prev_h_scroll;

                if was_diff_mode {
                    self.diff_mode = true;
                    self.diff_view_lines = prev_diff_lines;
                    self.diff_view_scroll = prev_diff_scroll;
                }

                // Try to restore tree_selected to point at the file entry.
                if let Some(idx) = self.file_tree.iter().position(|e| e.path == *rel_path) {
                    self.tree_selected = idx;
                }
            }
            // If the file was deleted, we naturally stay at "no file selected".
        }
    }

    /// Open (read) a file and store its lines in `file_content`.
    pub fn open_file(&mut self, worktree_path: &Path, relative_path: &str) {
        self.exit_diff_mode();
        self.highlighted_lines.clear();
        let full = worktree_path.join(relative_path);
        match fs::read_to_string(&full) {
            Ok(text) => {
                self.file_content = text.lines().map(|l| Self::expand_tabs(l, 4)).collect();
                // If file is empty but not zero-length, show one empty line.
                if self.file_content.is_empty() && !text.is_empty() {
                    self.file_content.push(String::new());
                }
                self.current_file = Some(relative_path.to_string());
                self.file_scroll = 0;
                self.h_scroll = 0;
            }
            Err(e) => {
                // Show error as file content so the user sees feedback.
                self.file_content = vec![format!("Error reading file: {e}")];
                self.current_file = Some(relative_path.to_string());
                self.file_scroll = 0;
                self.h_scroll = 0;
            }
        }
    }

    /// Toggle expand / collapse of the directory at index `idx` in
    /// `file_tree`.
    pub fn toggle_dir(&mut self, idx: usize) {
        if let Some(entry) = self.file_tree.get_mut(idx) {
            if entry.is_dir {
                entry.is_expanded = !entry.is_expanded;
            }
        }
    }

    /// Expand the directory at index `idx` (no-op if already expanded or if
    /// the entry is a file).
    pub fn expand_dir(&mut self, idx: usize) {
        if let Some(entry) = self.file_tree.get_mut(idx) {
            if entry.is_dir {
                entry.is_expanded = true;
            }
        }
    }

    /// Collapse the directory at index `idx` (no-op if already collapsed or if
    /// the entry is a file).
    pub fn collapse_dir(&mut self, idx: usize) {
        if let Some(entry) = self.file_tree.get_mut(idx) {
            if entry.is_dir {
                entry.is_expanded = false;
            }
        }
    }

    /// Return indices into `file_tree` that are currently visible, taking
    /// collapsed directories into account.
    pub fn visible_indices(&self) -> Vec<usize> {
        let mut result = Vec::new();
        let mut skip_depth: Option<usize> = None;

        for (i, entry) in self.file_tree.iter().enumerate() {
            // If we are skipping children of a collapsed dir, check depth.
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

        result
    }

    /// Execute a search over the file content and populate search_matches.
    pub fn execute_search(&mut self) {
        self.search_matches.clear();
        self.search_match_idx = 0;

        if self.search_query.is_empty() {
            return;
        }

        let query_lower = self.search_query.to_lowercase();
        for (i, line) in self.file_content.iter().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                self.search_matches.push(i);
            }
        }

        // Jump to first match at or after current scroll.
        if !self.search_matches.is_empty() {
            self.search_match_idx = self
                .search_matches
                .iter()
                .position(|&line| line >= self.file_scroll)
                .unwrap_or(0);
            self.file_scroll = self.search_matches[self.search_match_idx];
        }
    }

    /// Jump to the next search match.
    pub fn next_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_match_idx = (self.search_match_idx + 1) % self.search_matches.len();
        self.file_scroll = self.search_matches[self.search_match_idx];
    }

    /// Jump to the previous search match.
    pub fn prev_search_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_match_idx = if self.search_match_idx == 0 {
            self.search_matches.len() - 1
        } else {
            self.search_match_idx - 1
        };
        self.file_scroll = self.search_matches[self.search_match_idx];
    }

    // -- Filename fuzzy search ------------------------------------------------

    /// Run fuzzy filename search over the cached file list and populate results.
    pub fn execute_filename_search(&mut self) {
        self.filename_search_results.clear();

        let query = self.filename_search_query.to_lowercase();

        for path in &self.filename_search_all_files {
            let path_lower = path.to_lowercase();
            let name_lower = path.rsplit('/').next().unwrap_or(path).to_lowercase();

            // If query is empty, include all files with score 0.
            if query.is_empty() {
                self.filename_search_results.push(ScoredFile {
                    path: path.clone(),
                    score: 0,
                });
                continue;
            }

            // Check fuzzy subsequence match first — skip non-matching files.
            if !Self::is_fuzzy_match(&query, &path_lower) {
                continue;
            }

            let mut score: i32 = 10; // Base score for fuzzy match.

            // Bonus: consecutive character matches.
            score += Self::consecutive_bonus(&query, &path_lower);

            // Bonus: filename exact prefix.
            if name_lower.starts_with(&query) {
                score += 100;
            }

            // Bonus: path substring match.
            if path_lower.contains(&query) {
                score += 50;
            }

            // Bonus: filename substring match.
            if name_lower.contains(&query) {
                score += 30;
            }

            // Bonus: word boundary match (char after '/', '_', '-', '.').
            if Self::has_word_boundary_match(&query, &path_lower) {
                score += 20;
            }

            self.filename_search_results.push(ScoredFile {
                path: path.clone(),
                score,
            });
        }

        // Sort by score descending, then path ascending for stability.
        self.filename_search_results.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path))
        });
    }

    /// Check if all characters of `query` appear in `haystack` in order.
    fn is_fuzzy_match(query: &str, haystack: &str) -> bool {
        let mut haystack_chars = haystack.chars();
        for qc in query.chars() {
            if !haystack_chars.any(|hc| hc == qc) {
                return false;
            }
        }
        true
    }

    /// Award bonus points for consecutive matching characters.
    fn consecutive_bonus(query: &str, haystack: &str) -> i32 {
        let mut bonus = 0i32;
        let mut consecutive = 0;
        let mut hay_iter = haystack.chars().peekable();

        for qc in query.chars() {
            let mut found = false;
            for hc in hay_iter.by_ref() {
                if hc == qc {
                    consecutive += 1;
                    if consecutive > 1 {
                        bonus += consecutive;
                    }
                    found = true;
                    break;
                }
                consecutive = 0;
            }
            if !found {
                break;
            }
        }
        bonus
    }

    /// Check if query characters match at word boundaries in the haystack
    /// (after '/', '_', '-', '.', or at position 0).
    fn has_word_boundary_match(query: &str, haystack: &str) -> bool {
        let boundary_chars: Vec<char> = haystack
            .char_indices()
            .filter(|&(i, _)| {
                if i == 0 {
                    return true;
                }
                let prev = haystack.as_bytes().get(i - 1).copied().unwrap_or(0);
                matches!(prev, b'/' | b'_' | b'-' | b'.')
            })
            .map(|(_, c)| c)
            .collect();

        if boundary_chars.len() < query.len() {
            return false;
        }

        let mut bi = 0;
        for qc in query.chars() {
            let mut found = false;
            while bi < boundary_chars.len() {
                if boundary_chars[bi] == qc {
                    bi += 1;
                    found = true;
                    break;
                }
                bi += 1;
            }
            if !found {
                return false;
            }
        }
        true
    }

    /// Run syntect highlighting on `file_content` and cache the result.
    pub fn highlight_content(&mut self, syntax_set: &SyntaxSet, theme: &SyntectTheme) {
        self.highlighted_lines.clear();

        if self.file_content.is_empty() {
            return;
        }

        // Determine syntax from file extension.
        let ext = self
            .current_file
            .as_ref()
            .and_then(|p| Path::new(p).extension())
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let syntax = syntax_set
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, theme);

        // Reconstruct the full text with newlines for syntect (it expects them).
        let full_text: String = self
            .file_content
            .iter()
            .map(|l| format!("{l}\n"))
            .collect();

        for line in LinesWithEndings::from(&full_text) {
            let ranges = match h.highlight_line(line, syntax_set) {
                Ok(r) => r,
                Err(_) => {
                    // Fallback: plain white.
                    self.highlighted_lines.push(vec![(
                        ratatui::style::Style::default().fg(ratatui::style::Color::White),
                        line.trim_end_matches('\n').to_string(),
                    )]);
                    continue;
                }
            };

            let spans: Vec<(ratatui::style::Style, String)> = ranges
                .into_iter()
                .map(|(style, text)| {
                    let ratatui_style = syntect_tui::translate_style(style)
                        .unwrap_or_default()
                        .bg(ratatui::style::Color::Reset);
                    // Strip trailing newline from the last token.
                    let text = text.trim_end_matches('\n').to_string();
                    (ratatui_style, text)
                })
                .collect();

            self.highlighted_lines.push(spans);
        }
    }

    // -- Line selection helpers -----------------------------------------------

    /// Clear the current line selection.
    pub fn clear_selection(&mut self) {
        self.selected_line_start = None;
        self.selected_line_end = None;
    }

    /// Return the selected range as `(start, end)` (both 1-indexed, inclusive).
    /// Returns `None` if no line is selected.
    pub fn selected_range(&self) -> Option<(usize, usize)> {
        self.selected_line_start.map(|start| {
            let end = self.selected_line_end.unwrap_or(start);
            if start <= end {
                (start, end)
            } else {
                (end, start)
            }
        })
    }

    /// Check whether a 1-indexed line number falls within the current
    /// selection range.
    pub fn is_line_selected(&self, line_1indexed: usize) -> bool {
        if let Some((start, end)) = self.selected_range() {
            line_1indexed >= start && line_1indexed <= end
        } else {
            false
        }
    }

    /// Handle a click on a line number.  Returns `true` if a double-click
    /// was detected (the caller should open the comment input).
    pub fn click_line_number(&mut self, line_1indexed: usize) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_line_click_time);
        let is_double = elapsed.as_millis() < 400
            && self.last_line_click_line == line_1indexed;

        self.last_line_click_time = now;
        self.last_line_click_line = line_1indexed;

        if is_double {
            // Double-click -> select single line and signal comment creation.
            self.selected_line_start = Some(line_1indexed);
            self.selected_line_end = None;
            return true;
        }

        // Single click logic:
        if self.selected_line_start.is_none() {
            // First click — select start line.
            self.selected_line_start = Some(line_1indexed);
            self.selected_line_end = None;
        } else if self.selected_line_end.is_none() {
            // Second click — set end line (range).
            self.selected_line_end = Some(line_1indexed);
        } else {
            // Third click — start a new selection.
            self.selected_line_start = Some(line_1indexed);
            self.selected_line_end = None;
        }

        false
    }

    // -- Unified diff view ----------------------------------------------------

    /// Build the unified diff view entries from a `FileDiff`.
    pub fn build_unified_diff_view(&mut self, file_diff: &FileDiff) {
        self.diff_view_lines.clear();

        for (hunk_idx, hunk) in file_diff.hunks.iter().enumerate() {
            // Add hunk separator between hunks (not before the first one).
            if hunk_idx > 0 {
                self.diff_view_lines.push(UnifiedDiffEntry::HunkSeparator {
                    func_header: hunk.func_header.clone(),
                });
            }

            for line in &hunk.lines {
                self.diff_view_lines.push(UnifiedDiffEntry::Line {
                    tag: line.tag,
                    new_line_no: line.new_line_no,
                    content: line.content.clone(),
                    inline_segments: line.inline_segments.clone(),
                });
            }
        }

        if !self.diff_view_lines.is_empty() {
            self.diff_mode = true;
            self.diff_view_scroll = 0;
        }
    }

    /// Return the maximum line width (in characters) of the current content.
    ///
    /// In diff mode this scans `diff_view_lines`; otherwise it scans
    /// `file_content`. Returns 0 when there is nothing to display.
    pub fn max_content_width(&self) -> usize {
        if self.diff_mode {
            self.diff_view_lines
                .iter()
                .map(|entry| match entry {
                    UnifiedDiffEntry::Line { content, .. } => content.chars().count(),
                    UnifiedDiffEntry::HunkSeparator { func_header } => {
                        func_header.as_ref().map_or(0, |h| h.chars().count())
                    }
                })
                .max()
                .unwrap_or(0)
        } else {
            self.file_content
                .iter()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
        }
    }

    /// Increase `h_scroll` by `delta`, clamping so the view never scrolls
    /// past the longest line in the current content.
    pub fn scroll_right(&mut self, delta: usize) {
        let max_w = self.max_content_width();
        // Allow scrolling until only a few characters remain visible.
        let limit = max_w.saturating_sub(4);
        self.h_scroll = (self.h_scroll + delta).min(limit);
    }

    /// Exit unified diff mode and reset related state.
    pub fn exit_diff_mode(&mut self) {
        self.diff_mode = false;
        self.diff_view_lines.clear();
        self.diff_view_scroll = 0;
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
            let Some(idx) = self.file_tree.iter().position(|e| e.path == target_path) else {
                return; // Entry not found — cannot reveal.
            };

            if is_last {
                // Select the target file/dir.
                self.tree_selected = idx;
                // Adjust scroll so the item is visible.
                let visible = self.visible_indices();
                if let Some(vis_pos) = visible.iter().position(|&vi| vi == idx) {
                    let height = self.explorer_tree_height;
                    if vis_pos < self.tree_scroll || vis_pos >= self.tree_scroll + height {
                        self.tree_scroll = vis_pos.saturating_sub(height / 3);
                    }
                }
            } else {
                // Intermediate directory — ensure children are loaded and expand.
                self.ensure_children_loaded(idx, worktree_root);
                if let Some(entry) = self.file_tree.get_mut(idx) {
                    entry.is_expanded = true;
                }
            }

            parent_path = target_path;
        }
    }

    // -- Internal helpers -----------------------------------------------------

    /// Expand tab characters to spaces, respecting tab stop positions.
    fn expand_tabs(line: &str, tab_width: usize) -> String {
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
        let entry = match self.file_tree.get(idx) {
            Some(e) if e.is_dir && !e.children_loaded => e,
            _ => return,
        };

        let full_path = worktree_root.join(&entry.path);
        let child_depth = entry.depth + 1;

        let mut children: Vec<FileTreeEntry> = Vec::new();
        Self::read_dir_entries(worktree_root, &full_path, child_depth, &mut children);

        self.file_tree[idx].children_loaded = true;

        if children.is_empty() {
            return;
        }

        let insert_pos = idx + 1;
        let count = children.len();

        // Adjust tree_selected if it's at or after the insertion point.
        if self.tree_selected >= insert_pos {
            self.tree_selected += count;
        }

        self.file_tree.splice(insert_pos..insert_pos, children);
    }

    /// Read the immediate children of `dir` and append them to `entries`.
    /// Does not recurse — children directories will have
    /// `children_loaded: false`.
    fn read_dir_entries(
        root: &Path,
        dir: &Path,
        depth: usize,
        entries: &mut Vec<FileTreeEntry>,
    ) {
        if depth > Self::MAX_DEPTH {
            return;
        }

        let Ok(read_dir) = fs::read_dir(dir) else {
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

            entries.push(FileTreeEntry {
                path: rel_path,
                name,
                depth,
                is_dir,
                is_expanded: false,
                children_loaded: false,
            });
        }
    }

    /// Populate the filename search cache by walking the entire filesystem
    /// tree under the given worktree root.
    pub fn populate_filename_search_cache(&mut self, worktree_root: &Path) {
        self.filename_search_all_files.clear();
        Self::collect_all_file_paths(worktree_root, worktree_root, 0, &mut self.filename_search_all_files);
    }

    /// Recursively collect all file paths under `dir`, skipping the same
    /// directories as `walk_dir` / `SKIP_DIRS`.  Only file paths (not
    /// directories) are appended to `paths`, stored as relative paths from
    /// `root`.
    fn collect_all_file_paths(root: &Path, dir: &Path, depth: usize, paths: &mut Vec<String>) {
        if depth > Self::MAX_DEPTH {
            return;
        }
        let Ok(read_dir) = fs::read_dir(dir) else {
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

    /// Walk `dir` and append entries to `entries`. Only recurses into
    /// directories that are auto-expanded (depth 0). Deeper directories
    /// will have `children_loaded: false` and their contents are loaded
    /// lazily when the user expands them.
    fn walk_dir(
        root: &Path,
        dir: &Path,
        depth: usize,
        entries: &mut Vec<FileTreeEntry>,
    ) {
        if depth > Self::MAX_DEPTH {
            return;
        }

        let Ok(read_dir) = fs::read_dir(dir) else {
            return;
        };

        // Collect and sort: directories first, then files, alphabetically.
        let mut children: Vec<_> = read_dir
            .filter_map(|e| e.ok())
            .collect();

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

            let auto_expand = depth == 0;

            entries.push(FileTreeEntry {
                path: rel_path,
                name,
                depth,
                is_dir,
                is_expanded: auto_expand,
                children_loaded: false,
            });

            if is_dir && auto_expand {
                let entry_idx = entries.len() - 1;
                // Recurse into auto-expanded directories.
                Self::walk_dir(root, &child_path, depth + 1, entries);
                entries[entry_idx].children_loaded = true;
            }
        }
    }
}
