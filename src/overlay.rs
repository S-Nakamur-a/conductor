//! Overlay state types.
//!
//! Each overlay popup has its own state struct, extracted from the monolithic
//! `App` struct to improve organization and reduce field count.

use crate::app::Focus;
use crate::background::BackgroundOp;
use crate::claude_sessions::ResumableSession;
use crate::git_engine::CommitInfo;
use crate::grep_search::{GrepMatch, GrepProgress};
use crate::review_store::SessionHistory;
use crate::text_input::TextInput;

/// Switch-branch overlay state.
#[derive(Default)]
pub struct SwitchBranchOverlay {
    pub active: bool,
    pub branches: Vec<String>,
    pub selected: usize,
    pub filter: TextInput,
}


/// Grab-branch overlay state.
#[derive(Default)]
pub struct GrabOverlay {
    pub active: bool,
    pub branches: Vec<String>,
    pub selected: usize,
}


/// Cherry-pick overlay state.
#[derive(Default)]
pub struct CherryPickOverlay {
    pub active: bool,
    pub source_branch: String,
    pub commits: Vec<CommitInfo>,
    pub selected: usize,
}


/// Prune overlay state.
#[derive(Default)]
pub struct PruneOverlay {
    pub active: bool,
    pub stale: Vec<String>,
}


/// Resume-session overlay state.
#[derive(Default)]
pub struct ResumeSessionOverlay {
    pub active: bool,
    pub sessions: Vec<ResumableSession>,
    pub selected: usize,
    pub filter: TextInput,
    pub all_projects: bool,
}


/// Grep full-text search overlay state.
#[derive(Default)]
pub struct GrepSearchOverlay {
    pub active: bool,
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
    pub active: bool,
    pub filter: TextInput,
    pub selected: usize,
}


/// Session history overlay state.
#[derive(Default)]
pub struct HistoryOverlay {
    pub active: bool,
    pub records: Vec<SessionHistory>,
    pub selected: usize,
    pub search_query: TextInput,
    pub search_active: bool,
}


/// Repository selector overlay state.
#[derive(Default)]
pub struct RepoSelectorOverlay {
    pub active: bool,
    pub selected: usize,
}


/// Open-repository path input overlay state.
#[derive(Default)]
pub struct OpenRepoOverlay {
    pub active: bool,
    pub buffer: TextInput,
}


/// Help overlay state.
pub struct HelpOverlay {
    pub active: bool,
    pub context: Focus,
}

impl Default for HelpOverlay {
    fn default() -> Self {
        Self {
            active: false,
            context: Focus::Worktree,
        }
    }
}
