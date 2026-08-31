//! Viewer で開いているファイルのタブ行。
//!
//! ブロック内側の先頭行に描く。パンくずと同じ扱いで、コード行がその分だけ
//! 下がる（[super::file_view] が screen_row_map にプレースホルダを詰める）。
//!
//! タブが 1 枚だけのときは描かない — パスはタイトルに出ているので、1 行を
//! 消費するだけの価値が無い。
//!
//! クリック領域はターミナルのタブバーと同じ [TabHit] で表すので、マウス処理は
//! 幅を計算し直さずこの描画結果をそのまま引ける。オーバーフローヒント
//! （‹N / N›）もクリックできる領域で、ターミナルのタブバーや worktree
//! ストリップと同じく窓を横へずらす。

use crate::hit_map::ColumnSpans;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;
use crate::ui::common::strip::visible_window;
use crate::ui::tab_bar::TabAction;
use crate::viewer::ViewerState;

const CLOSE: &str = " [x]";
const SEP: &str = " ";

fn w(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// パスを max_w 表示カラムに収める。前を削って末尾（ファイル名）を残す。
fn elide_head(path: &str, max_w: u16) -> String {
    let max_w = max_w as usize;
    if UnicodeWidthStr::width(path) <= max_w {
        return path.to_string();
    }
    if max_w <= 1 {
        return "\u{2026}".to_string();
    }
    let budget = max_w - 1;
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0usize;
    for ch in path.chars().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > budget {
            break;
        }
        tail.push(ch);
        used += cw;
    }
    tail.push('\u{2026}');
    tail.reverse();
    tail.into_iter().collect()
}

/// タブ行を描くかどうか。描かないなら Viewer は 1 行も失わない。
pub(crate) fn is_visible(vs: &ViewerState) -> bool {
    vs.tabs.len() >= 2
}

