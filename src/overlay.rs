//! Overlay state types.
//!
//! Each overlay popup has its own state struct, extracted from the monolithic
//! `App` struct to improve organization and reduce field count.
//!
//! The `ActiveOverlay` enum tracks which overlay is currently visible,
//! replacing the previous `active: bool` field on each struct.

use crate::app::Focus;
use crate::background::BackgroundOp;
use crate::claude_sessions::ResumableSession;
use crate::git_engine::CommitInfo;
use crate::grep_search::GrepProgress;
use crate::review_store::SessionHistory;
use crate::search_result_tree::SearchResultTree;
use crate::text_input::TextInput;

/// Which overlay is currently active (at most one at a time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveOverlay {
    #[default]
    None,
    SwitchBranch,
    Grab,
    Prune,
    CherryPick,
    History,
    ResumeSession,
    RepoSelector,
    OpenRepo,
    /// PR-number/URL input for "Review: Review Pull Request…" — stays open
    /// (with the typed input preserved) if the intake attempt fails, so the
    /// user can correct and retry without retyping.
    PrInput,
    GrepSearch,
    Help,
    CommandPalette,
    /// Worktree switcher — the modal that replaced the left worktree column.
    /// Reuses the existing worktree list state and `handle_worktree_key`.
    WorktreeSwitcher,
    /// Full-screen comment list — overview of all review comments on the branch
    /// with jump-to-location. Reuses the comment list state + handler.
    CommentList,
    /// Theme picker — Up/Down to browse, live preview on each move, Enter to
    /// persist, Esc to revert to the theme that was active when the picker opened.
    ThemePicker,
}

/// Theme picker overlay state.
#[derive(Default)]
pub struct ThemePickerOverlay {
    /// All available theme names in display order (see `Theme::all_names`).
    pub themes: Vec<String>,
    /// Currently highlighted index within `themes`.
    pub selected: usize,
    /// The `theme_name` active when the picker was opened — used to revert on Esc.
    pub original: String,
}

/// Switch-branch overlay state.
#[derive(Default)]
pub struct SwitchBranchOverlay {
    pub branches: Vec<String>,
    pub selected: usize,
    pub filter: TextInput,
}

/// Grab-branch overlay state.
#[derive(Default)]
pub struct GrabOverlay {
    pub branches: Vec<String>,
    pub selected: usize,
    pub filter: TextInput,
}

/// Cherry-pick overlay state.
#[derive(Default)]
pub struct CherryPickOverlay {
    pub source_branch: String,
    pub commits: Vec<CommitInfo>,
    pub selected: usize,
}

/// Prune overlay state.
#[derive(Default)]
pub struct PruneOverlay {
    pub stale: Vec<String>,
}

/// Resume-session overlay state.
#[derive(Default)]
pub struct ResumeSessionOverlay {
    pub sessions: Vec<ResumableSession>,
    pub selected: usize,
    pub filter: TextInput,
    pub all_projects: bool,
}

/// Grep full-text search overlay state.
#[derive(Default)]
pub struct GrepSearchOverlay {
    pub query: TextInput,
    pub result_tree: SearchResultTree,
    pub selected: usize,
    pub scroll: usize,
    pub running: bool,
    pub bg_op: BackgroundOp<GrepProgress>,
    pub regex_mode: bool,
    pub case_sensitive: bool,
    /// Debounce timer for incremental search.
    pub debounce_deadline: Option<std::time::Instant>,
    /// Whether phase1 (recently-modified files only) results are currently displayed.
    pub phase1_active: bool,
    /// Background op for phase2 (full search) when doing 2-phase incremental search.
    pub bg_op_phase2: BackgroundOp<GrepProgress>,
    /// Accumulates raw matches from background search, rebuilt into tree on completion.
    pub pending_matches: Vec<crate::grep_search::GrepMatch>,
    /// Whether the query input field is focused (true) or the result list (false).
    /// Defaults to true so the input field is focused when the overlay opens.
    pub input_focused: bool,
}

/// Command palette overlay state.
#[derive(Default)]
pub struct CommandPaletteOverlay {
    pub filter: TextInput,
    pub selected: usize,
}

/// Session history overlay state.
#[derive(Default)]
pub struct HistoryOverlay {
    pub records: Vec<SessionHistory>,
    pub selected: usize,
    pub search_query: TextInput,
    pub search_active: bool,
}

/// Repository selector overlay state.
#[derive(Default)]
pub struct RepoSelectorOverlay {
    pub selected: usize,
}

