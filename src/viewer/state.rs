//! Viewer state struct definitions.
//!
//! All the sub-structs that together make up [`ViewerState`], plus the small
//! enums (`ScreenRow`, `ExplorerBottomView`, `LineSelection`) they hold.
//! Behavior (methods) lives in sibling modules; this file only defines
//! layout.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use crate::git_engine::status_map::GitStatusMap;
use crate::media_state::MediaState;
use crate::text_input::TextInput;

use super::file_tree::{FileTreeEntry, ScoredFile};
use super::file_view::UnifiedDiffEntry;

// ── Sub-structs ──────────────────────────────────────────────────────

/// File tree management state.
#[derive(Default)]
pub struct FileTreeState {
    /// このツリーを歩いた根。エントリの相対パスはすべてここからの相対で、
    /// 絶対パスに戻せるのはこの値だけ。
    ///
    /// 読むのは [`ViewerState::root`]、書くのは [`ViewerState::load_file_tree`] /
    /// [`ViewerState::replace_tree`] / [`ViewerState::set_root`] だけに限る。
    /// 以前は根を持たず、ファイルを開くたびに呼び出し側が「今どの worktree か」
    /// を引き直して渡していたので、表示中のツリーと開く先が食い違っても誰も
    /// 気付けなかった (worktree 切り替えはツリーの走査を裏に回すため、古い
    /// エントリと新しい根が同時に存在する瞬間がある)。
    pub(in crate::viewer) root: PathBuf,
    /// Flattened file tree (directories + files, pre-order).
    pub file_tree: Vec<FileTreeEntry>,
    /// Index of the selected row in the *full* (unfiltered) tree.
    pub tree_selected: usize,
    /// Vertical scroll offset for the tree pane.
    pub tree_scroll: usize,
    /// Cached result of `visible_indices()`. Invalidated when tree structure changes.
    pub cached_visible_indices: Option<Rc<Vec<usize>>>,
    /// Git status snapshot backing each entry's `git_state`. Refreshed once
    /// per `load_file_tree()` call (not per frame, not per entry) and
    /// reused by `ensure_children_loaded()` for lazily-loaded children in
    /// between full rebuilds — see D5 in the plan doc.
    pub git_status: GitStatusMap,
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
    /// なぜ `file_content` が空なのかの理由。読み込みに失敗したときだけ入る。
    ///
    /// 「未選択」「中身が空のファイル」「読めなかった」はどれも `file_content` が
    /// 空になるので、これが無いと Viewer は 3 つを見分けられず、失敗が黙って
    /// 「ファイル未選択」に丸められる。`open_file` が成功したら必ず消す。
    pub load_error: Option<String>,
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
    /// Runnable tests in the current file, keyed by 1-indexed line number.
    /// Populated by the language-specific scanner ([`crate::go_test`] for
    /// `*_test.go`, [`crate::rust_test`] for `*.rs`); empty for other files.
    /// Drives the ▶ run buttons.
    pub test_runs: std::collections::HashMap<usize, crate::test_run::TestRun>,
    /// Which identifier occurrences in the open file are code rather than
    /// prose inside a comment or string. Built from the file's own text when
    /// it is opened, so it always describes what is actually on screen —
    /// independent of whatever root the symbol index happens to be built over.
    /// Empty for languages we have no grammar for, which leaves them with no
    /// navigation rather than wrong navigation.
    pub code_mask: crate::symbol_index::CodeMask,
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
    /// Rows the diff list spends on its base-error banner (0 or 1, updated
    /// during render). The banner is not a `display_list` entry, so anything
    /// converting a screen row back into a list index — mouse clicks — has to
    /// subtract it or it selects the wrong file.
    pub explorer_diff_banner_rows: usize,
    /// Which view the explorer's bottom pane is currently showing.
    pub explorer_bottom_view: ExplorerBottomView,
    /// Index of the selected comment in the explorer comment list.
    pub comment_list_selected: usize,
    /// Vertical scroll offset for the explorer comment list.
    pub comment_list_scroll: usize,
    /// Set of 1-indexed line numbers whose inline comment threads are expanded.
    pub expanded_inline_threads: HashSet<usize>,
    /// Line number where inline reply input is active (None = not replying).
    pub inline_reply_line: Option<usize>,
    /// Comment ID that the inline reply targets.
    pub inline_reply_comment_id: Option<String>,
    /// Text buffer for inline reply input.
    pub inline_reply_buffer: TextInput,
    /// Index of the selected step in the walkthrough view (the list cursor
    /// that `j`/`k` move).
    pub walkthrough_selected: usize,
    /// Index of the step the Viewer is currently reflecting — the last one
    /// *jumped to* (`Enter`/`n`/`N`), which drives the Viewer's full-width
    /// step banner and line-range underline. Kept distinct from
    /// `walkthrough_selected` so merely moving the list cursor with `j`/`k`
    /// doesn't shift the Viewer out from under the reviewer. `None` until a
    /// step is jumped to.
    pub walkthrough_viewing: Option<usize>,
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
            explorer_diff_banner_rows: 0,
            explorer_bottom_view: ExplorerBottomView::default(),
            comment_list_selected: 0,
            comment_list_scroll: 0,
            expanded_inline_threads: HashSet::new(),
            inline_reply_line: None,
            inline_reply_comment_id: None,
            inline_reply_buffer: TextInput::new_multiline(),
            walkthrough_selected: 0,
            walkthrough_viewing: None,
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

/// Symbol hover info for the jump underline (D8: shown on any rest, not just
/// Cmd/Ctrl+hover — see `has_jump_modifier` below).
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
    /// Whether Cmd/Ctrl was held as of the last mouse-move over this symbol.
    /// Drives the underline's color (D8's 2-stage disclosure): `false` draws
    /// `theme.hint` ("there's a definition here"), `true` draws
    /// `theme.accent` ("press now to jump"). The click contract itself is
    /// unchanged — this only controls which promise the underline makes.
    pub has_jump_modifier: bool,
}

