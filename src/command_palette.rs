//! Command palette — fuzzy-searchable command index.
//!
//! Provides a VSCode-style command palette (`Ctrl+Shift+P` / `:`) for
//! discovering and executing any application command.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    // Navigation
    FocusWorktree,
    FocusExplorer,
    FocusViewer,
    FocusTerminalClaude,
    FocusTerminalShell,
    TogglePanelExpand,

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

    // Repository
    OpenRepo,
    SwitchRepo,

    // Worktree (additional)
    UngrabBranch,

    // Explorer
    ShowDiffList,
    ShowCommentList,

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

    // GitHub / PR
    OpenPullRequest,

    // App
    UpdateAndRestart,
    Quit,
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

pub struct PaletteCommand {
    pub id: CommandId,
    pub label: &'static str,
    pub category: CommandCategory,
    pub keybinding: Option<&'static str>,
    pub keywords: &'static str,
}

pub const COMMANDS: &[PaletteCommand] = &[
    // Navigation
    PaletteCommand { id: CommandId::FocusWorktree, label: "Focus: Worktree Panel",
        category: CommandCategory::Navigation, keybinding: Some("Tab"), keywords: "panel switch" },
    PaletteCommand { id: CommandId::FocusExplorer, label: "Focus: Explorer Panel",
        category: CommandCategory::Navigation, keybinding: Some("Tab"), keywords: "panel files" },
    PaletteCommand { id: CommandId::FocusViewer, label: "Focus: Viewer Panel",
        category: CommandCategory::Navigation, keybinding: Some("Tab"), keywords: "panel file view" },
    PaletteCommand { id: CommandId::FocusTerminalClaude, label: "Focus: Claude Code Terminal",
        category: CommandCategory::Navigation, keybinding: Some("Tab"), keywords: "terminal claude" },
    PaletteCommand { id: CommandId::FocusTerminalShell, label: "Focus: Shell Terminal",
        category: CommandCategory::Navigation, keybinding: Some("Tab"), keywords: "terminal shell" },
    PaletteCommand { id: CommandId::TogglePanelExpand, label: "Toggle Panel Expand",
        category: CommandCategory::Navigation, keybinding: None, keywords: "resize maximize fullscreen" },

    // Worktree
    PaletteCommand { id: CommandId::CreateWorktree, label: "Worktree: Create New",
        category: CommandCategory::Worktree, keybinding: Some("w"), keywords: "branch new add" },
    PaletteCommand { id: CommandId::DeleteWorktree, label: "Worktree: Delete Selected",
        category: CommandCategory::Worktree, keybinding: Some("X"), keywords: "remove branch" },
    PaletteCommand { id: CommandId::SwitchBranch, label: "Worktree: Switch Branch (Remote)",
        category: CommandCategory::Worktree, keybinding: Some("s"), keywords: "checkout remote" },
    PaletteCommand { id: CommandId::GrabBranch, label: "Worktree: Grab Branch",
        category: CommandCategory::Worktree, keybinding: Some("g"), keywords: "grab checkout branch" },
    PaletteCommand { id: CommandId::PruneWorktrees, label: "Worktree: Prune Stale",
        category: CommandCategory::Worktree, keybinding: Some("P"), keywords: "clean stale" },
    PaletteCommand { id: CommandId::MergeToMain, label: "Worktree: Merge into Main",
        category: CommandCategory::Worktree, keybinding: Some("m"), keywords: "merge main" },
    PaletteCommand { id: CommandId::RefreshWorktrees, label: "Worktree: Refresh List",
        category: CommandCategory::Worktree, keybinding: Some("r"), keywords: "reload update" },
    PaletteCommand { id: CommandId::ResetMainToOrigin, label: "Worktree: Reset Main to Origin",
        category: CommandCategory::Worktree, keybinding: Some("R"), keywords: "reset origin" },
    PaletteCommand { id: CommandId::CherryPick, label: "Worktree: Cherry-pick",
        category: CommandCategory::Worktree, keybinding: Some("p"), keywords: "cherry pick commit" },

    // Worktree (additional)
    PaletteCommand { id: CommandId::UngrabBranch, label: "Worktree: Ungrab Branch",
        category: CommandCategory::Worktree, keybinding: Some("G"), keywords: "ungrab release branch" },

    // Terminal
    PaletteCommand { id: CommandId::NewClaudeCode, label: "Terminal: New Claude Code",
        category: CommandCategory::Terminal, keybinding: Some("Ctrl+n"), keywords: "spawn ai" },
    PaletteCommand { id: CommandId::NewShell, label: "Terminal: New Shell",
        category: CommandCategory::Terminal, keybinding: Some("Ctrl+t"), keywords: "spawn bash zsh" },
    PaletteCommand { id: CommandId::ResumeClaudeSession, label: "Terminal: Resume Claude Session",
        category: CommandCategory::Terminal, keybinding: None, keywords: "resume continue" },

    // Git
    PaletteCommand { id: CommandId::RefreshDiff, label: "Diff: Refresh",
        category: CommandCategory::Git, keybinding: None, keywords: "reload diff" },

    // View
    PaletteCommand { id: CommandId::SearchInFile, label: "Search in File",
        category: CommandCategory::View, keybinding: Some("/"), keywords: "find grep" },
    PaletteCommand { id: CommandId::ToggleHelp, label: "Show Help",
        category: CommandCategory::View, keybinding: Some("?"), keywords: "keybindings shortcuts" },
    PaletteCommand { id: CommandId::ShowDiffList, label: "Explorer: Show Diff List",
        category: CommandCategory::View, keybinding: Some("d"), keywords: "diff changed files" },
    PaletteCommand { id: CommandId::ShowCommentList, label: "Explorer: Show Comment List",
        category: CommandCategory::View, keybinding: Some("c"), keywords: "comment review list" },

    // Review
    PaletteCommand { id: CommandId::ShowReviewComments, label: "Review: Show Comments",
        category: CommandCategory::Review, keybinding: Some("c"), keywords: "comment list" },
    PaletteCommand { id: CommandId::ShowReviewTemplates, label: "Review: Show Templates",
        category: CommandCategory::Review, keybinding: None, keywords: "template prompt" },
    PaletteCommand { id: CommandId::SessionHistory, label: "Review: Session History",
        category: CommandCategory::Review, keybinding: Some("H"), keywords: "history log" },
    PaletteCommand { id: CommandId::AddReviewComment, label: "Review: Add Comment",
        category: CommandCategory::Review, keybinding: Some("c"), keywords: "new comment add write" },
    PaletteCommand { id: CommandId::ViewCommentDetail, label: "Review: View Comment Detail",
        category: CommandCategory::Review, keybinding: Some("Space"), keywords: "detail preview" },
    PaletteCommand { id: CommandId::DeleteComment, label: "Review: Delete Comment",
        category: CommandCategory::Review, keybinding: Some("Del"), keywords: "remove delete" },
    PaletteCommand { id: CommandId::ToggleCommentResolve, label: "Review: Toggle Resolve",
        category: CommandCategory::Review, keybinding: Some("r"), keywords: "resolve unresolve status" },
    PaletteCommand { id: CommandId::EditComment, label: "Review: Edit Comment",
        category: CommandCategory::Review, keybinding: Some("e"), keywords: "edit modify update" },
    PaletteCommand { id: CommandId::ReplyToComment, label: "Review: Reply to Comment",
        category: CommandCategory::Review, keybinding: Some("R"), keywords: "reply respond" },
    PaletteCommand { id: CommandId::SaveSessionHistory, label: "Session: Save History",
        category: CommandCategory::Review, keybinding: Some("s"), keywords: "save record session" },

    // Repository
    PaletteCommand { id: CommandId::OpenRepo, label: "Repository: Open by Path",
        category: CommandCategory::Repository, keybinding: Some("Ctrl+o"), keywords: "open directory" },
    PaletteCommand { id: CommandId::SwitchRepo, label: "Repository: Switch",
        category: CommandCategory::Repository, keybinding: Some("Ctrl+r"), keywords: "project change" },

    // GitHub / PR
    PaletteCommand { id: CommandId::OpenPullRequest, label: "Worktree: Open Pull Request",
        category: CommandCategory::Worktree, keybinding: Some("v"), keywords: "pr github browser web open" },

    // App
    PaletteCommand { id: CommandId::UpdateAndRestart, label: "App: Update and Restart",
        category: CommandCategory::App, keybinding: None, keywords: "update upgrade restart download version" },
    PaletteCommand { id: CommandId::Quit, label: "Quit Conductor",
        category: CommandCategory::App, keybinding: Some("q"), keywords: "exit close" },
];

