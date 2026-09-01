//! worktree パネル — 一番左の列で worktree 一覧を表示する。
//!
//! 選択状態・ステータス表示・詳細情報を持つ worktree 一覧を描画する。
//!
//! 描画責務で分割されている: [list] が worktree/セッション一覧（ゾーン1）を、
//! [detail] が選択中 worktree の詳細セクション（ゾーン2）を描く。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::{App, Focus};

mod detail;
mod list;

/// worktree パネルを指定領域に描画する。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus.current() == Focus::Worktree;
    let border_color = if focused {
        app.appearance.theme.border_focused
    } else {
        app.appearance.theme.border_unfocused
    };

    // ゾーンのレイアウト計算。
    // ゾーン1: worktree + セッション一覧 — 40%
    // ゾーン2: 詳細セクション           — 60%
    let zones = if area.height < 10 {
        // 狭すぎる場合は一覧のみ表示する。
        Layout::vertical([Constraint::Percentage(100), Constraint::Length(0)]).split(area)
    } else {
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area)
    };

    // ゾーン1: worktree 一覧。
    list::render_worktree_list(frame, zones[0], app, focused, border_color);

    // ゾーン2: 詳細セクション。
    if zones[1].height >= 3 {
        let theme = &app.appearance.theme;
        detail::render_detail(frame, zones[1], app, theme, border_color);
    }
}