/// A symbol the mouse is resting on, awaiting the jump-underline's own
/// debounce (D9, 150ms — independent of the popup's 350ms `HOVER_IDLE` in
/// `code_nav.rs`) before it's promoted to `ClickTracker::hover_symbol`.
#[derive(Debug, Clone)]
pub struct PendingUnderline {
    pub symbol: String,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub since: std::time::Instant,
    /// Whether the (expensive, index-lookup) jumpability check has already
    /// run for this rested position, so the per-frame tick doesn't repeat it
    /// every frame while the mouse sits still.
    pub resolved: bool,
    pub has_jump_modifier: bool,
}

/// Double-click tracking state.
pub struct ClickTracker {
    /// Line number (1-indexed) currently under the mouse cursor in the viewer panel.
    pub hover_line: Option<usize>,
    /// Line number (1-indexed) when the mouse cursor is specifically over the gutter (line-number area).
    pub hover_gutter_line: Option<usize>,
    /// Resolved jump-underline target, shown once the mouse has rested on a
    /// jumpable symbol past the D9 debounce (`None` while waiting, or over a
    /// non-jumpable word — A7).
    pub hover_symbol: Option<HoverSymbol>,
    /// The rested-on candidate mid-debounce, before `hover_symbol` is decided.
    pub underline_pending: Option<PendingUnderline>,
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
            underline_pending: None,
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

/// Width (in columns) of the comment-marker column at the far left of the
/// Viewer — where the 💬/│ thread markers live, LEFT of the line numbers.
/// Kept separate from the "+" badge column (right of the numbers) so that
/// toggling an existing thread and starting a new comment never share a
/// click target: the whole gutter+badge side always starts a comment.
pub const COMMENT_MARKER_W: u16 = 2;

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
    /// Whether markdown files are shown rendered (SUMMARY-style prose) instead
    /// of as raw source. Sticky for the session: it survives opening another
    /// markdown file, and is simply ignored while a non-markdown file is open.
    /// Only takes effect in the plain-file view — see
    /// `is_showing_rendered_markdown`, which is what every renderer and event
    /// handler must gate on.
    pub md_rendered: bool,
    /// Vertical scroll offset within the rendered-markdown view. Reset per file
    /// (in `open_file`), unlike `md_rendered`.
    pub md_scroll: usize,
    /// Total wrapped line count of the rendered-markdown view, written during
    /// render and read by the key handler to clamp `md_scroll`.
    pub md_total_lines: usize,
}