pub struct ScoredCommand {
    pub index: usize,
    pub score: i32,
}

/// Filter and score commands against a query string.
///
/// Returns all commands (unscored) when `query` is empty, or only matching
/// commands sorted by relevance score when a query is provided.
pub fn filter_commands(query: &str) -> Vec<ScoredCommand> {
    if query.is_empty() {
        return COMMANDS
            .iter()
            .enumerate()
            .map(|(i, _)| ScoredCommand { index: i, score: 0 })
            .collect();
    }

    let query_lower = query.to_lowercase();
    let mut results: Vec<ScoredCommand> = Vec::new();

    for (i, cmd) in COMMANDS.iter().enumerate() {
        let label_lower = cmd.label.to_lowercase();
        let keywords_lower = cmd.keywords.to_lowercase();
        let category_lower = cmd.category.label().to_lowercase();
        let haystack = format!("{label_lower} {keywords_lower} {category_lower}");

        if !haystack.contains(&query_lower) {
            continue;
        }

        let mut score: i32 = 0;

        // Exact prefix match on label.
        if label_lower.starts_with(&query_lower) {
            score += 100;
        }
        // Word-boundary match.
        for word in label_lower.split(|c: char| !c.is_alphanumeric()) {
            if word.starts_with(&query_lower) {
                score += 50;
                break;
            }
        }
        // Substring in label.
        if label_lower.contains(&query_lower) {
            score += 20;
        }
        // Match in keywords.
        if keywords_lower.contains(&query_lower) {
            score += 10;
        }
        // Match in category.
        if category_lower.contains(&query_lower) {
            score += 5;
        }

        results.push(ScoredCommand { index: i, score });
    }

    results.sort_by(|a, b| b.score.cmp(&a.score));
    results
}
