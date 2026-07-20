//! Click handling for the strips above the main three-column area: the
//! notification bar, the worktree monitor strip, and the title bar.

use crate::app::{App, Focus};

use super::register_double_click;

/// Open the new-worktree creation dialog. Shared by the worktree bar's `[+]`
/// button and its blank-area double-click so the two entry points can't drift.
fn start_worktree_creation(app: &mut App) {
    use crate::app::{StatusLevel, WorktreeInputMode};
    app.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
    app.worktree_mgr.input_buffer.clear();
    app.set_status(
        "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string(),
        StatusLevel::Info,
    );
}

/// Handle a left click on the notification bar. Badge clicks jump to the
/// matching worktree. Returns `true` if the click was on the notification bar
/// (and thus consumed), regardless of whether a badge was hit.
pub(super) fn handle_notification_bar_click(
    app: &mut App,
    col: u16,
    row: u16,
    notif_area: ratatui::layout::Rect,
) -> bool {
    if notif_area.height == 0 || row != notif_area.y {
        return false;
    }
    for (start_col, end_col, branch) in &app.notification_bar_badges {
        if col >= *start_col && col < *end_col {
            if let Some(wt_idx) = app.worktrees.iter().position(|w| w.branch == *branch) {
                app.selected_worktree = wt_idx;
                app.on_worktree_changed();
                app.set_focus(Focus::TerminalClaude);
            }
            return true;
        }
    }
    true
}

/// How many chips a single wheel tick scrolls the worktree strip: a screenful
/// minus one chip of overlap (at least 1). The visible count is read back from
/// the `Select` regions recorded by the last render.
pub(super) fn wtbar_page_step(app: &App) -> usize {
    use crate::ui::worktree_bar::WtbarAction;
    let visible = app
        .wtbar_hits
        .iter()
        .filter(|h| matches!(h.action, WtbarAction::Select(_)))
        .count();
    visible.saturating_sub(1).max(1)
}

/// Handle a left click on the worktree bar: select (jump to the worktree and
/// its Claude session), delete (with confirmation), or add. Returns `true` if
/// the click landed on the bar row (and was thus consumed).
pub(super) fn handle_wtbar_click(
    app: &mut App,
    col: u16,
    row: u16,
    wtbar_area: ratatui::layout::Rect,
) -> bool {
    if wtbar_area.height == 0 || row != wtbar_area.y {
        return false;
    }
    use crate::app::{StatusLevel, WorktreeInputMode};
    use crate::ui::worktree_bar::WtbarAction;

    let action = app
        .wtbar_hits
        .iter()
        .find(|h| col >= h.x0 && col < h.x1)
        .map(|h| h.action);

    match action {
        Some(WtbarAction::Select(i)) if i < app.worktrees.len() => {
            app.selected_worktree = i;
            app.on_worktree_changed();
            app.set_focus(Focus::TerminalClaude);
        }
        Some(WtbarAction::ScrollLeft) => {
            app.wtbar_scroll = app.wtbar_scroll.saturating_sub(1);
        }
        Some(WtbarAction::ScrollRight) => {
            app.wtbar_scroll = app.wtbar_scroll.saturating_add(1);
        }
        Some(WtbarAction::Add) => start_worktree_creation(app),
        Some(WtbarAction::Delete(i)) => {
            if let Some(wt) = app.worktrees.get(i) {
                if wt.is_main {
                    app.set_status(
                        "Cannot delete the main worktree.".to_string(),
                        StatusLevel::Error,
                    );
                } else if app.is_worktree_pending_delete(&wt.path) {
                    app.set_status(
                        "Worktree is already being deleted.".to_string(),
                        StatusLevel::Warning,
                    );
                } else {
                    let branch = wt.branch.clone();
                    app.selected_worktree = i;
                    app.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingDelete;
                    app.set_status(
                        format!("Delete worktree '{branch}'? (y/n)"),
                        StatusLevel::Warning,
                    );
                }
            }
        }
        // Blank area of the bar: a double-click acts as the `[+]` button. A
        // single click has nothing to do (the bar holds no focus), so it is
        // just consumed.
        None => {
            if register_double_click(
                &mut app.worktree_mgr.wtbar_blank_last_click,
                std::time::Instant::now(),
            ) {
                start_worktree_creation(app);
            }
        }
        // Stale/out-of-range `Select` hit from a prior render: ignore.
        Some(WtbarAction::Select(_)) => {}
    }
    true
}

/// Handle a left click on the title bar (above the main area). Clicking the
/// update badge starts the update flow. Returns `true` if the click was on the
/// title bar (and thus consumed).
pub(super) fn handle_title_bar_click(
    app: &mut App,
    col: u16,
    row: u16,
    main_area: ratatui::layout::Rect,
) -> bool {
    if row >= main_area.y {
        return false;
    }
    if let Some((start, end)) = app.update_badge_cols
        && col >= start
        && col < end
        && app.update_info.is_some()
    {
        app.start_update_confirm();
    }
    true
}
