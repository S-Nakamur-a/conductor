//! Click handling for the Worktree column (worktree list / inline sessions).

use crate::app::{App, Focus};

use super::{register_double_click, register_double_click_on, ClickGeometry};

/// Handle a left click in the Worktree column (worktree list / inline sessions).
pub(super) fn handle_worktree_column_click(app: &mut App, row: u16, geom: &ClickGeometry) {
    let main_area = geom.main_area;
    // Click selects and switches to the worktree/session.
    let relative_row = (row - main_area.y) as usize;
    let item_row = relative_row.saturating_sub(1); // row 0 is border

    if !app.worktrees.rows.is_empty() && item_row < app.worktrees.rows.len() {
        // Double-click detection.
        let is_double = register_double_click_on(
            &mut app.worktree_mgr.item_last_click,
            &mut app.worktree_mgr.item_last_click_idx,
            item_row,
            std::time::Instant::now(),
        );

        app.set_focus(Focus::Worktree);
        app.worktrees.row_selected = item_row;
        app.sync_selected_worktree();
        match app.worktrees.rows[item_row] {
            crate::app::WorktreeListRow::Session { pty_idx, .. } => {
                app.on_worktree_changed();
                app.switch_claude_session(pty_idx);
                // Single click: keep focus on worktree panel.
                // Double click: move focus to terminal.
                if is_double {
                    app.set_focus(Focus::TerminalClaude);
                }
            }
            crate::app::WorktreeListRow::Worktree(_) => {
                app.on_worktree_changed();
                // Focus stays on worktree panel for both single and double click.
            }
        }
    } else {
        // Clicked on blank space below worktree items.
        let is_double = register_double_click(
            &mut app.worktree_mgr.blank_last_click,
            std::time::Instant::now(),
        );

        if is_double {
            // Double-click → open worktree creation dialog.
            app.worktree_mgr.input_mode = crate::app::WorktreeInputMode::CreatingWorktree;
            app.worktree_mgr.input_buffer.clear();
        } else {
            // Single click → just focus.
            app.set_focus(Focus::Worktree);
        }
    }
}
