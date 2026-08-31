//! worktree パネル — 一番左の列で worktree 一覧を表示する。
//!
//! 選択状態・ステータス表示・詳細情報、および任意の装飾ゾーン（水槽など）を持つ
//! worktree 一覧を描画する。
//!
//! 描画責務で分割されている: [list] が worktree/セッション一覧（ゾーン1）を、
//! [detail] が選択中 worktree の詳細セクション（ゾーン2）を描く。
//! ゾーン3（装飾）は [crate::worktree::decoration] を直接呼んで描画する。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::{App, Focus};
use crate::worktree::decoration::{self, DecorationMode};

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
    // ゾーン1: worktree + セッション一覧 — 40%（またはそれ以上）
    // ゾーン2: 詳細セクション           — 60%（またはそれ以下）
    // ゾーン3: 装飾（任意）             — 20%
    let decoration_mode = DecorationMode::from_str(&app.config.general.decoration);

    let zones = if area.height < 10 {
        // 狭すぎる場合は一覧のみ表示する。
        Layout::vertical([
            Constraint::Percentage(100),
            Constraint::Length(0),
            Constraint::Length(0),
        ])
        .split(area)
    } else if decoration_mode != DecorationMode::None {
        // 装飾が有効な場合は3ゾーンレイアウト。
        Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Percentage(40),
            Constraint::Percentage(20),
        ])
        .split(area)
    } else {
        // 装飾なしの場合は2ゾーンレイアウト。
        Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
            Constraint::Length(0),
        ])
        .split(area)
    };

    // ゾーン1: worktree 一覧。
    list::render_worktree_list(frame, zones[0], app, focused, border_color);

    // ゾーン2: 詳細セクション。
    if zones[1].height >= 3 {
        let theme = &app.appearance.theme;
        detail::render_detail(frame, zones[1], app, theme, border_color);
    }

    // ゾーン3: 装飾（任意）。
    if zones[2].height >= 4 {
        decoration::render_decoration(
            frame,
            zones[2],
            &app.decoration_states,
            &app.appearance.theme,
            decoration_mode,
        );
    }
}
