//! Command palette data model: the command taxonomy, categories, scope, and
//! the `PaletteCommand`/`ScoredCommand` record types.

use crate::keymap::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    // Navigation
    FocusWorktree,
    FocusExplorer,
    FocusViewer,
    FocusTerminalClaude,
    FocusTerminalShell,
    NextWorktree,
    PrevWorktree,
    TogglePanelExpand,
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,

    // Worktree
    CreateWorktree,
    DeleteWorktree,
    SwitchBranch,
    GrabBranch,
    PruneWorktrees,
    MergeToMain,
    RefreshWorktrees,
    ResetMainToOrigin,
    CherryPick,
    PullWorktree,

    // Terminal
    NewClaudeCode,
    NewShell,
    ResumeClaudeSession,

    // Git
    RefreshDiff,

    // View
    SearchInFile,
    ToggleHelp,

    // Review
    ShowReviewComments,
    ShowReviewTemplates,
    SessionHistory,
    ReviewPullRequest,
    GenerateWalkthrough,
    ForceGenerateWalkthrough,
    PublishReview,

    // Repository
    OpenRepo,
    SwitchRepo,

    // Worktree (additional)
    UngrabBranch,

    // Explorer
    ShowDiffList,
    ShowCommentList,
    ShowWalkthrough,

    // Viewer / Review
    AddReviewComment,
    ViewCommentDetail,

    // Comment actions
    DeleteComment,
    ToggleCommentResolve,
    EditComment,
    ReplyToComment,

    // Session
    SaveSessionHistory,

    // Search
    SearchFullText,

    // GitHub / PR
    OpenPullRequest,

    // App
    UpdateAndRestart,
    CheckForUpdate,
    TogglePartyMode,
    ToggleRichMode,
    Quit,

    // UI
    SwitchTheme,
    ToggleHighContrast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Navigation,
    Worktree,
    Terminal,
    Git,
    View,
    Review,
    Repository,
    App,
}

impl CommandCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Worktree => "Worktree",
            Self::Terminal => "Terminal",
            Self::Git => "Git",
            Self::View => "View",
            Self::Review => "Review",
            Self::Repository => "Repository",
            Self::App => "App",
        }
    }
}

/// Where a command sits relative to the focused panel: bound globally, bound in
/// the current panel's own layer, or bound only in some other panel's layer
/// (still runnable from the palette). Drives the grouped display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandScope {
    Current,
    Global,
    Other,
}

/// Sort rank for grouping filtered results by [`CommandScope`] (current panel
/// first, then global, then other panels). Shared by `search::filter_commands`
/// and its tests.
pub(super) fn scope_rank(scope: CommandScope) -> u8 {
    match scope {
        CommandScope::Current => 0,
        CommandScope::Global => 1,
        CommandScope::Other => 2,
    }
}

pub struct PaletteCommand {
    pub id: CommandId,
    pub label: &'static str,
    pub category: CommandCategory,
    /// The keymap action this command runs, if it has a keybinding. `None` for
    /// palette-only commands (no chord). The displayed shortcut and scope are
    /// derived from this via the keymap.
    pub action: Option<Action>,
    pub keywords: &'static str,
}

pub struct ScoredCommand {
    pub index: usize,
    pub score: i32,
    pub scope: CommandScope,
}
