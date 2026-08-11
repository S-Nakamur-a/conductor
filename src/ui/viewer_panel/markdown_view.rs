//! ビューアパネルの Markdown 描画モード — .md/.markdown ファイルをソースではなく
//! 文章として表示するモードと、両者を切り替えるヘッダーの Raw/Rendered トグル。
//!
//! 文章は SUMMARY 疑似ファイルと同じレンダラー（[crate::ui::markdown]、
//! App::markdown_cache 経由）で生成しているので、見出し・リスト・テーブル・
//! フェンス付きコードブロックはどちらの場所でも同じ見た目になる。
//!
//! **このモードには行番号がなく、したがって行単位の機能も一切ない。**
//! Markdown の描画は行の折り返し・詰め直し・省略・挿入を行うので、画面上の1行が
//! もはやソースの1行に対応しない: ガター、行選択、ホバーハイライト、コメント
//! 作成、インラインコメントスレッド、行に紐づいたジャンプはすべてここでは
//! 無意味であり、それぞれの発生源で無効化されている — 参照先は
//! [crate::viewer::ViewerState::is_showing_rendered_markdown] で、上記すべてが
//! これをゲートにしている。特筆すべきは、このレンダラーが
//! content.screen_row_map を一切書き込まないことで（[super::render] が入口で
//! これをクリアする）、その空のマップこそがマウスの行検索をすべて「行なし」に
//! 解決させている。

use std::ops::Range;

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

/// ヘッダーのトグルが位置する場所。Viewer の列が狭すぎて描画できない場合は
/// None。
///
/// レンダラー（[toggle_spans]。これが Some を返すことをゲートにしている）と
/// event/mouse 側のクリック判定は、どちらもこの1つの関数からレイアウトを
/// 導出しているので、描画されないトグルが誤ってクリック可能になることは
/// なく、描画されたトグルは常にその見た目どおりの位置でクリックできる。
///
/// タイトル行は右寄せで、ブロックの右枠の内側1セルで終わる。レイアウトは
/// [Raw|Rendered] [<=>] の並び。
pub(crate) fn toggle_segments(viewer_x: u16, viewer_w: u16) -> Option<ToggleSegments> {
    if viewer_w < MIN_VIEWER_W {
        return None;
    }
    // 右寄せタイトル行の最後の描画可能セルから始めて、展開ボタンと区切りの
    // スペース分だけ左に進み、チップ自身の右端に着地する。
    let line_end = viewer_x + viewer_w - 1; // 排他的
    let chip_end = line_end - EXPAND_BTN_W - 1; // 排他的
    let chip_start = chip_end - CHIP_W;
    // | の位置で分割する: "[Raw" は raw を選び、"|Rendered]" は rendered を選ぶ。
    let split = chip_start + 1 + RAW_LABEL.len() as u16;
    Some(ToggleSegments {
        raw: chip_start..split,
        rendered: split..chip_end,
    })
}

/// トグルチップの span 群。アクティブなモードをハイライトした状態で、
/// Viewer の右寄せタイトル行にそのまま追加できる形になっている。呼び出し側は
/// 現在の幅で [toggle_segments] が Some であることを事前に確認していなければ
/// ならない。
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

