//! Viewer state — file tree model and file content buffer.
//!
//! Manages the state for the Viewer mode: a hierarchical file tree built from
//! the filesystem (skipping `.git` directories) and the content of the
//! currently selected file.

mod file_tree;
mod file_view;

pub use file_tree::{FileTreeEntry, ScoredFile, file_icon};
pub use file_view::UnifiedDiffEntry;

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;

use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use std::collections::HashSet;

use crate::diff_state::{DiffHunk, DiffLineTag, FileDiff};
use crate::media_state::{self, MediaState};
use crate::text_input::TextInput;

// ── Sub-structs ──────────────────────────────────────────────────────

/// File tree management state.
#[derive(Default)]
pub struct FileTreeState {
    /// Flattened file tree (directories + files, pre-order).
    pub file_tree: Vec<FileTreeEntry>,
    /// Index of the selected row in the *full* (unfiltered) tree.
    pub tree_selected: usize,
    /// Vertical scroll offset for the tree pane.
    pub tree_scroll: usize,
    /// Cached result of `visible_indices()`. Invalidated when tree structure changes.
    pub cached_visible_indices: Option<Rc<Vec<usize>>>,
}

/// File content viewing state.
#[derive(Default)]
pub struct FileContentState {
    /// Lines of the currently open file.
    pub file_content: Vec<String>,
    /// Vertical scroll offset in the file-content pane.
    pub file_scroll: usize,
    /// Horizontal scroll offset (in characters) for the file-content pane.
    pub h_scroll: usize,
    /// Relative path of the file currently displayed (if any).
    pub current_file: Option<String>,
    /// Cached syntax-highlighted tokens per line (syntect output converted to ratatui styles).
    pub highlighted_lines: Vec<Vec<(ratatui::style::Style, String)>>,
    /// Hash of (current_file, file_content) used to skip redundant re-highlighting.
    pub highlighted_cache_key: Option<u64>,
    /// Cached diff annotations for the currently viewed file (line_no -> (tag, segments)).
    /// Invalidated when diff data changes or a different file is opened.
    pub cached_diff_annotations: Option<
        std::collections::HashMap<
            usize,
            (
                crate::diff_state::DiffLineTag,
                Vec<crate::diff_state::InlineSegment>,
            ),
        >,
    >,
    /// The file path for which `cached_diff_annotations` was built.
    pub cached_diff_annotations_file: Option<String>,
    /// Line number (1-indexed) highlighted from grep search result. Cleared on next file open.
    pub grep_highlight_line: Option<usize>,
    /// Screen-row mapping built during render. Used by mouse event handlers
    /// to translate screen positions to file lines / thread actions.
    pub screen_row_map: Vec<ScreenRow>,
    /// Runnable Go tests in the current file, keyed by 1-indexed line number.
    /// Populated (from [`crate::go_test::scan_go_test_runs`]) only for
    /// `*_test.go` files; empty otherwise. Drives the ▶ run buttons.
    pub test_runs: std::collections::HashMap<usize, crate::go_test::TestRun>,
}

/// What a screen row represents (for mouse click handling).
#[derive(Debug, Clone)]
pub enum ScreenRow {
    /// A source code line (1-indexed line number).
    Code(usize),
    /// A thread content row (not clickable for line selection).
    ThreadContent,
    /// An action row with clickable buttons for a specific comment.
    ThreadActions { comment_id: String },
}

/// In-file search state.
#[derive(Default)]
pub struct SearchState {
    /// Current search query (empty = no active search).
    pub search_query: TextInput,
    /// Line indices that match the current search query.
    pub search_matches: Vec<usize>,
    /// Index into search_matches for the current match.
    pub search_match_idx: usize,
    /// Whether the search input box is visible.
    pub search_active: bool,
}

/// Unified diff view state.
#[derive(Default)]
pub struct DiffViewState {
    /// Whether the viewer is in unified diff mode.
    pub diff_mode: bool,
    /// Unified diff view entries (populated when entering diff mode).
    pub diff_view_lines: Vec<UnifiedDiffEntry>,
    /// Vertical scroll offset for the diff view.
    pub diff_view_scroll: usize,
    /// Cached max line number for diff view (avoids O(n) scan per frame).
    pub diff_view_max_line_no: usize,
    /// Maps each rendered screen row (within the diff viewport) back to its
    /// index in `diff_view_lines`, or `None` for injected inline-thread rows.
    /// Written during render; used by mouse handling (e.g. expand-context)
    /// since inserted thread rows break the simple scroll+offset arithmetic.
    pub screen_entry_map: Vec<Option<usize>>,
}

/// Which view the Explorer's bottom pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplorerBottomView {
    /// The changed-files diff list.
    #[default]
    DiffList,
    /// The review comment list.
    Comments,
    /// The AI walkthrough (steps + selected step's body).
    Walkthrough,
}

