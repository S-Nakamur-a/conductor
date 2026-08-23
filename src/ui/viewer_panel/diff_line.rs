//! unified diff の1行分（コンテキスト/追加/削除の行）の描画。
//! ガター、コメントバッジ、syntax/word-diff によるスタイル付きコンテンツ、
//! GitHub 風の全幅背景塗りつぶしを含む。

use crate::diff_state::{DiffLineTag, InlineSegment};
use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::span_utils::h_scroll_spans;
use super::syntax::{merge_syntax_with_inline, render_inline_diff_spans, syntax_spans_for_line};

/// unified diff の1行分を描画するためのフレーム共有コンテキスト。
pub(super) struct DiffLineRenderCtx<'a> {
    pub(super) vs: &'a crate::viewer::ViewerState,
    pub(super) theme: &'a Theme,
    pub(super) gutter_width: usize,
    pub(super) tab_width: usize,
    pub(super) area_width: u16,
    pub(super) comment_lines: &'a std::collections::HashSet<usize>,
    pub(super) comment_end_lines: &'a std::collections::HashSet<usize>,
    pub(super) icon_set: crate::icons::IconSet,
}

/// diff の1行分（コンテキスト/追加/削除）の表示行を組み立てる。
/// ガター、コメントバッジ、syntax/word-diff によるスタイル付きコンテンツ、
/// GitHub 風の全幅背景塗りつぶしを含む。
pub(super) fn render_diff_content_line(
    tag: &DiffLineTag,
    new_line_no: &Option<usize>,
    content: &str,
    inline_segments: &[InlineSegment],
    ctx: &DiffLineRenderCtx,
) -> Line<'static> {
    let vs = ctx.vs;
    let theme = ctx.theme;
    let gutter_width = ctx.gutter_width;
    let tab_width = ctx.tab_width;
    let icon_set = ctx.icon_set;

    let is_selected = new_line_no.map(|n| vs.is_line_selected(n)).unwrap_or(false);
    let is_hovered = new_line_no
        .map(|n| vs.click.hover_line == Some(n))
        .unwrap_or(false);
    let is_gutter_hovered = new_line_no
        .map(|n| vs.click.hover_gutter_line == Some(n))
        .unwrap_or(false);
    // ガターのマーカー。
    let (gutter_prefix, diff_bg, emphasis_bg) = match tag {
        DiffLineTag::Insert => (
            "+",
            Some(theme.diff_add_bg),
            Some(theme.diff_add_bg_emphasis),
        ),
        DiffLineTag::Delete => (
            "-",
            Some(theme.diff_del_bg),
            Some(theme.diff_del_bg_emphasis),
        ),
        DiffLineTag::Equal => (" ", None, None),
    };

    // 行番号（削除行では空白）。
    let line_num_str = match new_line_no {
        Some(n) => format!("{n:>gutter_width$}"),
        None => " ".repeat(gutter_width),
    };

    // 空白2つのうち1つは、ファイル表示で折りたたみマーカーが入る列。diff 表示
    // には折りたたみが無いが、幅を揃えておかないとモードを切り替えるたびに
    // コードが1列ずれる。
    let num = format!("{gutter_prefix}{line_num_str}   \u{2502} ");
    let gutter_style = if is_selected {
        Style::default()
            .fg(theme.gutter_selected_fg)
            .bg(theme.gutter_selected_bg)
            .add_modifier(Modifier::BOLD)
    } else if is_gutter_hovered {
        Style::default()
            .fg(theme.gutter_hover_fg)
            .bg(theme.gutter_hover_bg)
    } else if is_hovered {
        Style::default().fg(theme.gutter_hover_fg)
    } else {
        match tag {
            DiffLineTag::Insert => Style::default().fg(theme.diff_add),
            DiffLineTag::Delete => Style::default().fg(theme.diff_del),
            DiffLineTag::Equal => Style::default().fg(theme.muted),
        }
    };
    let gutter_span = Span::styled(num, gutter_style);

    // コメントマーカー列（最左端、行番号より前）。プレーンビュー
    // （code_line.rs）と同じ並びで、コメント範囲の終端・範囲の途中・選択範囲の
    // 終端・コメント開始ボタンの順に優先する。
    let range_end = vs
        .selected_range()
        .filter(|(start, end)| end > start)
        .is_some_and(|(_, end)| *new_line_no == Some(end));
    let accent_marker = Style::default().fg(theme.accent);
    let marker = if new_line_no.is_some_and(|n| ctx.comment_end_lines.contains(&n)) {
        Span::styled(crate::icons::COMMENT.labeled(icon_set), accent_marker)
    } else if new_line_no.is_some_and(|n| ctx.comment_lines.contains(&n)) {
        Span::styled(crate::icons::COMMENT_SPAN.labeled(icon_set), accent_marker)
    } else if range_end {
        Span::styled(crate::icons::RANGE_END.labeled(icon_set), accent_marker)
    } else if is_gutter_hovered {
        Span::styled(
            crate::icons::ADD_COMMENT.labeled(icon_set),
            accent_marker.add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("  ")
    };

    // バッジ列（行番号の右）。diff ビューにはテスト実行マーカーが無いので常に空。
    let badge = Span::raw("  ");

    // コンテンツのスタイリング。
    let content_spans: Vec<Span> = if is_selected {
        vec![Span::styled(
            content.to_string(),
            Style::default()
                .bg(theme.line_selected_bg)
                .fg(theme.line_selected_fg),
        )]
    } else if !inline_segments.is_empty() {
        match tag {
            DiffLineTag::Insert => {
                // シンタックスハイライトと word-diff のマージを試みる。
                if let Some(line_no) = new_line_no {
                    let idx = line_no - 1;
                    vs.content
                        .highlighted_lines
                        .get(idx)
                        .filter(|t| !t.is_empty())
                        .and_then(|tokens| {
                            merge_syntax_with_inline(
                                inline_segments,
                                tokens,
                                diff_bg.unwrap_or(Color::Reset),
                                emphasis_bg.unwrap_or(Color::Reset),
                                tab_width,
                            )
                        })
                        .unwrap_or_else(|| {
                            render_inline_diff_spans(
                                inline_segments,
                                diff_bg.unwrap_or(Color::Reset),
                                emphasis_bg.unwrap_or(Color::Reset),
                                theme.fg,
                                tab_width,
                            )
                        })
                } else {
                    render_inline_diff_spans(
                        inline_segments,
                        diff_bg.unwrap_or(Color::Reset),
                        emphasis_bg.unwrap_or(Color::Reset),
                        theme.fg,
                        tab_width,
                    )
                }
            }
            DiffLineTag::Delete => render_inline_diff_spans(
                inline_segments,
                diff_bg.unwrap_or(Color::Reset),
                emphasis_bg.unwrap_or(Color::Reset),
                theme.fg,
                tab_width,
            ),
            DiffLineTag::Equal => {
                if let Some(line_no) = new_line_no {
                    syntax_spans_for_line(vs, line_no - 1, None, theme.fg)
                } else {
                    vec![Span::styled(
                        content.to_string(),
                        Style::default().fg(theme.fg),
                    )]
                }
            }
        }
    } else {
        // インラインセグメントがない場合はシンタックスハイライトかプレーン表示を使う。
        match tag {
            DiffLineTag::Insert => {
                if let Some(line_no) = new_line_no {
                    syntax_spans_for_line(vs, line_no - 1, diff_bg, theme.fg)
                } else {
                    vec![Span::styled(
                        content.to_string(),
                        Style::default()
                            .fg(theme.fg)
                            .bg(diff_bg.unwrap_or(Color::Reset)),
                    )]
                }
            }
            DiffLineTag::Delete => {
                vec![Span::styled(
                    content.to_string(),
                    Style::default()
                        .fg(theme.fg)
                        .bg(diff_bg.unwrap_or(Color::Reset)),
                )]
            }
            DiffLineTag::Equal => {
                if let Some(line_no) = new_line_no {
                    syntax_spans_for_line(vs, line_no - 1, None, theme.fg)
                } else {
                    vec![Span::styled(
                        content.to_string(),
                        Style::default().fg(theme.fg),
                    )]
                }
            }
        }
    };

    // 水平スクロールを適用し、パネル幅（枠線 + マーカー列 + ガター + バッジ）でクリップする。
    let content_max_w = (ctx.area_width as usize)
        .saturating_sub(crate::viewer::COMMENT_MARKER_W as usize + gutter_width + crate::viewer::GUTTER_FIXED_W + 4);
    let content_spans = h_scroll_spans(content_spans, vs.content.h_scroll, content_max_w);

    let mut spans = vec![marker, gutter_span, badge];
    spans.extend(content_spans);

    // Insert/Delete 行では、背景色を行末まで伸ばす（GitHub 風のブロック塗り）。
    if let Some(bg) = diff_bg {
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let panel_inner_w = ctx.area_width.saturating_sub(2) as usize;
        if used < panel_inner_w {
            let fill = " ".repeat(panel_inner_w - used);
            spans.push(Span::styled(fill, Style::default().bg(bg)));
        }
    }

    Line::from(spans)
}
