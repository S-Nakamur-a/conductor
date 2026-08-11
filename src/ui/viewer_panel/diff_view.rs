//! ビューアパネルの unified diff モード — プレーンファイルではなく未ステージ/
//! ステージ済み/コミット済みの変更を閲覧するときに使う GitHub 風の diff ビュー。

use crate::app::App;
use crate::theme::Theme;
use crate::viewer::UnifiedDiffEntry;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::comment_thread::{
    build_inline_compose_lines, build_inline_thread_lines, new_comment_anchor_end,
};
use super::diff_line::{DiffLineRenderCtx, render_diff_content_line};
use super::search_box::render_search_box;
use super::span_utils::digit_count;

/// hunk 区切り（hunk 間の折りたたまれたギャップ）の表示行を組み立てる。
/// 囲んでいる関数のヘッダーがあれば、それも注記として付ける。
fn render_hunk_separator(
    func_header: &Option<String>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    match func_header {
        Some(header) => {
            let prefix = " ··· ";
            let suffix = " ───";
            // 残りを ─ で埋める。
            let header_display = format!("{prefix}{header}{suffix}");
            let fill_len = width.saturating_sub(header_display.chars().count());
            let fill: String = "─".repeat(fill_len);
            Line::from(vec![
                Span::styled(prefix, Style::default().fg(theme.muted)),
                Span::styled(
                    header.clone(),
                    Style::default()
                        .fg(theme.diff_section_header)
                        .add_modifier(Modifier::ITALIC),
                ),
                Span::styled(format!("{suffix}{fill}"), Style::default().fg(theme.muted)),
            ])
        }
        None => {
            let sep = format!("{:─<width$}", " ··· ", width = width,);
            Line::from(Span::styled(sep, Style::default().fg(theme.muted)))
        }
    }
}

/// 展開可能なコンテキストブロックの表示行を組み立てる。隠れている行数と、
/// あれば関数ヘッダーを表示する。
fn render_expandable_context(
    hidden_count: usize,
    func_header: &Option<String>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let expand_label = format!(" \u{2295} {hidden_count} lines hidden (Enter to expand) ");
    let label_style = Style::default().fg(theme.accent);
    match func_header {
        Some(header) => {
            let suffix = " ───";
            let used =
                expand_label.chars().count() + header.chars().count() + suffix.chars().count();
            let fill_len = width.saturating_sub(used);
            let fill: String = "─".repeat(fill_len);
            Line::from(vec![
                Span::styled(expand_label, label_style),
                Span::styled(
                    header.clone(),
                    Style::default()
                        .fg(theme.diff_section_header)
                        .add_modifier(Modifier::ITALIC),
                ),
                Span::styled(format!("{suffix}{fill}"), Style::default().fg(theme.muted)),
            ])
        }
        None => {
            let fill_len = width.saturating_sub(expand_label.chars().count());
            let fill: String = "─".repeat(fill_len);
            Line::from(vec![
                Span::styled(expand_label, label_style),
                Span::styled(fill, Style::default().fg(theme.muted)),
            ])
        }
    }
}

