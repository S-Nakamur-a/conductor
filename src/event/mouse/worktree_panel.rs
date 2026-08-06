//! Worktreeカラム（worktreeリスト / インラインセッション）のクリック処理。

use crate::app::{App, Focus};

use super::{ClickGeometry, register_double_click, register_double_click_on};

/// Worktreeカラム（worktreeリスト / インラインセッション）内の左クリックを処理する。
pub(super) fn handle_worktree_column_click(app: &mut App, row: u16, geom: &ClickGeometry) {
    let main_area = geom.main_area;
    // クリックでworktree/セッションを選択し切り替える。
    let relative_row = (row - main_area.y) as usize;
    let item_row = relative_row.saturating_sub(1); // 行0は枠

    if !app.worktrees.rows.is_empty() && item_row < app.worktrees.rows.len() {
        // ダブルクリック検出。
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
        let is_double = register_double_click(
            &mut app.worktree_mgr.blank_last_click,
            std::time::Instant::now(),
        );

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
