//! The static menu table — which commands appear under which top-level menu,
//! in which order, with which separators.
//!
//! This table holds *taxonomy and presentation only*. Every actionable entry is
//! a [`CommandId`], and running it goes through
//! [`App::execute_palette_command`](crate::app::App::execute_palette_command) —
//! the same funnel the command palette and the keyboard shortcuts use. There is
//! no second copy of any command's behaviour here, and adding a menu entry can
//! never change what a command does.
//!
//! The short `label` on each item is menu-local on purpose. The palette labels
//! are self-describing because the palette is a flat list ("Worktree: Create
//! New"); inside a menu the top-level title already supplies that context, so
//! repeating it would read as "Worktree ▸ Worktree: Create New". Only the
//! display string is local — the `id` is what actually runs.
//!
//! `tests::every_command_is_reachable` holds the completeness promise: every
//! `CommandId` must appear in exactly one menu or be listed in
//! [`INTENTIONALLY_UNLISTED`] with a reason.

use crate::command_palette::CommandId;

/// One row of a dropdown: either an invocable command or a horizontal rule.
pub enum MenuItem {
    /// An invocable command. `label` is the text shown in the dropdown; `id` is
    /// what gets executed.
    Command {
        id: CommandId,
        label: &'static str,
    },
    /// A non-selectable divider between groups of related commands.
    Separator,
}

impl MenuItem {
    /// The command this row runs, or `None` for a [`MenuItem::Separator`].
    pub fn command(&self) -> Option<CommandId> {
        match self {
            MenuItem::Command { id, .. } => Some(*id),
            MenuItem::Separator => None,
        }
    }

    /// Whether this row can hold the selection. Separators are skipped by
    /// keyboard navigation and are not clickable.
    pub fn is_selectable(&self) -> bool {
        matches!(self, MenuItem::Command { .. })
    }
}

/// One top-level menu and the dropdown it opens.
pub struct Menu {
    /// The word shown on the menu bar itself.
    pub title: &'static str,
    pub items: &'static [MenuItem],
}

/// Shorthand for a command row.
const fn cmd(id: CommandId, label: &'static str) -> MenuItem {
    MenuItem::Command { id, label }
}

/// A divider row.
const SEP: MenuItem = MenuItem::Separator;

/// Commands deliberately absent from the menu bar, each with the reason. The
/// completeness test reads this list, so removing an entry here without adding
/// the command to a menu fails the build.
///
/// Note this covers only [`CommandId`]s. The keymap also has per-panel cursor
/// motions (`NavigateUp`, `GoToTop`, `NextHunk`, …) which are `Action`s without
/// a `CommandId` — they are modal cursor movement, not operations, and are
/// meaningless as a menu row. They are absent from the palette for the same
/// reason.
// Read by `tests::every_command_is_reachable`, which is the point of it: this
// list is the written record of what the menu deliberately omits, and the test
// is what stops the record from silently going stale.
#[allow(dead_code)]
pub const INTENTIONALLY_UNLISTED: &[(CommandId, &str)] = &[(
    CommandId::TogglePartyMode,
    "Labelled '(secret)' in the palette — listing it on the menu bar would \
     defeat the point. Still reachable from the command palette.",
)];