/// Explorer panel state (selections, scrolls).
pub struct ExplorerState {
    /// Index of the selected diff file in the diff list.
    pub diff_list_selected: usize,
    /// Vertical scroll offset for the diff list.
    pub diff_list_scroll: usize,
    /// Whether the explorer panel focus is on the diff list (bottom half).
    pub explorer_focus_on_diff_list: bool,
    /// Last known inner height of the explorer file-tree pane (updated during render).
    pub explorer_tree_height: usize,
    /// Last known inner height of the explorer diff-list pane (updated during render).
    pub explorer_diff_list_height: usize,
    /// Which view the explorer's bottom pane is currently showing.
    pub explorer_bottom_view: ExplorerBottomView,
    /// Index of the selected comment in the explorer comment list.
    pub comment_list_selected: usize,
    /// Vertical scroll offset for the explorer comment list.
    pub comment_list_scroll: usize,
    /// Line number (1-indexed) for comment preview triggered by single-clicking a comment marker.
    pub comment_preview_line: Option<usize>,
    /// Set of 1-indexed line numbers whose inline comment threads are expanded.
    pub expanded_inline_threads: HashSet<usize>,
    /// Line number where inline reply input is active (None = not replying).
    pub inline_reply_line: Option<usize>,
    /// Comment ID that the inline reply targets.
    pub inline_reply_comment_id: Option<String>,
    /// Text buffer for inline reply input.
    pub inline_reply_buffer: TextInput,
    /// Index of the selected step in the walkthrough view.
    pub walkthrough_selected: usize,
    /// Vertical scroll offset for the walkthrough step list.
    pub walkthrough_scroll: usize,
    /// IDs of walkthrough steps that have been jumped to at least once.
    pub viewed_steps: HashSet<String>,
    /// Whether the walkthrough step detail overlay (`space`) is open.
    pub walkthrough_detail_active: bool,
    /// Relative paths of files marked "viewed" by the reviewer.
    pub viewed: HashSet<String>,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            diff_list_selected: 0,
            diff_list_scroll: 0,
            explorer_focus_on_diff_list: false,
            explorer_tree_height: 20,
            explorer_diff_list_height: 20,
            explorer_bottom_view: ExplorerBottomView::default(),
            comment_list_selected: 0,
            comment_list_scroll: 0,
            comment_preview_line: None,
            expanded_inline_threads: HashSet::new(),
            inline_reply_line: None,
            inline_reply_comment_id: None,
            inline_reply_buffer: TextInput::new_multiline(),
            walkthrough_selected: 0,
            walkthrough_scroll: 0,
            viewed_steps: HashSet::new(),
            walkthrough_detail_active: false,
            viewed: HashSet::new(),
        }
    }
}

/// Line selection state for comments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LineSelection {
    /// No line is selected.
    #[default]
    None,
    /// A range is being dragged out — start line set, end not yet committed.
    /// Retained (with its dimmer "pending range" rendering) for a future
    /// click-drag range selection; commenting currently commits the range
    /// immediately (single click / shift-click), so nothing constructs this
    /// today.
    #[allow(dead_code)]
    Pending { start: usize },
    /// Range fully selected (start and end are 1-indexed, inclusive).
    /// `start` may be > `end` — callers normalize via `selected_range()`.
    Selected { start: usize, end: usize },
}

