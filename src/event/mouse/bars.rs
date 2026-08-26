//! メイン3カラム領域より上にある帯のクリック処理を担う。
//! 通知バー、worktree監視バー、タイトルバーが対象。

use crate::app::{App, Focus};

use super::register_double_click;

/// 新規worktree作成ダイアログを開く。worktreeバーの [+] ボタンと空白領域の
/// ダブルクリックの両方から呼ばれる共通処理で、2つの入口の挙動がずれないようにしている。
fn start_worktree_creation(app: &mut App) {
    use crate::app::{StatusLevel, WorktreeInputMode};
    app.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
    app.worktree_mgr.input_buffer.clear();
    app.set_status(
        "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string(),
        StatusLevel::Info,
    );
}

/// ホイール1ノッチでworktreeバーを何チップ分スクロールするか。画面1枚分から
/// 重なり用に1チップ引いた値（最低1）。表示チップ数は直前の描画で記録された
/// Select 領域から読み取る。
pub(super) fn wtbar_page_step(app: &App) -> usize {
    use crate::ui::worktree_bar::WtbarAction;
    let visible = app
        .wtbar
        .hits
        .spans()
        .filter(|(_, _, a)| matches!(a, WtbarAction::Select(_)))
        .count();
    visible.saturating_sub(1).max(1)
}

/// worktreeバーへの左クリックを処理する: 選択（worktreeとそのClaudeセッションへ
/// ジャンプ）、削除（確認あり）、追加のいずれか。クリックがバーの行上であれば
/// true を返す（消費した扱いにする）。
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

    let action = app.wtbar.hits.at(col);

    match action {
        Some(WtbarAction::Select(i)) if i < app.worktrees.len() => {
            app.worktrees.select(i);
            app.on_worktree_changed();
            app.set_focus(Focus::TerminalClaude);
        }
        Some(WtbarAction::ScrollLeft) => {
            app.wtbar.scroll = app.wtbar.scroll.saturating_sub(1);
        }
        Some(WtbarAction::ScrollRight) => {
            app.wtbar.scroll = app.wtbar.scroll.saturating_add(1);
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
                    app.worktrees.select(i);
                    app.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingDelete;
                    app.set_status(
                        format!("Delete worktree '{branch}'? (y/n)"),
                        StatusLevel::Warning,
                    );
                }
            }
        }
        // バーの空白領域: ダブルクリックは [+] ボタンと同じ扱い。
        // シングルクリックはやることがない（バー自体はフォーカスを持たない）ので、
        // ただ消費するだけ。
        None => {
            if register_double_click(
                &mut app.worktree_mgr.wtbar_blank_last_click,
                std::time::Instant::now(),
            ) {
                start_worktree_creation(app);
            }
        }
        // 直前の描画による古い/範囲外の Select ヒット: 無視する。
        Some(WtbarAction::Select(_)) => {}
    }
    true
}

/// タイトルバー（メイン領域より上）への左クリックを処理する。アップデートバッジを
/// クリックするとアップデートフローが始まる。クリックがタイトルバー上であれば
/// true を返す（消費した扱いにする）。
pub(super) fn handle_title_bar_click(
    app: &mut App,
    col: u16,
    row: u16,
    main_area: ratatui::layout::Rect,
) -> bool {
    if row >= main_area.y {
        return false;
    }
    if let Some((start, end)) = app.update.badge_cols
        && col >= start
        && col < end
        && app.update.info.is_some()
    {
        app.start_update_confirm();
    }
    true
}