/// unified diff ビュー（GitHub 風）を描画する。
pub(super) fn render_diff_view(frame: &mut Frame, area: Rect, app: &mut App, block: Block<'_>) {
    let inner_height = area.height.saturating_sub(2) as usize;

    // 表示行と、画面行→コメント/エントリのマップを組み立てる。インライン
    // コメントスレッドは、コメントされた各範囲の最終行の後に挿入される
    // （レビューコメントが diff の中にそのまま見え、デフォルトで展開された状態になる）。
    let (lines, screen_row_map, screen_entry_map) = {
        let theme = &app.theme;
        let vs = &app.viewer_state;
        let tab_width = app.config.viewer.tab_width;
        let gutter_width = digit_count(vs.diff_view.diff_view_max_line_no);

        // レビューコメントが付いている行番号（現在のファイルについて）。
        let comment_lines: std::collections::HashSet<usize> =
            app.review_state.file_comments.keys().copied().collect();
        let comment_end_lines: std::collections::HashSet<usize> = app
            .review_state
            .comments
            .iter()
            .filter(|c| vs.content.current_file.as_deref() == Some(&*c.file_path))
            .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
            .collect();
        let expanded = &vs.explorer.expanded_inline_threads;
        let inline_reply_line = vs.explorer.inline_reply_line;
        let compose_anchor_end = new_comment_anchor_end(app);

        let line_ctx = DiffLineRenderCtx {
            vs,
            theme,
            gutter_width,
            tab_width,
            area_width: area.width,
            comment_lines: &comment_lines,
            comment_end_lines: &comment_end_lines,
        };

        let mut lines: Vec<Line> = Vec::with_capacity(inner_height);
        let mut srm: Vec<crate::viewer::ScreenRow> = Vec::with_capacity(inner_height);
        let mut entry_map: Vec<Option<usize>> = Vec::with_capacity(inner_height);
        let mut remaining = inner_height;
        let scroll = vs.diff_view.diff_view_scroll;

        for (offset, entry) in vs.diff_view.diff_view_lines.iter().enumerate().skip(scroll) {
            if remaining == 0 {
                break;
            }
            let (line, new_no) = match entry {
                UnifiedDiffEntry::HunkSeparator { func_header } => {
                    let width = area.width.saturating_sub(2) as usize;
                    (render_hunk_separator(func_header, width, theme), None)
                }
                UnifiedDiffEntry::ExpandableContext {
                    hidden_count,
                    func_header,
                    ..
                } => {
                    let width = area.width.saturating_sub(2) as usize;
                    (
                        render_expandable_context(*hidden_count, func_header, width, theme),
                        None,
                    )
                }
                UnifiedDiffEntry::Line {
                    tag,
                    new_line_no,
                    content,
                    inline_segments,
                } => (
                    render_diff_content_line(tag, new_line_no, content, inline_segments, &line_ctx),
                    *new_line_no,
                ),
            };
            lines.push(line);
            srm.push(match new_no {
                Some(n) => crate::viewer::ScreenRow::Code(n),
                None => crate::viewer::ScreenRow::ThreadContent,
            });
            entry_map.push(Some(offset));
            remaining -= 1;

            // コメントの最終行の後にインラインコメントスレッドを挿入する。
            if remaining > 0
                && let Some(n) = new_no
                && comment_end_lines.contains(&n)
                && expanded.contains(&n)
            {
                let reply_cid = if inline_reply_line == Some(n) {
                    vs.explorer.inline_reply_comment_id.as_deref()
                } else {
                    None
                };
                let thread = build_inline_thread_lines(
                    n,
                    gutter_width,
                    area.width as usize,
                    &app.review_state,
                    reply_cid,
                    &vs.explorer.inline_reply_buffer,
                    theme,
                    &app.highlight.syntax_set,
                    &app.highlight.theme,
                    &app.markdown_cache,
                );
                for (l, rt) in thread {
                    if remaining == 0 {
                        break;
                    }
                    lines.push(l);
                    srm.push(rt);
                    entry_map.push(None);
                    remaining -= 1;
                }
            }

            // 新規コメント作成ボックスを、それが紐づく行の下に挿入する。
            if remaining > 0 && new_no.is_some() && compose_anchor_end == new_no {
                let compose = build_inline_compose_lines(
                    app.review_state.input_kind,
                    &app.review_state.input_buffer,
                    gutter_width,
                    area.width as usize,
                    theme,
                );
                for (l, rt) in compose {
                    if remaining == 0 {
                        break;
                    }
                    lines.push(l);
                    srm.push(rt);
                    entry_map.push(None);
                    remaining -= 1;
                }
            }
        }
        (lines, srm, entry_map)
    };

    app.viewer_state.content.screen_row_map = screen_row_map;
    app.viewer_state.diff_view.screen_entry_map = screen_entry_map;

    frame.render_widget(ratatui::widgets::Clear, area);
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);

    // 選択ヒントのオーバーレイを表示する。
    let theme = &app.theme;
    let vs = &app.viewer_state;

    // diff の行数がパネルに収まりきらない場合にスクロールバーを描画する —
    // トリガーも見た目も Explorer のファイルツリーと同じ。
    if vs.diff_view.diff_view_lines.len() > inner_height {
        let scrollbar_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        let mut scrollbar_state = ScrollbarState::new(
            vs.diff_view
                .diff_view_lines
                .len()
                .saturating_sub(inner_height),
        )
        .position(vs.diff_view.diff_view_scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
    if let Some((start, end)) = vs.selected_range() {
        let hint = if start == end {
            format!(" L{start} selected \u{2502} c: comment  Esc: clear ")
        } else {
            format!(" L{start}-L{end} selected \u{2502} c: comment  Esc: clear ")
        };
        let hint_width = hint.len().min(area.width.saturating_sub(2) as usize) as u16;
        let y = area.y + area.height.saturating_sub(2);
        let hint_area = Rect::new(area.x + 1, y, hint_width, 1);
        frame.render_widget(ratatui::widgets::Clear, hint_area);
        let hint_widget = Paragraph::new(Span::styled(
            hint,
            Style::default()
                .fg(theme.gutter_selected_fg)
                .bg(theme.gutter_selected_bg),
        ));
        frame.render_widget(hint_widget, hint_area);
    }

    // 検索入力のオーバーレイを表示する（全体オーバーレイに覆われている間はカーソル配置をしない）。
    if vs.search.search_active {
        render_search_box(
            frame,
            area,
            &vs.search.search_query,
            theme,
            app.is_any_overlay_active(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    /// 行の全 span の内容を1つの文字列に連結する。
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn hunk_separator_with_header_includes_header() {
        let theme = Theme::default();
        let line = render_hunk_separator(&Some("fn foo()".to_string()), 40, &theme);
        let text = line_text(&line);
        assert!(text.starts_with(" ··· "));
        assert!(text.contains("fn foo()"));
        // 3 つの span: prefix、header、suffix+fill。
        assert_eq!(line.spans.len(), 3);
    }

    #[test]
    fn hunk_separator_without_header_is_single_fill() {
        let theme = Theme::default();
        let line = render_hunk_separator(&None, 20, &theme);
        let text = line_text(&line);
        assert!(text.starts_with(" ··· "));
        // 要求された幅まで塗りつぶし文字で埋められている。
        assert_eq!(text.chars().count(), 20);
        assert_eq!(line.spans.len(), 1);
    }

    #[test]
    fn expandable_context_reports_hidden_count() {
        let theme = Theme::default();
        let line = render_expandable_context(7, &None, 50, &theme);
        let text = line_text(&line);
        assert!(text.contains("7 lines hidden"));
        assert!(text.contains("Enter to expand"));
    }

    #[test]
    fn expandable_context_with_header_includes_header() {
        let theme = Theme::default();
        let line = render_expandable_context(3, &Some("impl Bar".to_string()), 60, &theme);
        let text = line_text(&line);
        assert!(text.contains("3 lines hidden"));
        assert!(text.contains("impl Bar"));
        assert_eq!(line.spans.len(), 3);
    }
}