/// Fuzzy filename search state.
#[derive(Default)]
pub struct FilenameSearchState {
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

/// Symbol hover info for Cmd+hover underline.
#[derive(Debug, Clone)]
pub struct HoverSymbol {
    /// The symbol text (e.g. "AppState").
    #[allow(dead_code)]
    pub text: String,
    /// Line number (1-indexed) where the symbol is located.
    pub line: usize,
    /// Start column (0-indexed, in content characters before h_scroll).
    pub start_col: usize,
    /// End column (exclusive, 0-indexed).
    pub end_col: usize,
}

/// Double-click tracking state.
pub struct ClickTracker {
    /// Line number (1-indexed) currently under the mouse cursor in the viewer panel.
    pub hover_line: Option<usize>,
    /// Line number (1-indexed) when the mouse cursor is specifically over the gutter (line-number area).
    pub hover_gutter_line: Option<usize>,
    /// Symbol under the mouse cursor when Cmd/Ctrl is held (for underline + click-to-jump).
    pub hover_symbol: Option<HoverSymbol>,
    /// Timestamp (ms) of the last line-number click for double-click detection.
    pub last_line_click_time: std::time::Instant,
    /// The 1-indexed line number that was last clicked on.
    pub last_line_click_line: usize,
    /// While a gutter drag is in progress, the 1-indexed line where it started
    /// (the anchor). The range extends to the dragged-over line; the comment
    /// opens on mouse-up. `None` when not dragging.
    pub gutter_drag_anchor: Option<usize>,
    /// Timestamp of the last file-tree click for double-click detection.
    pub last_tree_click_time: std::time::Instant,
    /// The tree index that was last clicked in the file tree.
    pub last_tree_click_idx: usize,
    /// Timestamp of the last comment-list click for double-click detection.
    pub last_comment_click_time: std::time::Instant,
    /// The index that was last clicked in the comment list.
    pub last_comment_click_idx: usize,
}

impl Default for ClickTracker {
    fn default() -> Self {
        Self {
            hover_line: None,
            hover_gutter_line: None,
            hover_symbol: None,
            last_line_click_time: std::time::Instant::now(),
            last_line_click_line: 0,
            gutter_drag_anchor: None,
            last_tree_click_time: std::time::Instant::now(),
            last_tree_click_idx: usize::MAX,
            last_comment_click_time: std::time::Instant::now(),
            last_comment_click_idx: usize::MAX,
        }
    }
}

// ── Main struct ──────────────────────────────────────────────────────

/// All state owned by the Viewer mode.
#[derive(Default)]
pub struct ViewerState {
    /// File tree management.
    pub tree: FileTreeState,
    /// File content viewing.
    pub content: FileContentState,
    /// In-file search.
    pub search: SearchState,
    /// Unified diff view.
    pub diff_view: DiffViewState,
    /// Explorer panel state (selections, scrolls).
    pub explorer: ExplorerState,
    /// Line selection for comments.
    pub selection: LineSelection,
    /// Fuzzy filename search.
    pub filename_search: FilenameSearchState,
    /// Media rendering state (images/videos displayed as ASCII art).
    pub media_state: MediaState,
    /// Double-click tracking.
    pub click: ClickTracker,
    /// Whether 'g' was pressed and waiting for a second key (gd, gi, gr).
    pub pending_g_key: bool,
    /// Whether the viewer is showing the branch change-summary pseudo-file
    /// (the "SUMMARY" entry) instead of file content / diff. Mutually exclusive
    /// with `diff_view.diff_mode`; see `enter_summary_view` / `exit_summary_view`.
    pub show_summary: bool,
    /// Vertical scroll offset within the summary view.
    pub summary_scroll: usize,
    /// Total wrapped line count of the summary view, written during render and
    /// read by the key handler to clamp `summary_scroll`.
    pub summary_total_lines: usize,
}

/// The new-file line number a diff entry represents, for entries that map to
/// one: a concrete `Line` (`None` for a deletion, which has no new-file
/// line), or an `ExpandableContext`'s first hidden line. `HunkSeparator`
/// carries no line number.
fn diff_entry_new_line_no(entry: &UnifiedDiffEntry) -> Option<usize> {
    match entry {
        UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
        UnifiedDiffEntry::ExpandableContext { new_line_start, .. } => Some(*new_line_start),
        UnifiedDiffEntry::HunkSeparator { .. } => None,
    }
}

impl ViewerState {
    /// Invalidate the cached diff annotations (call when diff data changes).
    pub fn invalidate_diff_annotations(&mut self) {
        self.content.cached_diff_annotations = None;
        self.content.cached_diff_annotations_file = None;
    }

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
                // Preserve diff mode state across tree refreshes so that
                // file-watcher / periodic refreshes don't kick the user
                // out of the unified diff view.
                let was_diff_mode = self.diff_view.diff_mode;
                let prev_diff_lines = if was_diff_mode {
                    std::mem::take(&mut self.diff_view.diff_view_lines)
                } else {
                    Vec::new()
                };
                let prev_diff_scroll = self.diff_view.diff_view_scroll;
                let prev_diff_max_line_no = self.diff_view.diff_view_max_line_no;

                self.open_file(worktree_path, rel_path, tab_width);
                self.content.file_scroll = prev_file_scroll;
                self.content.h_scroll = prev_h_scroll;

