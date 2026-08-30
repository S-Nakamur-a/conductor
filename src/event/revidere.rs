//! revidere の 2 列レビュービューのキー処理。
//!
//! このビューは画面全体を占有するので、下に他のパネルは無い。フォールバック
//! 先が無いぶん、このレイヤーがキーをすべて引き受ける。

use crossterm::event::KeyEvent;

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};

/// 1 回のスクロールで動かす行数 (j/k)。
const SCROLL_STEP: usize = 1;

pub(super) fn handle_revidere_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let action = app.keymap.resolve(&key, KeyContext::Revidere)?;
    // 画面の切り替えは行き先ごとにキーが分かれているので、どちらの画面から
    // でも同じ結果になる。先に受けてしまってよい。
    match action {
        Action::RevidereShowOverview => {
            app.revidere.show_overview = true;
            return None;
        }
        Action::RevidereShowSections => {
            app.revidere.show_overview = false;
            return None;
        }
        // 区間の切り替えと解析の起動は、どちらの列を出していても同じ意味。
        Action::RevidereToggleScope => {
            app.cmd_toggle_revidere_scope();
            return None;
        }
        Action::AnalyzeRevidere => {
            app.cmd_confirm_analyze_revidere();
            return None;
        }
        Action::ForceAnalyzeRevidere => {
            app.cmd_analyze_revidere(true);
            return None;
        }
        _ => {}
    }

    // 概要は 1 列で読むだけの画面なので、項目に関わるキーは効かない。
    if app.revidere.show_overview {
        match action {
            Action::NavigateDown => scroll_overview(app, SCROLL_STEP as isize),
            Action::NavigateUp => scroll_overview(app, -(SCROLL_STEP as isize)),
            Action::GoToTop => app.revidere.overview_scroll = 0,
            // 概要を読み終えたら次は項目。読む順の入口として enter も通す。
            Action::Select => app.revidere.show_overview = false,
            Action::ExitSubPanel => app.set_focus(Focus::Explorer),
            _ => {}
        }
        return None;
    }

    match action {
        Action::NavigateDown => scroll_diff(app, SCROLL_STEP as isize),
        Action::NavigateUp => scroll_diff(app, -(SCROLL_STEP as isize)),
        Action::GoToTop => {
            app.revidere.selected = 0;
            app.revidere.diff_scroll = 0;
        }
        Action::GoToBottom => {
            let last = section_count(app).saturating_sub(1);
            select_section(app, last);
        }
        Action::RevidereNextSection => step_section(app, 1),
        Action::RevidererPrevSection => step_section(app, -1),
        // 項目が指す位置を通常の Viewer で開く。レビューコメントは Viewer 側に
        // あるので、ここが 2 列ビューと既存のコメント作成をつなぐ唯一の口になる。
        Action::Select => app.jump_to_selected_section(),
        Action::ExitSubPanel => app.set_focus(Focus::Explorer),
        _ => {}
    }
    None
}

fn section_count(app: &App) -> usize {
    app.revidere
        .current
        .as_ref()
        .map(|r| r.order.sections.len())
        .unwrap_or(0)
}

pub(super) fn scroll_diff(app: &mut App, delta: isize) {
    let cur = app.revidere.diff_scroll as isize;
    app.revidere.diff_scroll = (cur + delta).max(0) as usize;
}

pub(super) fn scroll_overview(app: &mut App, delta: isize) {
    let cur = app.revidere.overview_scroll as isize;
    app.revidere.overview_scroll = (cur + delta).max(0) as usize;
}

/// 項目の選択を delta 分動かし、右の diff をその項目の先頭へ送る。
///
/// 空の項目 (成果物が指した位置が diff に無かった項目) も飛ばさずに止まる。
/// 黙って飛ばすと、「在ると言った変更が無かった」ことに気付けなくなる。
pub(super) fn step_section(app: &mut App, delta: isize) {
    let len = section_count(app);
    if len == 0 {
        return;
    }
    let cur = app.revidere.selected as isize;
    select_section(app, (cur + delta).clamp(0, len as isize - 1) as usize);
}

pub(super) fn select_section(app: &mut App, idx: usize) {
    let Some(review) = app.revidere.current.as_ref() else {
        return;
    };
    if idx >= review.order.sections.len() {
        return;
    }
    app.revidere.selected = idx;
    // 先頭行は描画側が記録している。まだ 1 度も描いていなければ動かさない
    // (次のフレームで正しい位置が入る)。
    if let Some(row) = app.revidere.section_rows.get(idx).copied() {
        app.revidere.diff_scroll = row;
    }
}
