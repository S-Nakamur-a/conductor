//! Worktree ステータスバー — 以前の左カラムのworktree一覧を置き換える、
//! コンパクトで全幅のストリップ。
//!
//! すべての worktree を一目で把握できる（ブランチ、変更ファイル数、ahead/behind、
//! Claude Code の待機/稼働状態）ので、複数の並行セッションを視界の端で監視
//! できる。ストリップは操作可能: worktree をクリックするとそこ（とその
//! Claude セッション）にジャンプし、[+] で worktree を作成し、チップごとの
//! ✕ で削除する（確認あり）。より詳細な一覧/詳細UIはスイッチャーモーダル
//! （render_switcher_overlay）にある。

use crate::app::App;
use crate::hit_map::ColumnSpans;
use crate::ui::common::strip::visible_window;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

/// worktree バーのクリック可能領域が何をするか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WtbarAction {
    /// このインデックスの worktree（とその Claude セッション）にジャンプする。
    Select(usize),
    /// このインデックスの worktree を削除する（確認あり）。
    Delete(usize),
    /// 新しい worktree を作成する。
    Add,
    /// 左端に隠れている worktree を見せるためにストリップをスクロールする。
    ScrollLeft,
    /// 右端に隠れている worktree を見せるためにストリップをスクロールする。
    ScrollRight,
}