                if was_diff_mode {
                    self.diff_view.diff_mode = true;
                    self.diff_view.diff_view_lines = prev_diff_lines;
                    self.diff_view.diff_view_scroll = prev_diff_scroll;
                    self.diff_view.diff_view_max_line_no = prev_diff_max_line_no;
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

    /// Open (read) a file and store its lines in `file_content`.
    pub fn open_file(&mut self, worktree_path: &Path, relative_path: &str, tab_width: usize) {
        self.exit_diff_mode();
        self.content.highlighted_lines.clear();
        self.content.highlighted_cache_key = None;
        self.content.grep_highlight_line = None;
        self.content.test_runs.clear();
        let full = worktree_path.join(relative_path);

        // Handle media files (images/videos) via aa-media.
        if media_state::is_media_file(relative_path) {
            self.content.file_content.clear();
            self.content.current_file = Some(relative_path.to_string());
            self.content.file_scroll = 0;
            self.content.h_scroll = 0;
            // Actual rendering is triggered lazily during render (when panel
            // size is known). Clear the cache so it re-renders for the new file.
            self.media_state.clear();
            return;
        }

        // Clear media state when opening a non-media file.
        self.media_state.clear();

        match fs::read_to_string(&full) {
            Ok(text) => {
                self.content.file_content = text
                    .lines()
                    .map(|l| Self::expand_tabs(l, tab_width))
                    .collect();
                // If file is empty but not zero-length, show one empty line.
                if self.content.file_content.is_empty() && !text.is_empty() {
                    self.content.file_content.push(String::new());
                }
                self.content.current_file = Some(relative_path.to_string());
                self.content.file_scroll = 0;
                self.content.h_scroll = 0;
                self.content.test_runs =
                    crate::go_test::scan_go_test_runs(&self.content.file_content, relative_path);
            }
            Err(e) => {
                // Show error as file content so the user sees feedback.
                self.content.file_content = vec![format!("Error reading file: {e}")];
                self.content.current_file = Some(relative_path.to_string());
                self.content.file_scroll = 0;
                self.content.h_scroll = 0;
            }
        }
    }

    /// Returns true if the current file is a media file.
    pub fn is_current_file_media(&self) -> bool {
        self.content
            .current_file
            .as_deref()
            .is_some_and(media_state::is_media_file)
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

    /// Execute a search over the file content and populate search_matches.
    pub fn execute_search(&mut self) {
        self.search.search_matches.clear();
        self.search.search_match_idx = 0;

        if self.search.search_query.is_empty() {
            return;
        }

        let query_lower = self.search.search_query.to_lowercase();
        for (i, line) in self.content.file_content.iter().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                self.search.search_matches.push(i);
            }
        }

        // Jump to first match at or after current scroll.
        if !self.search.search_matches.is_empty() {
            self.search.search_match_idx = self
                .search
                .search_matches
                .iter()
                .position(|&line| line >= self.content.file_scroll)
                .unwrap_or(0);
            self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        }
        self.sync_diff_scroll_to_file_scroll();
    }

    /// Jump to the next search match.
    pub fn next_search_match(&mut self) {
        if self.search.search_matches.is_empty() {
            return;
        }
        self.search.search_match_idx =
            (self.search.search_match_idx + 1) % self.search.search_matches.len();
        self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        self.sync_diff_scroll_to_file_scroll();
    }

    /// Jump to the previous search match.
    pub fn prev_search_match(&mut self) {
        if self.search.search_matches.is_empty() {
            return;
        }
        self.search.search_match_idx = if self.search.search_match_idx == 0 {
            self.search.search_matches.len() - 1
        } else {
            self.search.search_match_idx - 1
        };
        self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        self.sync_diff_scroll_to_file_scroll();
    }

    /// Resolve the file line (0-indexed, matching `content.file_scroll`) that
    /// the diff view's current scroll position corresponds to, from the
    /// nearest concrete new-file line number at or after `diff_view_scroll`
    /// (falling back to the nearest one before it — e.g. when the cursor
    /// sits on a deleted line, which has no new-file line number).
    fn diff_scroll_file_line(&self) -> Option<usize> {
        let lines = &self.diff_view.diff_view_lines;
        let scroll = self.diff_view.diff_view_scroll.min(lines.len());
        lines[scroll..]
            .iter()
            .find_map(diff_entry_new_line_no)
            .or_else(|| {
                lines[..scroll]
                    .iter()
                    .rev()
                    .find_map(diff_entry_new_line_no)
            })
            .map(|n| n.saturating_sub(1))
    }

    /// Keep `content.file_scroll` in sync with the diff view's scroll
    /// position. Symbol lookup and search operate on `content.file_scroll`
    /// unconditionally (they predate diff mode), so anything reached while
    /// browsing a diff needs this synced first, or it would act on whatever
    /// line plain-view browsing last left `file_scroll` at. A no-op outside
    /// diff mode.
    pub fn sync_file_scroll_to_diff_scroll(&mut self) {
        if !self.diff_view.diff_mode {
            return;
        }
        if let Some(line) = self.diff_scroll_file_line() {
            self.content.file_scroll = line;
        }
    }

    /// Keep the diff view's scroll position in sync with `content.file_scroll`
    /// after it moves on its own (e.g. a search match) so the diff pane
    /// visibly follows along instead of staying put while the underlying
    /// cursor moves. A no-op outside diff mode.
    fn sync_diff_scroll_to_file_scroll(&mut self) {
        if !self.diff_view.diff_mode {
            return;
        }
        let target_line = self.content.file_scroll + 1; // new_line_no is 1-indexed
        if let Some(idx) = self
            .diff_view
            .diff_view_lines
            .iter()
            .position(|entry| diff_entry_new_line_no(entry).is_some_and(|n| n >= target_line))
        {
            self.diff_view.diff_view_scroll = idx;
        }
    }

    // -- Filename fuzzy search ------------------------------------------------