/// タブ行を area（高さ 1）に描き、クリック領域と解決後のスクロール位置
/// （最初に表示したタブ。呼び出し側が state へ書き戻す）を返す。
pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    vs: &ViewerState,
) -> (ColumnSpans<TabAction>, usize) {
    let mut hits = ColumnSpans::default();
    if area.width == 0 || area.height == 0 || !is_visible(vs) {
        return (hits, vs.tab_scroll);
    }

    let close_w = w(CLOSE);
    let sep_w = w(SEP);
    let hint_reserve = 4u16; // "\u{2039}NN " / " NN\u{203a}"

    // 1 枚の長いパスが行を占有しないよう、ラベル幅に上限を設ける。
    let max_label_w = area
        .width
        .saturating_sub(close_w + sep_w + hint_reserve * 2)
        .clamp(6, 32);
    let labels: Vec<String> = vs
        .tabs
        .iter()
        .map(|t| elide_head(&t.path, max_label_w))
        .collect();
    let slots: Vec<u16> = labels.iter().map(|l| w(l) + close_w).collect();

    let total = vs.tabs.len();
    let all_fit = visible_window(&slots, sep_w, area.width, 0, 0, false).1 == total;
    let avail = if all_fit {
        area.width
    } else {
        area.width.saturating_sub(hint_reserve * 2)
    };
    let (start, end) = if all_fit {
        (0, total)
    } else {
        visible_window(
            &slots,
            sep_w,
            avail,
            vs.tab_scroll,
            vs.active_tab,
            vs.tab_reveal,
        )
    };

    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;

    if start > 0 {
        let hint = format!("\u{2039}{start} ");
        let hint_w = w(&hint);
        spans.push(Span::styled(hint, Style::default().fg(theme.hint)));
        hits.push(x, x + hint_w, TabAction::ScrollLeft);
        x += hint_w;
    }

    for (idx, label) in labels.iter().enumerate().take(end).skip(start) {
        if idx > start {
            spans.push(Span::styled(
                SEP,
                Style::default().fg(theme.border_unfocused),
            ));
            x += sep_w;
        }
        let label_w = w(label);
        let mut style = if idx == vs.active_tab {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        if vs.tabs[idx].status.is_preview() {
            style = style.add_modifier(Modifier::ITALIC);
        }
        spans.push(Span::styled(label.clone(), style));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("[x]", Style::default().fg(theme.error)));
        hits.push(x, x + label_w + 1, TabAction::Select(idx));
        hits.push(
            x + label_w + 1,
            x + label_w + close_w,
            TabAction::Close(idx),
        );
        x += label_w + close_w;
    }

    if end < total {
        let hint = format!(" {}\u{203a}", total - end);
        let hint_w = w(&hint);
        spans.push(Span::styled(hint, Style::default().fg(theme.hint)));
        hits.push(x, x + hint_w, TabAction::ScrollRight);
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    (hits, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn state(paths: &[&str], active: usize) -> ViewerState {
        let mut vs = ViewerState::default();
        for path in paths {
            vs.tabs.push(crate::viewer::ViewerTab::for_test(path));
        }
        vs.active_tab = active;
        // タブを切り替えた直後の状態。focus_tab が立てるのと同じ。
        vs.tab_reveal = true;
        vs
    }

    /// [super::super::file_view::render_tab_row] と同じ書き戻しをする。
    fn draw(width: u16, vs: &mut ViewerState) -> (ratatui::buffer::Buffer, ColumnSpans<TabAction>) {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        let mut hits = ColumnSpans::default();
        let mut scroll = 0;
        terminal
            .draw(|f| {
                let (h, s) = render(f, f.area(), &theme, vs);
                hits = h;
                scroll = s;
            })
            .unwrap();
        vs.tab_scroll = scroll;
        vs.tab_reveal = false;
        (terminal.backend().buffer().clone(), hits)
    }

    fn text(buf: &ratatui::buffer::Buffer, width: u16) -> String {
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    /// タブが 1 枚だけならタブ行は出ない。パスはタイトルに出ているので、
    /// 1 行を使う理由が無い。
    #[test]
    fn タブが1つなら行を取らない() {
        let mut vs = state(&["src/main.rs"], 0);
        assert!(!is_visible(&vs));
        let (_, hits) = draw(40, &mut vs);
        assert!(hits.is_empty());
    }

    /// 長いパスは前を削ってファイル名を残す。頭から切ると、どのタブも
    /// "src/ui/vie…" のように見分けが付かなくなる。
    #[test]
    fn 長いパスは末尾を残す() {
        assert_eq!(
            elide_head("src/viewer/render/tab_row.rs", 12),
            "\u{2026}/tab_row.rs"
        );
        assert_eq!(elide_head("a.rs", 12), "a.rs");
    }

    #[test]
    fn 見えているタブは選べて閉じられる() {
        let mut vs = state(&["a.rs", "b.rs"], 0);
        let (_, hits) = draw(60, &mut vs);
        for idx in 0..2 {
            assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Select(idx)));
            assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Close(idx)));
        }
        // Select 領域のクリックはそのタブに当たる。
        let sel = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Select(1))
            .unwrap();
        assert_eq!(hits.at(sel.0), Some(TabAction::Select(1)));
    }

    /// preview タブは italic で描く。永続タブとの違いはここにしか出ない。
    #[test]
    fn previewのタブは斜体で描く() {
        let mut vs = state(&["a.rs", "b.rs"], 1);
        vs.tabs[1].status = crate::viewer::ViewerTabStatus::Preview;
        let (buf, hits) = draw(60, &mut vs);
        let start = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Select(1))
            .unwrap()
            .0;
        assert!(buf[(start, 0)].modifier.contains(Modifier::ITALIC));
        let persistent = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::Select(0))
            .unwrap()
            .0;
        assert!(!buf[(persistent, 0)].modifier.contains(Modifier::ITALIC));
    }

    /// アクティブなタブは、はみ出していても必ず見えていなければならない —
    /// 今どのファイルを読んでいるのかが分からなくなる。
    #[test]
    fn 溢れてもアクティブなタブは見える位置に残る() {
        let paths: Vec<String> = (0..12).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut vs = state(&refs, 11);
        let (buf, hits) = draw(40, &mut vs);
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Select(11)));
        assert!(text(&buf, 40).contains("file11.rs"));
    }

    /// はみ出したタブへはヒントのクリックで届く。ここが空だと、隠れたタブは
    /// マウスからは一切触れない（それがこのヒントの唯一の役目）。
    #[test]
    fn 溢れの印を押すと隠れたタブに届く() {
        let paths: Vec<String> = (0..12).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut vs = state(&refs, 0);

        let (_, hits) = draw(40, &mut vs);
        let right = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::ScrollRight)
            .expect("右にはみ出しているのでヒントがある");
        assert_eq!(hits.at(right.0), Some(TabAction::ScrollRight));
        let hidden = hits
            .spans()
            .filter_map(|(_, _, a)| match a {
                TabAction::Select(idx) => Some(*idx),
                _ => None,
            })
            .max()
            .unwrap()
            + 1;

        // クリック = マウス処理と同じ 1 つ右へ。
        vs.tab_scroll += 1;
        let (buf, hits) = draw(40, &mut vs);
        assert!(
            hits.spans()
                .any(|(_, _, a)| *a == TabAction::Select(hidden)),
            "隠れていたタブ {hidden} が選べるようになる"
        );
        assert!(text(&buf, 40).contains(&paths[hidden]));

        // 左にもはみ出したので、戻る側のヒントも出る。
        let left = hits
            .spans()
            .find(|(_, _, a)| **a == TabAction::ScrollLeft)
            .expect("左にはみ出しているのでヒントがある");
        vs.tab_scroll = vs.tab_scroll.saturating_sub(1);
        assert_eq!(hits.at(left.0), Some(TabAction::ScrollLeft));
        let (_, hits) = draw(40, &mut vs);
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Select(0)));
    }

    /// スクロールして覗いている間は、アクティブなタブへ引き戻されない。
    #[test]
    fn スクロールがアクティブなタブに巻き戻されない() {
        let paths: Vec<String> = (0..12).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut vs = state(&refs, 0);

        draw(40, &mut vs);
        vs.tab_scroll += 1;
        let (_, hits) = draw(40, &mut vs);
        assert!(
            !hits.spans().any(|(_, _, a)| *a == TabAction::Select(0)),
            "アクティブなタブ 0 は窓の外へ出たまま"
        );

        // タブを切り替えれば戻ってくる。
        vs.tab_reveal = true;
        let (_, hits) = draw(40, &mut vs);
        assert!(hits.spans().any(|(_, _, a)| *a == TabAction::Select(0)));
    }

    /// はみ出した分は左右のヒントで数が分かる。
    #[test]
    fn 溢れは左右どちらにも印が出る() {
        let paths: Vec<String> = (0..12).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut vs = state(&refs, 6);
        let (buf, _) = draw(40, &mut vs);
        let rendered = text(&buf, 40);
        assert!(rendered.contains('\u{2039}'), "left hint: {rendered}");
        assert!(rendered.contains('\u{203a}'), "right hint: {rendered}");
    }
}
