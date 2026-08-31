//! Worktreeカラム（worktreeリスト / インラインセッション）のクリック処理。

use crate::app::{App, Focus};
use crate::event::mouse::ClickGeometry;

/// Worktreeカラム（worktreeリスト / インラインセッション）内の左クリックを処理する。
pub(crate) fn handle_worktree_column_click(app: &mut App, row: u16, geom: &ClickGeometry) {
    let main_area = geom.main_area;
    // クリックでworktree/セッションを選択し切り替える。
    let relative_row = (row - main_area.y) as usize;
    let item_row = relative_row.saturating_sub(1); // 行0は枠

    if !app.worktrees.rows.is_empty() && item_row < app.worktrees.rows.len() {
        // ダブルクリック検出。
        let is_double = app.worktree_mgr.item_clicks.is_double(item_row);

        app.set_focus(Focus::Worktree);
        app.worktrees.row_selected = item_row;
        app.sync_selected_worktree();
        match app.worktrees.rows[item_row] {
            crate::app::WorktreeListRow::Session { pty_idx, .. } => {
                app.on_worktree_changed();
                app.switch_claude_session(pty_idx);
                // シングルクリック: フォーカスをworktreeパネルに残す。
                // ダブルクリック: フォーカスをターミナルに移す。
                if is_double {
                    app.set_focus(Focus::TerminalClaude);
                }
            }
            crate::app::WorktreeListRow::Worktree(_) => {
                app.on_worktree_changed();
                // シングルクリックでもダブルクリックでもフォーカスはworktreeパネルのまま。
            }
        }
    } else {
        // worktree項目より下の空白部分へのクリック。
        let is_double = app.worktree_mgr.blank_clicks.is_double(0);

        if is_double {
            // ダブルクリック → worktree作成ダイアログを開く。
            app.worktree_mgr.input_mode = crate::app::WorktreeInputMode::CreatingWorktree;
            app.worktree_mgr.input_buffer.clear();
        } else {
            // シングルクリック → フォーカスするだけ。
            app.set_focus(Focus::Worktree);
        }
    }
}