/// 開いている markdown ファイルをパネル全体に文章として描画する。
///
/// block は Viewer 自身のブロック（タイトル＋トグルはすでに乗っている）なので、
/// このモードでも生ビューと同じフレームを保ち、変わるのは中身だけになる。
pub(super) fn render_markdown_view(frame: &mut Frame, area: Rect, app: &mut App, block: Block<'_>) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    let (total, scroll, visible) = {
        let key = format!(
            "viewer-md:{}",
            app.viewer_state
                .content
                .current_file
                .as_deref()
                .unwrap_or("")
        );
        let body = app.viewer_state.content.file_content.join("\n");
        // 折り返された文章がスクロールバーのトラックと決して衝突しないよう
        // 右側に1列確保する（summary view のインセットに合わせている）。
        app.markdown_cache.render_window(
            &key,
            &body,
            inner_width.saturating_sub(1),
            &app.theme,
            &app.highlight.syntax_set,
            &app.highlight.theme,
            app.viewer_state.md_scroll,
            inner_height,
        )
    };

    // キー入力ハンドラがスクロールをクランプできるよう総行数を記録し、
    // ドキュメントが縮んだ（あるいはパネルが広がって再折り返しで短くなった）
    // 場合でもナビゲーションが応答し続けるよう、クランプ済みのスクロール位置を
    // 書き戻す。
    app.viewer_state.md_total_lines = total;
    app.viewer_state.md_scroll = scroll;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// チップは [<=>] 展開ボタンのすぐ左に来なければならない。展開ボタンの
    /// クリック判定（expand_button_at）は、その列の右端の2つ手前で終わる
    /// 5セルを占有する。重なると、片方のボタンがもう片方のクリックを
    /// 奪ってしまう。
    #[test]
    fn toggle_sits_just_left_of_the_expand_button() {
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
    fn toggle_halves_are_adjacent_and_correctly_sized() {
        let seg = toggle_segments(0, 80).unwrap();
        assert_eq!(
            seg.raw.end, seg.rendered.start,
            "no dead gap between halves"
        );
        // "[Raw" と "|Rendered]"。
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
    fn narrow_columns_get_no_toggle() {
        assert!(toggle_segments(0, MIN_VIEWER_W - 1).is_none());
        assert!(toggle_segments(0, 0).is_none());
        assert!(toggle_segments(0, MIN_VIEWER_W).is_some());
    }

    /// 列のオフセットが何であれ、チップはパネルの内側に収まる。
    #[test]
    fn toggle_stays_within_the_column() {
        for w in [MIN_VIEWER_W, MIN_VIEWER_W + 1, 100, 300] {
            let seg = toggle_segments(7, w).unwrap();
            assert!(seg.raw.start > 7, "must not overlap the left border");
            assert!(
                seg.rendered.end < 7 + w,
                "must not overrun the right border"
            );
        }
    }

    /// 決定的なチェック: file_view と全く同じ形でヘッダーを描画し、
    /// toggle_segments が主張する各セルが実際にそのチップの部分を保持している
    /// ことを確認する。両者の算術上の一致だけでは不十分である — 右寄せタイトルが
    /// 実際にどこに着地するかは ratatui が決めており、そこにズレがあれば
    /// クリックは誤った半分（あるいは [<=>] ボタン）に送られてしまうが、
    /// 単体の計算だけではそれを検出できない。タイトルには幅広グリフのケースも
    /// 含める。CJK のファイル名は1文字あたり2カラムを消費するため。
    #[test]
    fn drawn_columns_match_the_hit_test() {
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
            for w in [MIN_VIEWER_W, MIN_VIEWER_W + 1, 40, 60, 120] {
                let mut term = Terminal::new(TestBackend::new(w, 5)).unwrap();
                // レンダラーがタイトルに割り当てるのと同じ予算: 枠線 + トグル +
                // [<=>] + 隙間1列。
                let budget = (w as usize).saturating_sub(2 + TOGGLE_W as usize + 5 + 1);
                let fitted = crate::ui::viewer_panel::file_view::fit_title(title, budget);
                term.draw(|f| {
                    let mut spans = toggle_spans(false, &theme);
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
                let ctx = format!("title={title:?} w={w}");

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
                // そして展開ボタンも、自身のクリック判定が言う位置に本当に存在する。
                assert_eq!(cell(w - 6), "[", "{ctx}: [<=>] start");
                assert_eq!(cell(w - 2), "]", "{ctx}: [<=>] end");
            }
        }
    }

    #[test]
    fn spans_highlight_the_active_mode() {
        let theme = Theme::default();
        let raw_mode = toggle_spans(false, &theme);
        let rendered_mode = toggle_spans(true, &theme);
        // インデックス1が "Raw"、インデックス3が "Rendered"。
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