fn w(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// 走らせていない worktree では空。印の無い状態が「まだ解析していない」を意味するので、
/// 印そのものが情報として効く。
fn review_mark(state: crate::revidere::ArtifactState, ui_tick: u64) -> String {
    match state {
        crate::revidere::ArtifactState::None => String::new(),
        state => format!(" {}", crate::ui::common::revidere_marker(state, ui_tick)),
    }
}

/// 背景は敷かない。全テーマで accent と selected_bg が同じ色なので、選択中チップの
/// 塗りを引き継ぐと実行中の印が背景と同色になって消える。
fn review_style(theme: &crate::theme::Theme, state: crate::revidere::ArtifactState) -> Style {
    Style::default()
        .fg(crate::ui::common::revidere_color(theme, state))
        .add_modifier(Modifier::BOLD)
}

/// 描画前に集めた worktree ごとのデータ。可変幅のウィンドウを app への
/// 借用を保持したまま計算できるようにする。
struct Chip {
    text: String,
    width: u16,
    /// 削除ボタン（✕）。main worktree では空。
    del: &'static str,
    del_width: u16,
    waiting: bool,
    active: bool,
    is_current: bool,
    /// revidere の解析の状態。マーカーの色を決めるのに使う。
    review: crate::revidere::ArtifactState,
    /// チップの外に置くマーカー。解析していない worktree では空。
    review_text: String,
    review_width: u16,
}

/// worktree モニターストリップを描画し、そのクリック可能領域を
/// app.wtbar.hits に記録する。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    app.wtbar.hits.clear();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let muted = app.theme.muted;
    let success = app.theme.success;
    let warning = app.theme.warning;
    let border = app.theme.border_secondary;
    let error = app.theme.error;

    // スマート/通常の worktree がバックグラウンドで作成中 → 一目でわかる
    // よう最も左のマーカーを回転させる。
    let creating = app.worktree_mgr.pending_worktrees.iter().any(|p| {
        matches!(
            p.op,
            crate::app::PendingWorktreeOp::Creating | crate::app::PendingWorktreeOp::SmartCreating
        )
    });

    let max_x = area.x + area.width;
    let mut x = area.x;
    let mut spans: Vec<Span> = Vec::new();
    let mut hits = ColumnSpans::default();

    // 識別マーカー（worktree 作成中は回転する）。
    {
        let icon = if creating {
            format!("{} ", crate::ui::common::spinner_frame(app.ui_tick))
        } else {
            "\u{2387} ".to_string()
        };
        let style = if creating {
            Style::default().fg(success).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted).add_modifier(Modifier::BOLD)
        };
        x += w(&icon);
        spans.push(Span::styled(icon, style));
    }
    // 新規 worktree ボタンは右端に固定する（Claude/Shell セッションタブバーと
    // 一貫させている）。ここで場所を確保しておき、最後に描画する。
    // " [+]" = 前方の空白 + ボタン。
    let add = " [+]";
    let add_w = w(add);
    let chips_max_x = max_x.saturating_sub(add_w);

    // 先にチップのデータを集めておく（app.worktrees への借用を解放するため）。
    let chips: Vec<Chip> = app
        .worktrees
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let waiting = app.terminal.cc_waiting_worktrees.contains(&wt.path);
            let active = app.terminal.cc_active_worktrees.contains(&wt.path);

            let mut text = String::from(" ");
            if waiting {
                text.push_str("\u{23f3} ");
            } else if active {
                text.push_str("\u{25cf} ");
            }
            text.push_str(&wt.branch);
            if !wt.is_clean {
                text.push_str(&format!(" ~{}", wt.added + wt.modified + wt.deleted));
            }
            if let Some(a) = wt.ahead
                && a > 0
            {
                text.push_str(&format!(" \u{2191}{a}"));
            }
            if let Some(b) = wt.behind
                && b > 0
            {
                text.push_str(&format!(" \u{2193}{b}"));
            }
            text.push(' ');
            // revidere の状態は常に見えていてほしい。複数の worktree で
            // 走らせると、終わったのがどれかはステータス行の 1 本では追えない。
            // 出すのはチップの外 — 中に入れると、選択中チップの塗りと同じ色に
            // なって消えるうえ、~3 や ↑1 と並んで git の情報に見える。
            let review = crate::revidere::artifact_state(
                &wt.path,
                wt.head_time,
                app.revidere.runs.is_running(&wt.branch),
            );
            let review_text = review_mark(review, app.ui_tick);

            // Claude/Shell セッションタブに合わせて [x]（以前は ✕）。チップの
            // 塗りつぶし背景のすぐ外側に置くので、危険色の赤が読みやすいまま
            // になる。
            let del = if wt.is_main { "" } else { "[x]" };
            Chip {
                width: w(&text),
                del,
                del_width: w(del),
                waiting,
                active,
                is_current: i == app.worktrees.selected_index(),
                review,
                review_width: w(&review_text),
                review_text,
                text,
            }
        })
        .collect();

    let total = chips.len();
    // 最初に表示するチップ以外の前に描く区切り文字。幅とリテラルを一緒に
    // 定義することで両者がずれないようにしている。
    let sep = "\u{2502} ";
    let sep_w = w(sep);
    let avail_full = chips_max_x.saturating_sub(x);

    // オーバーフローヒントなしですべて収まるか？収まるならヒント分の確保を省く。
    let slots: Vec<u16> = chips
        .iter()
        .map(|c| c.width + c.review_width + c.del_width)
        .collect();
    let all_fit = visible_window(&slots, sep_w, avail_full, 0, 0, false).1 == total;

    // スクロールが必要な場合、左右のオーバーフローヒントの分の場所を確保する。
    let hint_reserve_per_side = 5u16;
    let avail = if all_fit {
        avail_full
    } else {
        avail_full.saturating_sub(hint_reserve_per_side * 2)
    };

    let (start, end) = if all_fit {
        (0, total)
    } else {
        visible_window(
            &slots,
            sep_w,
            avail,
            app.wtbar.scroll,
            app.worktrees.selected_index(),
            app.wtbar.reveal_selected,
        )
    };

    // 左側のオーバーフローヒント（クリックで左スクロール）。その側に隠れている
    // worktree のいずれかがユーザ入力待ちなら warning 色で強調する。
    if start > 0 {
        let waiting_left = chips[..start].iter().any(|c| c.waiting);
        let hint = format!("\u{2039}{} ", start);
        let hw = w(&hint);
        spans.push(Span::styled(
            hint,
            Style::default().fg(if waiting_left { warning } else { muted }),
        ));
        hits.push(x, x + hw, WtbarAction::ScrollLeft);
        x += hw;
    }

    for (offset, chip) in chips[start..end].iter().enumerate() {
        let i = start + offset;
        if offset > 0 {
            spans.push(Span::styled(sep, Style::default().fg(border)));
            x += sep_w;
        }

        let chip_style = if chip.is_current {
            // アクティブな worktree が単なる色変化ではなく一目でわかるよう、
            // 塗りつぶしたチップにする — チップのテキスト自身が前後の空白を
            // 持っている。
            Style::default()
                .fg(app.theme.selected_fg)
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else if chip.waiting {
            Style::default().fg(warning)
        } else if chip.active {
            Style::default().fg(success)
        } else {
            Style::default().fg(muted)
        };
        // hover 時の背景（bar/タブ類は gutter_hover_bg を再利用しており、
        // どのテーマでも現在のチップ自身の selected_bg の塗りとは区別できる）。
        // 現在のチップは既に強い塗りつぶしを持っているので、その上に hover
        // 背景を重ねてはいない — それ以上区別する余地がないため。
        let chip_style = if !chip.is_current && app.wtbar.hover == Some(WtbarAction::Select(i)) {
            chip_style.bg(app.theme.gutter_hover_bg)
        } else {
            chip_style
        };
        spans.push(Span::styled(chip.text.clone(), chip_style));
        hits.push(x, x + chip.width, WtbarAction::Select(i));
        x += chip.width;

        if !chip.del.is_empty() {
            let del_style = Style::default().fg(error);
            let del_style = if app.wtbar.hover == Some(WtbarAction::Delete(i)) {
                del_style.bg(app.theme.gutter_hover_bg)
            } else {
                del_style
            };
            spans.push(Span::styled(chip.del, del_style));
            hits.push(x, x + chip.del_width, WtbarAction::Delete(i));
            x += chip.del_width;
        }

        // revidere のマーカーはチップの塗りの外に置く。背景を敷かないので
        // 全テーマで色がそのまま出るし、塗りの外にあること自体が「チップ本体
        // とは別のもの」という手掛かりになる。押したときはチップと同じく
        // その worktree へ移る。
        if chip.review_width > 0 {
            spans.push(Span::styled(
                chip.review_text.clone(),
                review_style(&app.theme, chip.review),
            ));
            hits.push(x, x + chip.review_width, WtbarAction::Select(i));
            x += chip.review_width;
        }
    }

    // 右側のオーバーフローヒント（クリックで右スクロール）。
    if end < total {
        let waiting_right = chips[end..].iter().any(|c| c.waiting);
        let hint = format!(" {}\u{203a}", total - end);
        let hw = w(&hint);
        spans.push(Span::styled(
            hint,
            Style::default().fg(if waiting_right { warning } else { muted }),
        ));
        hits.push(x, x + hw, WtbarAction::ScrollRight);
        x += hw;
    }

    // 新規 worktree の [+] ボタンを右端にぴったり固定する。
    if x < chips_max_x {
        let pad = (chips_max_x - x) as usize;
        spans.push(Span::raw(" ".repeat(pad)));
        x = chips_max_x;
    }
    spans.push(Span::styled(
        add,
        Style::default().fg(success).add_modifier(Modifier::BOLD),
    ));
    hits.push(x, x + add_w, WtbarAction::Add);

    app.wtbar.scroll = start;
    app.wtbar.reveal_selected = false;
    app.wtbar.hits = hits;
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// worktree スイッチャーモーダルを描画する: 中央に配置したポップアップで、
/// worktree パネル全体（一覧＋詳細＋セッション）をそのまま再利用するため
/// 選択/作成などは既存のUIとキー操作をそのまま保つ。
pub fn render_switcher_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    // 小さすぎるターミナルで min > max になって u16::clamp が panic しない
    // よう、下限を area にクランプする。
    let w = ((area.width as u32 * 60 / 100) as u16).clamp(24.min(area.width), area.width);
    let h = ((area.height as u32 * 70 / 100) as u16).clamp(6.min(area.height), area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);
    frame.render_widget(ratatui::widgets::Clear, popup);
    crate::worktree::render::render(frame, popup, app);
}

