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
use crate::grep_search::{GrepMatch, GrepProgress};
use crate::review_store::SessionHistory;
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
    pub results: Vec<GrepMatch>,
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