/// Open-repository path input overlay state.
#[derive(Default)]
pub struct OpenRepoOverlay {
    pub buffer: TextInput,
}

/// PR-number/URL input overlay state ("Review: Review Pull Request…").
#[derive(Default)]
pub struct PrInputOverlay {
    pub buffer: TextInput,
    /// Set while a background PR intake (gh/git) is running for this overlay.
    pub loading: bool,
    /// Set on a failed intake attempt; cleared on the next Enter/edit. The
    /// overlay stays open and `buffer` is left untouched so the user can
    /// correct and retry.
    pub error: Option<String>,
    pub bg_op: BackgroundOp<crate::pr_intake::PrIntakeOutcome>,
}

/// Code navigation: references overlay state (for `gr` — Find References).
#[derive(Default)]
pub struct ReferencesOverlay {
    pub active: bool,
    pub symbol_name: String,
    pub results: Vec<crate::symbol_index::Reference>,
    pub selected: usize,
    pub scroll: usize,
}

/// A single symbol hint shown during Vimium-style navigation.
#[derive(Debug, Clone)]
pub struct SymbolHint {
    /// 2-character label (e.g. "aa", "ab").
    pub label: String,
    /// The symbol name (e.g. "AppState").
    pub symbol_name: String,
    /// 1-indexed line number.
    pub line: usize,
    /// 0-indexed start column in content.
    pub start_col: usize,
    /// 0-indexed end column (exclusive) in content.
    #[allow(dead_code)]
    pub end_col: usize,
}

/// Vimium-style symbol hint overlay — shown when `g` is pressed in Viewer.
#[derive(Default)]
pub struct SymbolHintOverlay {
    pub active: bool,
    /// All generated hints for visible symbols.
    pub hints: Vec<SymbolHint>,
    /// Characters typed so far for label matching (0-2 chars).
    pub input: String,
}

/// An action available for a selected symbol.
#[derive(Debug, Clone)]
pub struct SymbolAction {
    /// Key to press (e.g. 'd', 'i', 'r').
    pub key: char,
    /// Description (e.g. "Go to definition").
    pub label: String,
    /// Target file path.
    pub file_path: String,
    /// Target line number (1-indexed).
    pub line: usize,
}

/// Action selection modal shown after picking a symbol hint.
#[derive(Default)]
pub struct SymbolActionOverlay {
    pub active: bool,
    pub symbol_name: String,
    pub actions: Vec<SymbolAction>,
    pub selected: usize,
    /// Screen row (0-indexed) of the source symbol, used to preserve vertical
    /// position when jumping.
    pub source_screen_row: usize,
}

/// A symbol the mouse/cursor is resting on, awaiting the idle debounce before
/// its hover popup is resolved. `resolved` flips true once we've attempted the
/// lookup (whether or not it produced a popup) so the per-frame tick doesn't
/// recompute every frame while the cursor sits still.
pub struct HoverCandidate {
    /// Identifier under the cursor/mouse.
    pub symbol: String,
    /// 1-indexed content line the symbol is on.
    pub line: usize,
    /// File the symbol is in, for definition disambiguation.
    pub file: Option<String>,
    /// Absolute screen row of the symbol — the popup anchors just below (or
    /// above, if there's no room) this row.
    pub anchor_row: u16,
    /// Absolute screen column of the symbol's start, for horizontal placement.
    pub anchor_col: u16,
    /// Start column (0-indexed, in content characters before h_scroll) of the
    /// symbol in its source line — carried into `HoverInfoOverlay::target_*`
    /// once resolved, so the popup's target can stay highlighted (A8)
    /// independent of the mouse's current position.
    pub start_col: usize,
    /// End column (exclusive), see `start_col`.
    pub end_col: usize,
    /// When the cursor/mouse came to rest on this symbol.
    pub since: std::time::Instant,
    /// Whether the lookup has already run for this candidate.
    pub resolved: bool,
}

/// A code preview (level 2): a window of source lines around a reference,
/// shown when a row in the references list is clicked.
pub struct HoverPreview {
    /// File the preview is from (repo-relative).
    pub file: String,
    /// 1-indexed reference line the preview is centered on.
    pub center_line: usize,
    /// `(1-indexed line number, text)` for each shown line.
    pub lines: Vec<(usize, String)>,
    /// Rendered rect, written by the renderer for hit-testing.
    pub rect: ratatui::layout::Rect,
}

