//! Command palette — fuzzy-searchable command index.
//!
//! Provides a VSCode-style command palette (`Ctrl+P` / `:`) for discovering and
//! executing any application command. Each command carries the keymap [`Action`]
//! it corresponds to (when it has one), so its displayed shortcut and its scope
//! (global vs. the focused panel's layer) are derived live from the keymap and
//! never go stale. Palette-only commands (no keybinding) carry `action: None`.

use crate::keymap::{Action, KeyContext, KeyMap};

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

fn scope_rank(scope: CommandScope) -> u8 {
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

pub const COMMANDS: &[PaletteCommand] = &[
    // Navigation
    PaletteCommand {
        id: CommandId::FocusWorktree,
        label: "Focus: Worktree Panel",
        category: CommandCategory::Navigation,
        action: Some(Action::FocusWorktree),
        keywords: "panel switch",
    },
    PaletteCommand {
        id: CommandId::FocusExplorer,
        label: "Focus: Explorer Panel",
        category: CommandCategory::Navigation,
        action: Some(Action::FocusExplorer),
        keywords: "panel files",
    },
    PaletteCommand {
        id: CommandId::FocusViewer,
        label: "Focus: Viewer Panel",
        category: CommandCategory::Navigation,
        action: Some(Action::FocusViewer),
        keywords: "panel file view",
    },
    PaletteCommand {
        id: CommandId::FocusTerminalClaude,
        label: "Focus: Claude Code Terminal",
        category: CommandCategory::Navigation,
        action: Some(Action::FocusTerminalClaude),
        keywords: "terminal claude",
    },
    PaletteCommand {
        id: CommandId::FocusTerminalShell,
        label: "Focus: Shell Terminal",
        category: CommandCategory::Navigation,
        action: Some(Action::FocusTerminalShell),
        keywords: "terminal shell",
    },
    PaletteCommand {
        id: CommandId::NextWorktree,
        label: "Next Worktree",
        category: CommandCategory::Navigation,
        action: Some(Action::NextWorktree),
        keywords: "worktree switch next cycle tab",
    },
    PaletteCommand {
        id: CommandId::PrevWorktree,
        label: "Previous Worktree",
        category: CommandCategory::Navigation,
        action: Some(Action::PrevWorktree),
        keywords: "worktree switch previous cycle tab",
    },
    PaletteCommand {
        id: CommandId::TogglePanelExpand,
        label: "Toggle Panel Expand",
        category: CommandCategory::Navigation,
        action: Some(Action::TogglePanelExpand),
        keywords: "resize maximize fullscreen",
    },
    PaletteCommand {
        id: CommandId::ResizePaneLeft,
        label: "Layout: Resize Pane Left",
        category: CommandCategory::Navigation,
        action: Some(Action::ResizePaneLeft),
        keywords: "resize pane panel width column shrink grow tmux left",
    },
    PaletteCommand {
        id: CommandId::ResizePaneRight,
        label: "Layout: Resize Pane Right",
        category: CommandCategory::Navigation,
        action: Some(Action::ResizePaneRight),
        keywords: "resize pane panel width column shrink grow tmux right",
    },
    PaletteCommand {
        id: CommandId::ResizePaneUp,
        label: "Layout: Resize Pane Up",
        category: CommandCategory::Navigation,
        action: Some(Action::ResizePaneUp),
        keywords: "resize pane panel height shell claude split shorter taller tmux up",
    },
    PaletteCommand {
        id: CommandId::ResizePaneDown,
        label: "Layout: Resize Pane Down",
        category: CommandCategory::Navigation,
        action: Some(Action::ResizePaneDown),
        keywords: "resize pane panel height shell claude split shorter taller tmux down",
    },
    // Worktree
    PaletteCommand {
        id: CommandId::CreateWorktree,
        label: "Worktree: Create New",
        category: CommandCategory::Worktree,
        action: Some(Action::CreateWorktree),
        keywords: "branch new add",
    },
    PaletteCommand {
        id: CommandId::DeleteWorktree,
        label: "Worktree: Delete Selected",
        category: CommandCategory::Worktree,
        action: Some(Action::DeleteWorktree),
        keywords: "remove branch",
    },
    PaletteCommand {
        id: CommandId::SwitchBranch,
        label: "Worktree: Switch Branch (Remote)",
        category: CommandCategory::Worktree,
        action: Some(Action::SwitchBranch),
        keywords: "checkout remote",
    },
    PaletteCommand {
        id: CommandId::GrabBranch,
        label: "Worktree: Grab Branch",
        category: CommandCategory::Worktree,
        action: Some(Action::GrabBranch),
        keywords: "grab checkout branch",
    },
    PaletteCommand {
        id: CommandId::PruneWorktrees,
        label: "Worktree: Prune Stale",
        category: CommandCategory::Worktree,
        action: Some(Action::PruneWorktrees),
        keywords: "clean stale",
    },
    PaletteCommand {
        id: CommandId::MergeToMain,
        label: "Worktree: Merge into Main",
        category: CommandCategory::Worktree,
        action: Some(Action::MergeToMain),
        keywords: "merge main",
    },
    PaletteCommand {
        id: CommandId::RefreshWorktrees,
        label: "Worktree: Refresh List",
        category: CommandCategory::Worktree,
        action: Some(Action::RefreshWorktrees),
        keywords: "reload update",
    },
    PaletteCommand {
        id: CommandId::ResetMainToOrigin,
        label: "Worktree: Reset Main to Origin",
        category: CommandCategory::Worktree,
        action: Some(Action::ResetMainToOrigin),
        keywords: "reset origin",
    },
    PaletteCommand {
        id: CommandId::CherryPick,
        label: "Worktree: Cherry-pick",
        category: CommandCategory::Worktree,
        action: Some(Action::CherryPick),
        keywords: "cherry pick commit",
    },
    PaletteCommand {
        id: CommandId::PullWorktree,
        label: "Worktree: Pull (fast-forward)",
        category: CommandCategory::Worktree,
        action: Some(Action::PullWorktree),
        keywords: "pull fetch update fast-forward ff sync",
    },
    // Worktree (additional)
    PaletteCommand {
        id: CommandId::UngrabBranch,
        label: "Worktree: Ungrab Branch",
        category: CommandCategory::Worktree,
        action: Some(Action::UngrabBranch),
        keywords: "ungrab release branch",
    },
    // Terminal
    PaletteCommand {
        id: CommandId::NewClaudeCode,
        label: "Terminal: New Claude Code",
        category: CommandCategory::Terminal,
        action: Some(Action::NewClaudeCode),
        keywords: "spawn ai",
    },
    PaletteCommand {
        id: CommandId::NewShell,
        label: "Terminal: New Shell",
        category: CommandCategory::Terminal,
        action: Some(Action::NewShell),
        keywords: "spawn bash zsh",
    },
    PaletteCommand {
        id: CommandId::ResumeClaudeSession,
        label: "Terminal: Resume Claude Session",
        category: CommandCategory::Terminal,
        action: None,
        keywords: "resume continue",
    },
    // Git
    PaletteCommand {
        id: CommandId::RefreshDiff,
        label: "Diff: Refresh",
        category: CommandCategory::Git,
        action: None,
        keywords: "reload diff",
    },
    // View
    PaletteCommand {
        id: CommandId::SearchInFile,
        label: "Search in File",
        category: CommandCategory::View,
        action: Some(Action::SearchInFile),
        keywords: "find grep",
    },
    PaletteCommand {
        id: CommandId::SearchFullText,
        label: "Search: Full-text Search (Grep)",
        category: CommandCategory::View,
        action: Some(Action::SearchFullText),
        keywords: "grep search find text content regex ripgrep fulltext",
    },
    PaletteCommand {
        id: CommandId::ToggleHelp,
        label: "Show Help",
        category: CommandCategory::View,
        action: Some(Action::ShowHelp),
        keywords: "keybindings shortcuts",
    },
    PaletteCommand {
        id: CommandId::ShowDiffList,
        label: "Explorer: Show Diff List",
        category: CommandCategory::View,
        action: Some(Action::ShowDiffList),
        keywords: "diff changed files",
    },
    PaletteCommand {
        id: CommandId::ShowCommentList,
        label: "Explorer: Show Comment List",
        category: CommandCategory::View,
        action: Some(Action::ShowCommentList),
        keywords: "comment review list",
    },
    // Review
    PaletteCommand {
        id: CommandId::ShowReviewComments,
        label: "Review: Show Comments",
        category: CommandCategory::Review,
        action: None,
        keywords: "comment list",
    },
    PaletteCommand {
        id: CommandId::ShowReviewTemplates,
        label: "Review: Show Templates",
        category: CommandCategory::Review,
        action: None,
        keywords: "template prompt",
    },
    PaletteCommand {
        id: CommandId::SessionHistory,
        label: "Review: Session History",
        category: CommandCategory::Review,
        action: Some(Action::SessionHistory),
        keywords: "history log",
    },
    PaletteCommand {
        id: CommandId::AddReviewComment,
        label: "Review: Add Comment",
        category: CommandCategory::Review,
        action: Some(Action::AddComment),
        keywords: "new comment add write",
    },
    PaletteCommand {
        id: CommandId::ViewCommentDetail,
        label: "Review: View Comment Detail",
        category: CommandCategory::Review,
        action: Some(Action::ViewCommentDetail),
        keywords: "detail preview",
    },
    PaletteCommand {
        id: CommandId::DeleteComment,
        label: "Review: Delete Comment",
        category: CommandCategory::Review,
        action: Some(Action::DeleteComment),
        keywords: "remove delete",
    },
    PaletteCommand {
        id: CommandId::ToggleCommentResolve,
        label: "Review: Toggle Resolve",
        category: CommandCategory::Review,
        action: Some(Action::ToggleResolve),
        keywords: "resolve unresolve status",
    },
    PaletteCommand {
        id: CommandId::EditComment,
        label: "Review: Edit Comment",
        category: CommandCategory::Review,
        action: Some(Action::EditComment),
        keywords: "edit modify update",
    },
    PaletteCommand {
        id: CommandId::ReplyToComment,
        label: "Review: Reply to Comment",
        category: CommandCategory::Review,
        action: Some(Action::ReplyToComment),
        keywords: "reply respond",
    },
    PaletteCommand {
        id: CommandId::SaveSessionHistory,
        label: "Session: Save History",
        category: CommandCategory::Review,
        action: None,
        keywords: "save record session",
    },
    // Repository
    PaletteCommand {
        id: CommandId::OpenRepo,
        label: "Repository: Open by Path",
        category: CommandCategory::Repository,
        action: Some(Action::OpenRepo),
        keywords: "open directory",
    },
    PaletteCommand {
        id: CommandId::SwitchRepo,
        label: "Repository: Switch",
        category: CommandCategory::Repository,
        action: Some(Action::SwitchRepo),
        keywords: "project change",
    },
    // GitHub / PR
    PaletteCommand {
        id: CommandId::OpenPullRequest,
        label: "Worktree: Open Pull Request",
        category: CommandCategory::Worktree,
        action: Some(Action::OpenPullRequest),
        keywords: "pr github browser web open",
    },
    // App
    PaletteCommand {
        id: CommandId::CheckForUpdate,
        label: "App: Check for Updates",
        category: CommandCategory::App,
        action: None,
        keywords: "update upgrade version check latest release new github",
    },
    PaletteCommand {
        id: CommandId::UpdateAndRestart,
        label: "App: Update and Restart",
        category: CommandCategory::App,
        action: None,
        keywords: "update upgrade restart download version",
    },
    PaletteCommand {
        id: CommandId::TogglePartyMode,
        label: "🎉 Party Mode (secret)",
        category: CommandCategory::App,
        action: None,
        keywords: "party rainbow fun secret celebration mode hidden festive disco",
    },
    PaletteCommand {
        id: CommandId::ToggleRichMode,
        label: "✨ Toggle Rich Mode",
        category: CommandCategory::App,
        action: None,
        keywords: "rich mode graphics gradient border glow visual effects truecolor",
    },
    PaletteCommand {
        id: CommandId::Quit,
        label: "Quit Conductor",
        category: CommandCategory::App,
        action: Some(Action::Quit),
        keywords: "exit close",
    },
    // UI
    PaletteCommand {
        id: CommandId::SwitchTheme,
        label: "Switch Theme",
        category: CommandCategory::View,
        action: Some(Action::OpenThemePicker),
        keywords: "theme color light dark appearance palette catppuccin solarized github",
    },
    PaletteCommand {
        id: CommandId::ToggleHighContrast,
        label: "UI: Toggle High Contrast",
        category: CommandCategory::View,
        action: None,
        keywords: "high contrast accessibility a11y legibility bright bold theme readable vision",
    },
];

pub struct ScoredCommand {
    pub index: usize,
    pub score: i32,
    pub scope: CommandScope,
}

/// Classify a command relative to the focused panel. Global-bound actions are
/// "global" even if a panel layer also binds them (e.g. `:` for the palette);
/// otherwise an action bound in the current panel's own layer is "current", and
/// anything else (bound only in another panel, runnable here via the palette) is
/// "other". Palette-only commands count as global.
fn command_scope(cmd: &PaletteCommand, keymap: &KeyMap, current: KeyContext) -> CommandScope {
    match cmd.action {
        None => CommandScope::Global,
        Some(action) => {
            if !keymap.keys_in_layer(KeyContext::Global, action).is_empty() {
                CommandScope::Global
            } else if current != KeyContext::Global
                && !keymap.keys_in_layer(current, action).is_empty()
            {
                CommandScope::Current
            } else {
                CommandScope::Other
            }
        }
    }
}

/// Fuzzy score for a command against a lowercased query; `None` if no match.
fn score_command(cmd: &PaletteCommand, query_lower: &str) -> Option<i32> {
    let label_lower = cmd.label.to_lowercase();
    let keywords_lower = cmd.keywords.to_lowercase();
    let category_lower = cmd.category.label().to_lowercase();
    let haystack = format!("{label_lower} {keywords_lower} {category_lower}");

    if !haystack.contains(query_lower) {
        return None;
    }

    let mut score: i32 = 0;
    if label_lower.starts_with(query_lower) {
        score += 100;
    }
    for word in label_lower.split(|c: char| !c.is_alphanumeric()) {
        if word.starts_with(query_lower) {
            score += 50;
            break;
        }
    }
    if label_lower.contains(query_lower) {
        score += 20;
    }
    if keywords_lower.contains(query_lower) {
        score += 10;
    }
    if category_lower.contains(query_lower) {
        score += 5;
    }
    Some(score)
}

/// Filter and score commands against a query, grouped by scope relative to the
/// focused panel (`current`). Returns all commands (sorted by scope) when the
/// query is empty, or matching commands sorted by scope then relevance.
///
/// The ordering is shared by the renderer (for grouped display) and the key
/// handler (for selection + execution), so `selected` indexes into this exact
/// sequence.
pub fn filter_commands(query: &str, keymap: &KeyMap, current: KeyContext) -> Vec<ScoredCommand> {
    let query_lower = query.to_lowercase();

    let mut results: Vec<ScoredCommand> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, cmd)| {
            let score = if query.is_empty() {
                0
            } else {
                score_command(cmd, &query_lower)?
            };
            Some(ScoredCommand {
                index: i,
                score,
                scope: command_scope(cmd, keymap, current),
            })
        })
        .collect();

    results.sort_by(|a, b| {
        scope_rank(a.scope)
            .cmp(&scope_rank(b.scope))
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.index.cmp(&b.index))
    });
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap() -> KeyMap {
        KeyMap::new(&toml::Table::new())
    }

    #[test]
    fn every_command_action_is_valid() {
        // A Some(action) must round-trip through the action vocabulary, so a
        // palette entry can never point at a stale/renamed action.
        for cmd in COMMANDS {
            if let Some(action) = cmd.action {
                assert_eq!(
                    Action::from_str(action.as_str()),
                    Some(action),
                    "command {:?} has an unrecognized action",
                    cmd.id
                );
            }
        }
    }

    #[test]
    fn comprehensive_worktree_commands_present() {
        // Guards against silent omissions of high-value worktree commands —
        // `pull_worktree` was previously missing entirely.
        let must_have = [
            Action::CreateWorktree,
            Action::DeleteWorktree,
            Action::SwitchBranch,
            Action::GrabBranch,
            Action::UngrabBranch,
            Action::PruneWorktrees,
            Action::MergeToMain,
            Action::PullWorktree,
            Action::CherryPick,
            Action::OpenPullRequest,
        ];
        for action in must_have {
            assert!(
                COMMANDS.iter().any(|c| c.action == Some(action)),
                "missing palette command for {action:?}"
            );
        }
    }

    #[test]
    fn scope_splits_global_from_current_layer() {
        let km = keymap();
        // Focused on the worktree panel: create-worktree is a worktree-layer
        // action → Current; quit is global → Global.
        let scoped = filter_commands("", &km, KeyContext::Worktree);
        let scope_of = |action: Action| {
            scoped
                .iter()
                .find(|s| COMMANDS[s.index].action == Some(action))
                .map(|s| s.scope)
        };
        assert_eq!(scope_of(Action::CreateWorktree), Some(CommandScope::Current));
        assert_eq!(scope_of(Action::Quit), Some(CommandScope::Global));
        // A viewer-only action is neither global nor in the worktree layer.
        assert_eq!(scope_of(Action::SearchInFile), Some(CommandScope::Other));
    }

    #[test]
    fn results_are_grouped_current_then_global_then_other() {
        let km = keymap();
        let scoped = filter_commands("", &km, KeyContext::Worktree);
        let ranks: Vec<u8> = scoped.iter().map(|s| scope_rank(s.scope)).collect();
        assert!(
            ranks.windows(2).all(|w| w[0] <= w[1]),
            "scopes must be contiguous/ordered: {ranks:?}"
        );
    }
}
