//! ビューアパネルの Markdown 描画モード — .md/.markdown ファイルをソースではなく
//! 文章として表示するモードと、両者を切り替えるヘッダーの Raw/Rendered トグル。
//!
//! 文章は SUMMARY 疑似ファイルと同じレンダラー（[crate::ui::markdown]、
//! App::markdown_cache 経由）で生成しているので、見出し・リスト・テーブル・
//! フェンス付きコードブロックはどちらの場所でも同じ見た目になる。
//!
//! **このモードには行番号がなく、したがって行単位の機能も一切ない。** 折り返し・詰め直し・
//! 省略・挿入で画面上の1行がソースの1行に対応しないため。ゲートは
//! [crate::viewer::ViewerState::is_showing_rendered_markdown]。このレンダラーが
//! content.screen_row_map を書かないので、空のマップがマウスの行検索を「行なし」に解決する。

use std::ops::Range;

use super::outcome::ScrollOutcome;
use crate::app::App;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

const RAW_LABEL: &str = "Raw";
const RENDERED_LABEL: &str = "Rendered";

/// トグルチップ [Raw|Rendered] の表示幅。
const CHIP_W: u16 = 1 + RAW_LABEL.len() as u16 + 1 + RENDERED_LABEL.len() as u16 + 1;

/// タイトル行でトグルが占める幅: チップに加えて、右側の [<=>] 展開ボタンとの
/// 間の1スペース分。
pub(crate) const TOGGLE_W: u16 = CHIP_W + 1;

/// トグルのレイアウトの基準になる [<=>] 展開ボタンの幅。このボタン自身の
/// クリック判定を持つ event::mouse::ClickGeometry::expand_button_at と
/// 同期させておくこと。
const EXPAND_BTN_W: u16 = 5;

/// トグルを表示できる Viewer 列の最小幅。これより狭いとタイトルの余地が
/// なくなるので、トグルは完全に描画しない（キーボードとパレットからは
/// 引き続きモードを切り替えられる）。
const MIN_VIEWER_W: u16 = TOGGLE_W + EXPAND_BTN_W + 8;

/// viewer_x を起点に viewer_w 幅の Viewer 列における、トグルの2つの半分の
/// 画面上の列。
pub(crate) struct ToggleSegments {
    /// 生ソースを選ぶ列（[Raw）。
    pub raw: Range<u16>,
    /// 描画済み markdown を選ぶ列（|Rendered]）。
    pub rendered: Range<u16>,
}

/// ヘッダーのトグルが位置する場所。Viewer の列が狭すぎて描画できないなら `None`。
///
/// レンダラーとクリック判定がどちらもこの 1 つの関数からレイアウトを導出するので、
/// 描画されないトグルがクリック可能になることはない。タイトル行は右寄せで、
/// [Raw|Rendered] [<=>] の並び。
pub(crate) fn toggle_segments(viewer_x: u16, viewer_w: u16) -> Option<ToggleSegments> {
    if viewer_w < MIN_VIEWER_W {
        return None;
    }
    let line_end = viewer_x + viewer_w - 1; // 排他的
    let chip_end = line_end - EXPAND_BTN_W - 1; // 排他的
    let chip_start = chip_end - CHIP_W;
    let split = chip_start + 1 + RAW_LABEL.len() as u16;
    Some(ToggleSegments {
        raw: chip_start..split,
        rendered: split..chip_end,
    })
}

/// トグルチップの span 群。呼び出し側は現在の幅で [toggle_segments] が Some である
/// ことを事前に確認していなければならない。
pub(crate) fn toggle_spans(rendered: bool, theme: &Theme) -> Vec<Span<'static>> {
    let chrome = Style::default().fg(theme.muted);
    let active = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().fg(theme.muted);
    vec![
        Span::styled("[", chrome),
        Span::styled(RAW_LABEL, if rendered { inactive } else { active }),
        Span::styled("|", chrome),
        Span::styled(RENDERED_LABEL, if rendered { active } else { inactive }),
        Span::styled("] ", chrome),
    ]
}