/// The menu bar, left to right.
pub const MENUS: &[Menu] = &[
    Menu {
        title: "Repo",
        items: &[
            cmd(CommandId::OpenRepo, "Open Repository…"),
            cmd(CommandId::SwitchRepo, "Switch Repository…"),
            SEP,
            cmd(CommandId::RefreshDiff, "Refresh Diff"),
            SEP,
            cmd(CommandId::Quit, "Quit Conductor"),
        ],
    },
    Menu {
        title: "Worktree",
        items: &[
            cmd(CommandId::CreateWorktree, "New Worktree…"),
            cmd(CommandId::DeleteWorktree, "Delete Worktree…"),
            SEP,
            cmd(CommandId::NextWorktree, "Next Worktree"),
            cmd(CommandId::PrevWorktree, "Previous Worktree"),
            SEP,
            cmd(CommandId::SwitchBranch, "Switch Branch (Remote)…"),
            cmd(CommandId::GrabBranch, "Grab Branch…"),
            cmd(CommandId::UngrabBranch, "Ungrab Branch"),
            SEP,
            cmd(CommandId::PullWorktree, "Pull (fast-forward)"),
            cmd(CommandId::MergeToMain, "Merge into Main"),
            cmd(CommandId::CherryPick, "Cherry-pick…"),
            cmd(CommandId::ResetMainToOrigin, "Reset Main to Origin"),
            SEP,
            cmd(CommandId::PruneWorktrees, "Prune Stale Worktrees"),
            cmd(CommandId::RefreshWorktrees, "Refresh Worktree List"),
            SEP,
            cmd(CommandId::OpenPullRequest, "Open Pull Request in Browser"),
        ],
    },
    Menu {
        title: "Review",
        items: &[
            cmd(CommandId::AddReviewComment, "Add Comment"),
            cmd(CommandId::EditComment, "Edit Comment"),
            cmd(CommandId::ReplyToComment, "Reply to Comment"),
            cmd(CommandId::DeleteComment, "Delete Comment"),
            cmd(CommandId::ToggleCommentResolve, "Toggle Resolved"),
            cmd(CommandId::ViewCommentDetail, "View Comment Detail"),
            SEP,
            cmd(CommandId::ShowReviewComments, "Show Comments"),
            cmd(CommandId::ShowReviewTemplates, "Show Templates"),
            SEP,
            cmd(CommandId::ReviewPullRequest, "Review Pull Request…"),
            // The walkthrough rows sit together under Review rather than with
            // the other Explorer bottom-pane switches under View: reading a
            // walkthrough is a review activity, and having "show" next to
            // "generate" is what you want when there isn't one yet.
            cmd(CommandId::ShowWalkthrough, "Show Walkthrough"),
            cmd(CommandId::GenerateWalkthrough, "Generate Walkthrough"),
            cmd(
                CommandId::ForceGenerateWalkthrough,
                "Regenerate Walkthrough (force)",
            ),
            SEP,
            cmd(CommandId::PublishReview, "Publish Comments to GitHub…"),
            SEP,
            cmd(CommandId::SessionHistory, "Session History"),
            cmd(CommandId::SaveSessionHistory, "Save Session History"),
        ],
    },
    Menu {
        title: "View",
        items: &[
            cmd(CommandId::ShowDiffList, "Changed Files"),
            cmd(CommandId::ShowCommentList, "Comment List"),
            SEP,
            cmd(CommandId::ToggleMarkdownRender, "Markdown: Raw / Rendered"),
            SEP,
            cmd(CommandId::SwitchTheme, "Switch Theme…"),
            cmd(CommandId::ToggleHighContrast, "Toggle High Contrast"),
            cmd(CommandId::ToggleRichMode, "Toggle Rich Mode"),
        ],
    },
    Menu {
        title: "Panel",
        items: &[
            cmd(CommandId::FocusWorktree, "Focus Worktree"),
            cmd(CommandId::FocusExplorer, "Focus Explorer"),
            cmd(CommandId::FocusViewer, "Focus Viewer"),
            cmd(CommandId::FocusTerminalClaude, "Focus Claude Code"),
            cmd(CommandId::FocusTerminalShell, "Focus Shell"),
            SEP,
            cmd(CommandId::TogglePanelExpand, "Maximize / Restore Panel"),
            SEP,
            cmd(CommandId::ResizePaneLeft, "Resize Pane Left"),
            cmd(CommandId::ResizePaneRight, "Resize Pane Right"),
            cmd(CommandId::ResizePaneUp, "Resize Pane Up"),
            cmd(CommandId::ResizePaneDown, "Resize Pane Down"),
        ],
    },
    Menu {
        title: "Search",
        items: &[
            cmd(CommandId::SearchInFile, "Search in File…"),
            cmd(CommandId::SearchFullText, "Full-text Search (Grep)…"),
        ],
    },
    Menu {
        title: "Terminal",
        items: &[
            cmd(CommandId::NewClaudeCode, "New Claude Code Session"),
            cmd(CommandId::NewShell, "New Shell Session"),
            cmd(CommandId::ResumeClaudeSession, "Resume Claude Session…"),
        ],
    },
    Menu {
        title: "Help",
        items: &[
            cmd(CommandId::ToggleHelp, "Keyboard Shortcuts"),
            SEP,
            cmd(CommandId::CheckForUpdate, "Check for Updates"),
            cmd(CommandId::UpdateAndRestart, "Update and Restart"),
        ],
    },
];
