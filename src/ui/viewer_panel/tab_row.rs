//! Viewer で開いているファイルのタブ行。
//!
//! ブロック内側の先頭行に描く。パンくずと同じ扱いで、コード行がその分だけ
//! 下がる（[super::file_view] が screen_row_map にプレースホルダを詰める）。
//!
//! タブが 1 枚だけのときは描かない — パスはタイトルに出ているので、1 行を
//! 消費するだけの価値が無い。
//!
//! クリック領域はターミナルのタブバーと同じ [TabHit] で表すので、マウス処理は
//! 幅を計算し直さずこの描画結果をそのまま引ける。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;
use crate::ui::tab_bar::{TabAction, TabHit};
use crate::ui::worktree_bar::visible_window;
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

/// タブ行を area（高さ 1）に描き、クリック領域を返す。
pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    vs: &ViewerState,
) -> Vec<TabHit> {
    let mut hits: Vec<TabHit> = Vec::new();
    if area.width == 0 || area.height == 0 || !is_visible(vs) {
        return hits;
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
    // アクティブなタブは常に見えていなければならないので、窓はそこを基準に開く。
    let (start, end) = if all_fit {
        (0, total)
    } else {
        visible_window(&slots, sep_w, avail, vs.active_tab, vs.active_tab, true)
    };

    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;

    if start > 0 {
        let hint = format!("\u{2039}{start} ");
        x += w(&hint);
        spans.push(Span::styled(hint, Style::default().fg(theme.hint)));
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
        let style = if idx == vs.active_tab {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        spans.push(Span::styled(label.clone(), style));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("[x]", Style::default().fg(theme.error)));
        hits.push(TabHit {
            x0: x,
            x1: x + label_w + 1,
            action: TabAction::Select(idx),
        });
        hits.push(TabHit {
            x0: x + label_w + 1,
            x1: x + label_w + close_w,
            action: TabAction::Close(idx),
        });
        x += label_w + close_w;
    }

    if end < total {
        spans.push(Span::styled(
            format!(" {}\u{203a}", total - end),
            Style::default().fg(theme.hint),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tab_bar::hit_at;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn state(paths: &[&str], active: usize) -> ViewerState {
        let mut vs = ViewerState::default();
        for path in paths {
            vs.tabs.push(crate::viewer::ViewerTab::for_test(path));
        }
        vs.active_tab = active;
        vs
    }

    fn draw(width: u16, vs: &ViewerState) -> (ratatui::buffer::Buffer, Vec<TabHit>) {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        let mut hits = Vec::new();
        terminal
            .draw(|f| {
                hits = render(f, f.area(), &theme, vs);
            })
            .unwrap();
        (terminal.backend().buffer().clone(), hits)
    }

    fn text(buf: &ratatui::buffer::Buffer, width: u16) -> String {
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    /// タブが 1 枚だけならタブ行は出ない。パスはタイトルに出ているので、
    /// 1 行を使う理由が無い。
    #[test]
    fn a_single_tab_does_not_take_a_row() {
        let vs = state(&["src/main.rs"], 0);
        assert!(!is_visible(&vs));
        let (_, hits) = draw(40, &vs);
        assert!(hits.is_empty());
    }

    /// 長いパスは前を削ってファイル名を残す。頭から切ると、どのタブも
    /// "src/ui/vie…" のように見分けが付かなくなる。
    #[test]
    fn long_paths_keep_their_tail() {
        assert_eq!(
            elide_head("src/ui/viewer_panel/tab_row.rs", 12),
            "\u{2026}/tab_row.rs"
        );
        assert_eq!(elide_head("a.rs", 12), "a.rs");
    }

    #[test]
    fn every_visible_tab_is_selectable_and_closable() {
        let vs = state(&["a.rs", "b.rs"], 0);
        let (_, hits) = draw(60, &vs);
        for idx in 0..2 {
            assert!(hits.iter().any(|h| h.action == TabAction::Select(idx)));
            assert!(hits.iter().any(|h| h.action == TabAction::Close(idx)));
        }
        // Select 領域のクリックはそのタブに当たる。
        let sel = hits
            .iter()
            .find(|h| h.action == TabAction::Select(1))
            .unwrap();
        assert_eq!(hit_at(&hits, sel.x0), Some(TabAction::Select(1)));
    }

    /// アクティブなタブは、はみ出していても必ず見えていなければならない —
    /// 今どのファイルを読んでいるのかが分からなくなる。
    #[test]
    fn the_active_tab_stays_visible_when_tabs_overflow() {
        let paths: Vec<String> = (0..12).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let vs = state(&refs, 11);
        let (buf, hits) = draw(40, &vs);
        assert!(hits.iter().any(|h| h.action == TabAction::Select(11)));
        assert!(text(&buf, 40).contains("file11.rs"));
    }

    /// はみ出した分は左右のヒントで数が分かる。
    #[test]
    fn overflow_is_announced_on_both_sides() {
        let paths: Vec<String> = (0..12).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let vs = state(&refs, 6);
        let (buf, _) = draw(40, &vs);
        let rendered = text(&buf, 40);
        assert!(rendered.contains('\u{2039}'), "left hint: {rendered}");
        assert!(rendered.contains('\u{203a}'), "right hint: {rendered}");
    }
}
