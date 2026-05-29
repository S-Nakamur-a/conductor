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
    GrepSearch,
    Help,
    CommandPalette,
    /// Worktree switcher — the modal that replaced the left worktree column.
    /// Reuses the existing worktree list state and `handle_worktree_key`.
    WorktreeSwitcher,
    /// Full-screen comment list — overview of all review comments on the branch
    /// with jump-to-location. Reuses the comment list state + handler.
    CommentList,
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
    pub grep_search: GrepSearchOverlay,
    pub help: HelpOverlay,
    pub command_palette: CommandPaletteOverlay,
}