/// The references list (level 1), opened by clicking `N refs` in the base hover
/// popup. Mouse-first: rows are clickable to open a [`HoverPreview`].
pub struct HoverRefs {
    /// Symbol whose references these are (list title).
    pub symbol: String,
    /// All references found.
    pub results: Vec<crate::symbol_index::Reference>,
    /// Highlighted row (for keyboard nav / preview target).
    pub selected: usize,
    /// First visible row index.
    pub scroll: usize,
    /// Rendered list-popup rect, written by the renderer.
    pub rect: ratatui::layout::Rect,
    /// `(result index, row rect)` for each visible row, written by the renderer.
    pub row_hits: Vec<(usize, ratatui::layout::Rect)>,
    /// The open preview, if a row was clicked.
    pub preview: Option<HoverPreview>,
}

/// Symbol hover-info popup — signature/doc/references for the symbol under the
/// viewer cursor. Shown automatically when the mouse rests on a symbol or the
/// keyboard cursor sits idle; `info` is the resolved popup (`None` = hidden),
/// `pending` is the candidate counting down the idle debounce. `anchor_row`/
/// `anchor_col` are the screen position of the resolved symbol, for placement.
///
/// It can escalate into an interactive modal stack: clicking `N refs` pins the
/// popup and opens [`HoverRefs`]; clicking a row opens a [`HoverPreview`].
/// `pinned` popups survive focus/idle loss until Esc or a click outside;
/// `leave_at` is the short grace window keeping a still-transient popup alive
/// after the mouse leaves the symbol (so the cursor can reach it to click).
#[derive(Default)]
pub struct HoverInfoOverlay {
    pub info: Option<crate::hover_info::HoverInfo>,
    pub pending: Option<HoverCandidate>,
    pub anchor_row: u16,
    pub anchor_col: u16,
    pub pinned: bool,
    pub leave_at: Option<std::time::Instant>,
    /// The viewed file the current `info` was resolved against. When the viewer
    /// switches files underneath a (non-pinned) popup, this no longer matches
    /// `content.current_file`, and the tick drops the now-stale popup.
    pub shown_file: Option<String>,
    /// 1-indexed source line of the symbol `info` describes (A8: lets the
    /// renderer keep that symbol highlighted for as long as the popup is
    /// shown, independent of `ClickTracker::hover_symbol` — the mouse may
    /// have since moved off it, or be sitting in the popup's leave-grace
    /// window, while the underline itself has no such grace).
    pub target_line: usize,
    /// Start column of `target_line`'s highlighted symbol (see `target_line`).
    pub target_start_col: usize,
    /// End column (exclusive) of the highlighted symbol.
    pub target_end_col: usize,
    /// Base popup rect, written by the renderer for hit-testing.
    pub info_rect: ratatui::layout::Rect,
    /// The `N refs` clickable region within the base popup (zero-sized if the
    /// symbol has no references), written by the renderer.
    pub refs_hit: ratatui::layout::Rect,
    pub refs: Option<HoverRefs>,
}

impl HoverInfoOverlay {
    /// Whether any part of the hover modal stack is showing.
    pub fn is_shown(&self) -> bool {
        self.info.is_some()
    }

    /// Reset the whole hover modal stack to hidden/unpinned.
    pub fn reset(&mut self) {
        self.info = None;
        self.pending = None;
        self.pinned = false;
        self.leave_at = None;
        self.shown_file = None;
        self.target_line = 0;
        self.target_start_col = 0;
        self.target_end_col = 0;
        self.refs = None;
        self.info_rect = ratatui::layout::Rect::default();
        self.refs_hit = ratatui::layout::Rect::default();
    }
}

/// Help overlay state.
pub struct HelpOverlay {
    pub context: Focus,
}

impl Default for HelpOverlay {
    fn default() -> Self {
        Self {
            context: Focus::Worktree,
        }
    }
}

/// Container for all overlay state, replacing individual overlay fields on App.
#[derive(Default)]
pub struct OverlayManager {
    /// Which overlay is currently active.
    pub active: ActiveOverlay,
    pub switch_branch: SwitchBranchOverlay,
    pub grab: GrabOverlay,
    pub prune: PruneOverlay,
    pub cherry_pick: CherryPickOverlay,
    pub history: HistoryOverlay,
    pub resume_session: ResumeSessionOverlay,
    pub repo_selector: RepoSelectorOverlay,
    pub open_repo: OpenRepoOverlay,
    pub pr_input: PrInputOverlay,
    pub grep_search: GrepSearchOverlay,
    pub help: HelpOverlay,
    pub command_palette: CommandPaletteOverlay,
    pub theme_picker: ThemePickerOverlay,
}