    /// Run fuzzy filename search over the cached file list and populate results.
    pub fn execute_filename_search(&mut self) {
        self.filename_search.filename_search_results.clear();

        let query = self.filename_search.filename_search_query.to_lowercase();

        for path in &self.filename_search.filename_search_all_files {
            let path_lower = path.to_lowercase();
            let name_lower = path.rsplit('/').next().unwrap_or(path).to_lowercase();

            // If query is empty, include all files with score 0.
            if query.is_empty() {
                self.filename_search
                    .filename_search_results
                    .push(ScoredFile {
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

            self.filename_search
                .filename_search_results
                .push(ScoredFile {
                    path: path.clone(),
                    score,
                });
        }

        // Sort by score descending, then path ascending for stability.
        self.filename_search
            .filename_search_results
            .sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
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
    ///
    /// Computes a hash of `(current_file, file_content)` and skips
    /// re-highlighting when the content has not changed since the last call.
    pub fn highlight_content(&mut self, syntax_set: &SyntaxSet, theme: &SyntectTheme) {
        if self.content.file_content.is_empty() {
            self.content.highlighted_lines.clear();
            self.content.highlighted_cache_key = None;
            return;
        }

        // Compute a cache key from the file path and content.
        let hash = {
            let mut hasher = DefaultHasher::new();
            self.content.current_file.hash(&mut hasher);
            self.content.file_content.hash(&mut hasher);
            hasher.finish()
        };

        if self.content.highlighted_cache_key == Some(hash) {
            return; // Content unchanged — skip redundant highlighting.
        }

        self.content.highlighted_lines.clear();

        // Determine syntax from file extension.
        let ext = self
            .content
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
            .content
            .file_content
            .iter()
            .map(|l| format!("{l}\n"))
            .collect();

        for line in LinesWithEndings::from(&full_text) {
            let ranges = match h.highlight_line(line, syntax_set) {
                Ok(r) => r,
                Err(_) => {
                    // Fallback: plain white.
                    self.content.highlighted_lines.push(vec![(
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

            self.content.highlighted_lines.push(spans);
        }

        self.content.highlighted_cache_key = Some(hash);
    }

    // -- Line selection helpers -----------------------------------------------

    /// Clear the current line selection.
    pub fn clear_selection(&mut self) {
        self.selection = LineSelection::None;
    }

    /// Return the selected range as `(start, end)` (both 1-indexed, inclusive,
    /// normalized so start <= end). Returns `None` if no line is selected.
    pub fn selected_range(&self) -> Option<(usize, usize)> {
        match self.selection {
            LineSelection::None => None,
            LineSelection::Pending { start } => Some((start, start)),
            LineSelection::Selected { start, end } => Some(if start <= end {
                (start, end)
            } else {
                (end, start)
            }),
        }
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

    /// Whether the selection is in the pending state (first click done, waiting
    /// for second).
    pub fn is_selection_pending(&self) -> bool {
        matches!(self.selection, LineSelection::Pending { .. })
    }

    /// Handle a click on the gutter "+" button (GitHub-style commenting).
    ///
    /// A plain click selects just `line_1indexed`; a shift-click extends a
    /// range from the previously clicked line (the anchor, kept fixed so
    /// successive shift-clicks grow from the same origin). The caller then
    /// opens the comment input, which reads the resulting `selection`.
    pub fn gutter_comment_click(&mut self, line_1indexed: usize, extend: bool) {
        let anchor = self.click.last_line_click_line;
        if extend && anchor != 0 {
            let (start, end) = if anchor <= line_1indexed {
                (anchor, line_1indexed)
            } else {
                (line_1indexed, anchor)
            };
            self.selection = LineSelection::Selected { start, end };
        } else {
            self.selection = LineSelection::Selected {
                start: line_1indexed,
                end: line_1indexed,
            };
            self.click.last_line_click_line = line_1indexed;
            self.click.last_line_click_time = std::time::Instant::now();
        }
    }

    /// Return the total gutter width (in columns) used by the line-number
    /// area in the viewer panel.  The gutter consists of:
    ///   prefix(1) + digits(gutter_width) + space(1) + '│'(1) + space(1)
    /// = gutter_width + 4
    pub fn gutter_total_width(&self) -> u16 {
        let digit_w = if self.diff_view.diff_mode {
            // Must match the renderer's gutter width exactly, or mouse hit-testing
            // (badge/thread toggles, symbol jumps) drifts off by a column. The
            // renderer uses `diff_view_max_line_no`, which also counts the
            // `new_line_end` of collapsed (ExpandableContext) regions — those can
            // out-digit every *visible* line, so recomputing from `Line` entries
            // alone here would under-count and shift every click target left.
            digit_count(self.diff_view.diff_view_max_line_no)
        } else {
            digit_count(self.content.file_content.len())
        };
        (digit_w + 4) as u16
    }

    // -- Unified diff view ----------------------------------------------------

    /// Build the unified diff view entries from a `FileDiff`.
    ///
    /// Inserts `ExpandableContext` entries between hunks to represent hidden
    /// context lines that can be expanded on demand.
    pub fn build_unified_diff_view(&mut self, file_diff: &FileDiff) {
        self.diff_view.diff_view_lines.clear();

        let total_new_lines = self.content.file_content.len();

        // Helper: find the max new_line_no in a hunk.
        let hunk_max_new_line = |hunk: &DiffHunk| -> usize {
            hunk.lines
                .iter()
                .filter_map(|l| l.new_line_no)
                .max()
                .unwrap_or(0)
        };
        // Helper: find the min new_line_no in a hunk.
        let hunk_min_new_line = |hunk: &DiffHunk| -> usize {
            hunk.lines
                .iter()
                .filter_map(|l| l.new_line_no)
                .min()
                .unwrap_or(0)
        };

        for (hunk_idx, hunk) in file_diff.hunks.iter().enumerate() {
            if hunk_idx == 0 {
                // Before the first hunk: check for hidden lines at top of file.
                let first_new = hunk_min_new_line(hunk);
                if first_new > 1 {
                    let hidden_start = 1;
                    let hidden_end = first_new - 1;
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::ExpandableContext {
                            hidden_count: hidden_end - hidden_start + 1,
                            new_line_start: hidden_start,
                            new_line_end: hidden_end,
                            func_header: hunk.func_header.clone(),
                        });
                }
            } else {
                // Between hunks: compute hidden range.
                let prev_hunk = &file_diff.hunks[hunk_idx - 1];
                let prev_end = hunk_max_new_line(prev_hunk);
                let curr_start = hunk_min_new_line(hunk);
                let hidden_start = prev_end + 1;
                let hidden_end = curr_start.saturating_sub(1);
                if hidden_start <= hidden_end {
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::ExpandableContext {
                            hidden_count: hidden_end - hidden_start + 1,
                            new_line_start: hidden_start,
                            new_line_end: hidden_end,
                            func_header: hunk.func_header.clone(),
                        });
                } else {
                    // No hidden lines — keep a visual separator.
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::HunkSeparator {
                            func_header: hunk.func_header.clone(),
                        });
                }
            }

            for line in &hunk.lines {
                self.diff_view.diff_view_lines.push(UnifiedDiffEntry::Line {
                    tag: line.tag,
                    new_line_no: line.new_line_no,
                    content: line.content.clone(),
                    inline_segments: line.inline_segments.clone(),
                });
            }
        }

        // After the last hunk: check for hidden lines at bottom of file.
        if let Some(last_hunk) = file_diff.hunks.last() {
            let last_new = hunk_max_new_line(last_hunk);
            if last_new < total_new_lines {
                let hidden_start = last_new + 1;
                let hidden_end = total_new_lines;
                self.diff_view
                    .diff_view_lines
                    .push(UnifiedDiffEntry::ExpandableContext {
                        hidden_count: hidden_end - hidden_start + 1,
                        new_line_start: hidden_start,
                        new_line_end: hidden_end,
                        func_header: None,
                    });
            }
        }

        self.recalc_diff_max_line_no();

        if !self.diff_view.diff_view_lines.is_empty() {
            self.diff_view.diff_mode = true;
            self.diff_view.diff_view_scroll = 0;
        }
    }

    /// Recalculate the cached max line number from current diff view lines.
    fn recalc_diff_max_line_no(&mut self) {
        self.diff_view.diff_view_max_line_no = self
            .diff_view
            .diff_view_lines
            .iter()
            .filter_map(|e| match e {
                UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
                UnifiedDiffEntry::ExpandableContext { new_line_end, .. } => Some(*new_line_end),
                _ => None,
            })
            .max()
            .unwrap_or(0);
    }

    /// Return the maximum line width (in characters) of the current content.
    ///
    /// In diff mode this scans `diff_view_lines`; otherwise it scans
    /// `file_content`. Returns 0 when there is nothing to display.
    pub fn max_content_width(&self) -> usize {
        if self.diff_view.diff_mode {
            self.diff_view
                .diff_view_lines
                .iter()
                .map(|entry| match entry {
                    UnifiedDiffEntry::Line { content, .. } => content.chars().count(),
                    UnifiedDiffEntry::HunkSeparator { func_header }
                    | UnifiedDiffEntry::ExpandableContext { func_header, .. } => {
                        func_header.as_ref().map_or(0, |h| h.chars().count())
                    }
                })
                .max()
                .unwrap_or(0)
        } else {
            self.content
                .file_content
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
        self.content.h_scroll = (self.content.h_scroll + delta).min(limit);
    }

    /// Exit unified diff mode and reset related state. Also leaves the summary
    /// pseudo-file view — every file-open path funnels through here, so this is
    /// the single place that guarantees `show_summary` and `diff_mode` are never
    /// both set.
    pub fn exit_diff_mode(&mut self) {
        self.diff_view.diff_mode = false;
        self.diff_view.diff_view_lines.clear();
        self.diff_view.diff_view_scroll = 0;
        self.diff_view.diff_view_max_line_no = 0;
        self.show_summary = false;
        self.summary_scroll = 0;
    }

    /// Whether the viewer is currently showing the summary pseudo-file.
    pub fn is_summary(&self) -> bool {
        self.show_summary
    }

    /// Enter the summary pseudo-file view, leaving any diff/file content. Kept
    /// mutually exclusive with diff mode via `exit_diff_mode`.
    pub fn enter_summary_view(&mut self) {
        self.exit_diff_mode();
        self.show_summary = true;
        self.summary_scroll = 0;
    }

    /// Expand hidden context lines at the given index in `diff_view_lines`.
    ///
    /// If `expand_all` is true, all hidden lines are revealed. Otherwise,
    /// up to 10 lines are revealed — 5 from the top and 5 from the bottom
    /// of the hidden range (GitHub-style bidirectional expansion).
    /// Returns `true` if expansion occurred.
    pub fn expand_context_at(&mut self, idx: usize, expand_all: bool) -> bool {
        let entry = match self.diff_view.diff_view_lines.get(idx) {
            Some(UnifiedDiffEntry::ExpandableContext { .. }) => {
                self.diff_view.diff_view_lines[idx].clone()
            }
            _ => return false,
        };

        let (hidden_count, new_line_start, new_line_end, func_header) = match entry {
            UnifiedDiffEntry::ExpandableContext {
                hidden_count,
                new_line_start,
                new_line_end,
                func_header,
            } => (hidden_count, new_line_start, new_line_end, func_header),
            _ => unreachable!(),
        };

        if expand_all || hidden_count <= 10 {
            // Reveal all hidden lines.
            let mut new_entries: Vec<UnifiedDiffEntry> = Vec::with_capacity(hidden_count);
            for line_no in new_line_start..=new_line_end {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            let added = new_entries.len();
            self.diff_view
                .diff_view_lines
                .splice(idx..=idx, new_entries);

            if idx < self.diff_view.diff_view_scroll {
                let delta = added.saturating_sub(1);
                self.diff_view.diff_view_scroll += delta;
            }
        } else {
            // Bidirectional: reveal 5 from top + 5 from bottom.
            let top_count = 5usize;
            let bottom_count = 5usize;

            let mut new_entries: Vec<UnifiedDiffEntry> =
                Vec::with_capacity(top_count + bottom_count + 1);

            // Top lines (immediately after previous hunk).
            for line_no in new_line_start..new_line_start + top_count {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            // Smaller ExpandableContext for the remaining middle.
            let remaining_start = new_line_start + top_count;
            let remaining_end = new_line_end - bottom_count;
            new_entries.push(UnifiedDiffEntry::ExpandableContext {
                hidden_count: remaining_end - remaining_start + 1,
                new_line_start: remaining_start,
                new_line_end: remaining_end,
                func_header,
            });

            // Bottom lines (immediately before next hunk).
            for line_no in (new_line_end - bottom_count + 1)..=new_line_end {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            let added = new_entries.len();
            self.diff_view
                .diff_view_lines
                .splice(idx..=idx, new_entries);

            if idx < self.diff_view.diff_view_scroll {
                let delta = added.saturating_sub(1);
                self.diff_view.diff_view_scroll += delta;
            }
        }

        self.recalc_diff_max_line_no();
        true
    }

    /// Find the first `ExpandableContext` entry visible in the current viewport
    /// and return its index.
    pub fn find_visible_expandable(&self, viewport_height: usize) -> Option<usize> {
        let start = self.diff_view.diff_view_scroll;
        let end = (start + viewport_height).min(self.diff_view.diff_view_lines.len());
        for i in start..end {
            if matches!(
                self.diff_view.diff_view_lines.get(i),
                Some(UnifiedDiffEntry::ExpandableContext { .. })
            ) {
                return Some(i);
            }
        }
        None
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

    /// Walk `dir` and append its immediate children to `entries`.
    /// All directories start collapsed with `children_loaded: false`;
    /// their contents are loaded lazily when the user expands them.
    pub fn walk_dir(root: &Path, dir: &Path, depth: usize, entries: &mut Vec<FileTreeEntry>) {
        if depth > Self::MAX_DEPTH {
            return;
        }

        let Ok(read_dir) = fs::read_dir(dir) else {
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

/// Count the number of decimal digits in `n` (minimum 1).
fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Builds a `Line` entry with the given new-file line number (`None` for
    /// a deletion, which has no new-file line).
    fn diff_line(new_line_no: Option<usize>) -> UnifiedDiffEntry {
        UnifiedDiffEntry::Line {
            tag: DiffLineTag::Equal,
            new_line_no,
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }

    #[test]
    fn sync_file_scroll_to_diff_scroll_resolves_deletion_lines_forward() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = true;
        vs.diff_view.diff_view_lines = vec![
            UnifiedDiffEntry::HunkSeparator { func_header: None },
            diff_line(Some(10)), // idx 1
            diff_line(None),     // idx 2 — a deletion, no new-file line
            diff_line(Some(11)), // idx 3
        ];

        // Scrolled onto the deletion: no new-file line at this exact index,
        // so the cursor resolves forward to the next concrete line (11).
        vs.diff_view.diff_view_scroll = 2;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 10); // line 11, 0-indexed

        // Scrolled directly onto a concrete line: resolves to that line.
        vs.diff_view.diff_view_scroll = 1;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 9); // line 10, 0-indexed
    }

    #[test]
    fn sync_file_scroll_to_diff_scroll_is_noop_outside_diff_mode() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = false;
        vs.diff_view.diff_view_lines = vec![diff_line(Some(5))];
        vs.diff_view.diff_view_scroll = 0;
        vs.content.file_scroll = 42;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 42);
    }

    #[test]
    fn sync_diff_scroll_to_file_scroll_follows_a_search_jump() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = true;
        vs.diff_view.diff_view_lines = vec![
            diff_line(Some(1)), // idx 0
            diff_line(Some(2)), // idx 1
            diff_line(Some(3)), // idx 2
        ];
        vs.diff_view.diff_view_scroll = 0;

        // A search match landed on file_scroll = 2 (line 3, 0-indexed).
        vs.content.file_scroll = 2;
        vs.sync_diff_scroll_to_file_scroll();
        assert_eq!(vs.diff_view.diff_view_scroll, 2);
    }

    #[test]
    fn gutter_click_selects_single_line() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(7, false);
        assert_eq!(vs.selected_range(), Some((7, 7)));
    }

    #[test]
    fn shift_gutter_click_extends_range_from_anchor() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(5, false); // anchor at 5
        vs.gutter_comment_click(9, true); // shift-click extends to 9
        assert_eq!(vs.selected_range(), Some((5, 9)));
    }

    #[test]
    fn shift_gutter_click_normalizes_upward_range() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(9, false); // anchor at 9
        vs.gutter_comment_click(4, true); // shift-click above the anchor
        assert_eq!(vs.selected_range(), Some((4, 9)));
    }

    #[test]
    fn shift_gutter_click_without_anchor_falls_back_to_single_line() {
        let mut vs = ViewerState::default();
        // No prior click → anchor is the default 0, so this is just a single line.
        vs.gutter_comment_click(3, true);
        assert_eq!(vs.selected_range(), Some((3, 3)));
    }

    /// The explorer must list files purely from the filesystem, independent of
    /// git state. A directory ignored by `.gitignore` (i.e. not under git
    /// management) and the files nested inside it must still be reachable.
    #[test]
    fn walk_includes_gitignored_directories_and_recurses() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();

        // A `.gitignore` that excludes `generated/` (and `*.log`) from git
        // management. `generated` is deliberately NOT one of the heavy
        // `SKIP_DIRS`, so the only reason it could be hidden would be gitignore.
        fs::write(root.join(".gitignore"), "/generated\n*.log\n").unwrap();
        fs::create_dir_all(root.join("generated/sub")).unwrap();
        fs::write(root.join("generated/out.txt"), "x").unwrap();
        fs::write(root.join("generated/sub/inner.txt"), "x").unwrap();
        fs::write(root.join("generated/debug.log"), "x").unwrap();

        // Top-level walk must surface the gitignored directory itself.
        let mut top = Vec::new();
        ViewerState::walk_dir(root, root, 0, &mut top);
        assert!(
            top.iter().any(|e| e.name == "generated" && e.is_dir),
            "gitignored directory should still be listed: {:?}",
            top.iter().map(|e| &e.name).collect::<Vec<_>>()
        );

        // Expanding it must reveal nested files, including gitignored ones.
        let mut children = Vec::new();
        ViewerState::read_dir_entries(root, &root.join("generated"), 1, &mut children);
        let names: Vec<&str> = children.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"out.txt"), "files: {names:?}");
        assert!(names.contains(&"sub"), "files: {names:?}");
        assert!(
            names.contains(&"debug.log"),
            "gitignored file should be listed: {names:?}"
        );

        // And recursion continues one level deeper.
        let mut deep = Vec::new();
        ViewerState::read_dir_entries(root, &root.join("generated/sub"), 2, &mut deep);
        assert!(
            deep.iter().any(|e| e.name == "inner.txt"),
            "deep files: {:?}",
            deep.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
    }

    /// Heavy build/dependency directories are still skipped — that guard is a
    /// performance concern, not a git-management one.
    #[test]
    fn walk_still_skips_heavy_dirs() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();

        let mut top = Vec::new();
        ViewerState::walk_dir(root, root, 0, &mut top);
        assert!(top.iter().any(|e| e.name == "src"));
        assert!(
            !top.iter().any(|e| e.name == "node_modules"),
            "node_modules should be skipped"
        );
    }
}
