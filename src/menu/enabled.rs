//! Whether a command can run right now — what drives the greyed-out rows.
//!
//! **The default is enabled.** A case is written here only where the command
//! already refuses itself somewhere concrete, and each one names that place.
//! The asymmetry is deliberate: a wrong `false` silently removes a working
//! operation from the one UI that lists everything, while a wrong `true` costs
//! nothing beyond today's behaviour — the command runs and reports its own
//! outcome in the status bar, exactly as it does from the palette.
//!
//! So the rule for adding a case is: point at the existing check being
//! mirrored. Do not invent a precondition from what a command's name suggests.
//!
//! Where a command's real precondition needs I/O — a SQLite read, a `git`
//! call — only the cheap part is mirrored. This runs for every visible row of
//! an open dropdown on every frame, and the app redraws at 60fps, so a
//! predicate that queried the review DB would put a database round trip in the
//! render loop. Erring toward "enabled" is the safe side of that trade: the
//! command still explains itself when it can't proceed.

use crate::app::App;
use crate::command_palette::CommandId;

/// Whether `id` can run against the current app state.
///
/// Must stay allocation-light and side-effect free — see the module note on
/// where this is called from.
pub fn command_enabled(id: CommandId, app: &App) -> bool {
    let selected_worktree = app.worktrees.get(app.selected_worktree);

    match id {
        // ── App ──────────────────────────────────────────────────────────
        // `Action::UpdateAndRestart` is a no-op unless a release was found
        // (`event/global.rs`, `if app.update_info.is_some()`).
        CommandId::UpdateAndRestart => app.update_info.is_some(),

        // ── Repository ───────────────────────────────────────────────────
        // The repo selector only opens with somewhere to switch to
        // (`event/global.rs`, `if app.repo_list.len() > 1`).
        CommandId::SwitchRepo => app.repo_list.len() > 1,

        // ── Worktree ─────────────────────────────────────────────────────
        // The strip's delete button refuses the main worktree and one already
        // being torn down (`event/mouse/bars.rs`).
        CommandId::DeleteWorktree => selected_worktree
            .is_some_and(|w| !w.is_main && !app.is_worktree_pending_delete(&w.path)),

        // "Cannot merge main into itself." (`app/worktree_commands.rs`).
        CommandId::MergeToMain => selected_worktree.is_some_and(|w| !w.is_main),

        // "Already grabbing a branch. Ungrab first (G)."
        // (`app/worktree_commands.rs`). The follow-up "no non-main worktrees to
        // grab" check is not mirrored: it needs `load_grab_branches()`, which
        // mutates overlay state.
        CommandId::GrabBranch => app.worktree_mgr.grabbed_branch.is_none(),

        // "Not grabbing — nothing to ungrab." (`app/commands.rs`).
        CommandId::UngrabBranch => app.worktree_mgr.grabbed_branch.is_some(),

        // "No worktree selected." (`app/worktree_pr.rs`). Whether the branch
        // actually has a PR takes a `git` call, so it is left to the command.
        CommandId::OpenPullRequest => selected_worktree.is_some(),

        // ── Viewer ───────────────────────────────────────────────────────
        // "Raw/Rendered applies to a markdown file in the Viewer"
        // (`app/view_state.rs`) — the same helper the command consults.
        CommandId::ToggleMarkdownRender => app.viewer_state.markdown_toggle_available(),

        // ── Review ───────────────────────────────────────────────────────
        // A comment is anchored to the file open in the Viewer
        // (`app/review_commands.rs`, `if let Some(file_path) = …current_file`).
        CommandId::AddReviewComment => app.viewer_state.content.current_file.is_some(),

        // Both guard on the comment list being the focused sub-panel and
        // non-empty (`app/review_commands.rs`).
        CommandId::DeleteComment | CommandId::ToggleCommentResolve => comment_list_focused(app),

        // Both resolve a selected comment first and bail with "No comment
        // selected." (`app/review_commands.rs`).
        CommandId::EditComment | CommandId::ReplyToComment => app
            .review_state
            .selected_comment_idx(app.viewer_state.explorer.comment_list_selected)
            .is_some(),

        // Needs the review DB and a worktree (`app/review_publish.rs`). Whether
        // the branch has an associated PR is a `get_pr_review_meta` query, so
        // that part is left to the command.
        CommandId::PublishReview => app.review_store.is_some() && selected_worktree.is_some(),

        _ => true,
    }
}

/// Whether the Explorer's bottom pane is showing the comment list, has focus,
/// and has rows — the precondition `cmd_delete_comment` and
/// `cmd_toggle_comment_resolve` both spell out.
fn comment_list_focused(app: &App) -> bool {
    app.viewer_state.explorer.explorer_bottom_view == crate::viewer::ExplorerBottomView::Comments
        && app.viewer_state.explorer.explorer_focus_on_diff_list
        && !app.review_state.comment_list_rows.is_empty()
}