/// 開いている markdown ファイルをパネル全体に文章として描画する。block は Viewer 自身の
/// ブロックなので、このモードでも生ビューと同じフレームを保ち、変わるのは中身だけ。
pub(super) fn render_markdown_view(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    block: Block<'_>,
) -> ScrollOutcome {
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    let (total, scroll, visible) = {
        let key = format!(
            "viewer-md:{}",
            app.viewer.content.current_file.as_deref().unwrap_or("")
        );
        let body = app.viewer.content.file_content.join("\n");
        // スクロールバーのトラックと衝突しないよう右側に 1 列確保する (summary view に合わせる)。
        app.appearance.markdown_cache.render_window(
            &key,
            &body,
            inner_width.saturating_sub(1),
            &app.appearance.theme,
            &app.appearance.highlight.syntax_set,
            &app.appearance.highlight.theme,
            app.viewer.md_scroll,
            inner_height,
        )
    };

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Paragraph::new(visible).block(block), area);

    if total > inner_height {
        let scrollbar_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        let mut scrollbar_state =
            ScrollbarState::new(total.saturating_sub(inner_height)).position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    ScrollOutcome {
        total_lines: total,
        scroll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// チップは [<=>] 展開ボタンのすぐ左に来なければならない。重なると片方のボタンが
    /// もう片方のクリックを奪う。
    #[test]
    fn トグルは展開ボタンのすぐ左に来る() {
        let (x, w) = (40u16, 60u16);
        let seg = toggle_segments(x, w).expect("60 cols is plenty");
        let expand_start = x + w - 6;
        assert_eq!(
            seg.rendered.end + 1,
            expand_start,
            "one space must separate the chip from [<=>]"
        );
        assert!(seg.rendered.end <= expand_start);
    }

    #[test]
    fn トグルの左右は隣り合い幅も正しい() {
        let seg = toggle_segments(0, 80).unwrap();
        assert_eq!(
            seg.raw.end, seg.rendered.start,
            "no dead gap between halves"
        );
        assert_eq!(seg.raw.end - seg.raw.start, 1 + RAW_LABEL.len() as u16);
        assert_eq!(
            seg.rendered.end - seg.rendered.start,
            1 + RENDERED_LABEL.len() as u16 + 1
        );
        assert_eq!(seg.rendered.end - seg.raw.start, CHIP_W);
    }

    /// 描画されないトグルはクリックもできてはならない: レンダラーとクリック
    /// 判定はどちらも同じこの関数に尋ねているので、None は両方を同時に無効化する。
    #[test]
    fn 狭い列にはトグルを出さない() {
        assert!(toggle_segments(0, MIN_VIEWER_W - 1).is_none());
        assert!(toggle_segments(0, 0).is_none());
        assert!(toggle_segments(0, MIN_VIEWER_W).is_some());
    }

    /// 列のオフセットが何であれ、チップはパネルの内側に収まる。
    #[test]
    fn トグルは列の内側に収まる() {
        for w in [MIN_VIEWER_W, MIN_VIEWER_W + 1, 100, 300] {
            let seg = toggle_segments(7, w).unwrap();
            assert!(seg.raw.start > 7, "must not overlap the left border");
            assert!(
                seg.rendered.end < 7 + w,
                "must not overrun the right border"
            );
        }
    }

    /// file_view と全く同じ形でヘッダーを描画し、toggle_segments が主張する各セルが実際に
    /// そのチップを保持していることを確認する。右寄せタイトルの着地位置は ratatui が決める
    /// ので、単体の計算だけではズレを検出できない。CJK は 1 文字 2 カラムなので幅広も含める。
    #[test]
    fn 描いた列と当たり判定が一致する() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Alignment;
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders};

        let theme = Theme::default();
        for title in [
            " f.md ",
            " 設計メモ.md ",
            " a/very/deeply/nested/path/notes.md ",
        ] {
            // 折りたたみの段数が出てもトグルと [<=>] の列は動いてはならない。
            for fold_depth in ["", "\u{203a} 2/5 "] {
                for w in [MIN_VIEWER_W, MIN_VIEWER_W + 1, 40, 60, 120] {
                    let mut term = Terminal::new(TestBackend::new(w, 5)).unwrap();
                    // レンダラーと同じ予算: 枠線 + 段数 + トグル + [<=>] + 隙間 1 列。
                    let budget = (w as usize)
                        .saturating_sub(2 + fold_depth.chars().count() + TOGGLE_W as usize + 5 + 1);
                    let fitted = crate::viewer::render::file_view::fit_title(title, budget);
                    term.draw(|f| {
                        let mut spans = Vec::new();
                        if !fold_depth.is_empty() {
                            spans.push(Span::styled(fold_depth, Style::default()));
                        }
                        spans.extend(toggle_spans(false, &theme));
                        spans.push(Span::styled("[<=>]", Style::default()));
                        let block = Block::default()
                            .title(Span::raw(fitted.clone()))
                            .title_top(Line::from(spans).alignment(Alignment::Right))
                            .borders(Borders::ALL);
                        f.render_widget(block, Rect::new(0, 0, w, 5));
                    })
                    .unwrap();
                    let buf = term.backend().buffer().clone();
                    let seg = toggle_segments(0, w).expect("width is at least MIN_VIEWER_W");
                    let cell = |x: u16| buf[(x, 0)].symbol().to_string();
                    let ctx = format!("title={title:?} w={w} fold={fold_depth:?}");

                    assert_eq!(cell(seg.raw.start), "[", "{ctx}: raw half starts at '['");
                    assert_eq!(
                        cell(seg.rendered.start),
                        "|",
                        "{ctx}: rendered half starts at the separator"
                    );
                    assert_eq!(
                        cell(seg.rendered.end - 1),
                        "]",
                        "{ctx}: rendered half ends at ']'"
                    );
                    assert_eq!(cell(w - 6), "[", "{ctx}: [<=>] start");
                    assert_eq!(cell(w - 2), "]", "{ctx}: [<=>] end");
                }
            }
        }
    }

    #[test]
    fn いま有効なモードが強調される() {
        let theme = Theme::default();
        let raw_mode = toggle_spans(false, &theme);
        let rendered_mode = toggle_spans(true, &theme);
        assert!(raw_mode[1].style.add_modifier.contains(Modifier::BOLD));
        assert!(!raw_mode[3].style.add_modifier.contains(Modifier::BOLD));
        assert!(!rendered_mode[1].style.add_modifier.contains(Modifier::BOLD));
        assert!(rendered_mode[3].style.add_modifier.contains(Modifier::BOLD));
        // 描画される幅は、クリック判定が確保する幅と一致していなければならない。
        let drawn: usize = raw_mode
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(drawn as u16, TOGGLE_W);
    }
}