#[cfg(test)]
mod tests {
    use super::{review_mark, review_style};
    use crate::revidere::ArtifactState;
    use crate::theme::Theme;

    /// 走らせていない worktree には何も出さない。印が付いていないこと自体が
    /// 「まだ解析していない」を意味するので、ここが空でなくなると印が意味を失う。
    #[test]
    fn an_unanalysed_worktree_gets_no_mark() {
        assert_eq!(review_mark(ArtifactState::None, 0), "");
        for state in [
            ArtifactState::Running,
            ArtifactState::Fresh,
            ArtifactState::Stale,
        ] {
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(review_mark(state, 0).as_str()),
                2,
                "{state:?}"
            );
        }
    }

    /// 印に背景を敷かないこと。どのテーマでも accent は selected_bg と同じ色で、
    /// 選択中チップの塗りを引き継いだ瞬間に実行中の印が背景と同色になって消える。
    #[test]
    fn the_mark_never_carries_a_background() {
        let theme = Theme::from_name("catppuccin-mocha");
        assert_eq!(theme.accent, theme.selected_bg, "前提が変わっている");
        for state in [
            ArtifactState::Running,
            ArtifactState::Fresh,
            ArtifactState::Stale,
        ] {
            assert!(review_style(&theme, state).bg.is_none(), "{state:?}");
        }
    }

    // 幅が均一な10個のチップ、区切り文字幅は1。
}
