//! ビューアパネルのデフォルトの描画モード（プレーン/注釈付きファイルの内容）。
//!
//! 現在開いているファイルについて、行番号、diff ガターマーカー、コメント
//! マーカー、シンタックスハイライト、インラインのレビューコメントスレッドを
//! 描画する。unified diff モードは [super::diff_view] に、それ以外の疑似モードは
//! [super::media_view] / [super::summary_view] に委譲する。

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::code_line::{FileLineRenderCtx, render_code_line_rows};
use super::comment_thread::new_comment_anchor_end;
use super::diff_view::render_diff_view;
use super::markdown_view::{TOGGLE_W, render_markdown_view, toggle_segments, toggle_spans};
use super::media_view::render_media_view;
use super::outcome::{RenderOutcome, TabRowOutcome};
use super::search_box::render_search_box;
use super::span_utils::digit_count;
use super::tab_row;

/// 与えられた area に Viewer (ファイル内容) パネルを描画する。
///
/// diff 注釈キャッシュの充填・カーソル行の畳み展開・宣言貼り付け行の解決はいずれも App への
/// 可変借用が要る一方、この関数は結果を返すだけにしたいので、呼び出し側が先に済ませて
/// `sticky_declaration` として渡す。
pub(in crate::viewer) fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    sticky_declaration: Option<usize>,
) -> RenderOutcome {
    let theme = &app.appearance.theme;
    let vs = &app.viewer;
    let tab_width = app.config.viewer.tab_width;
    let focused = app.focus.current() == Focus::Viewer;
    let border_color = app.animated_border_color(Focus::Viewer);

    let is_expanded = app.layout.expanded == Some(Focus::Viewer);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    // Raw/Rendered トグル。toggle_segments はクリック判定が参照するのと同じ関数。
    let show_md_toggle =
        vs.markdown_toggle_available() && toggle_segments(area.x, area.width).is_some();
    let md_toggle_w = if show_md_toggle { TOGGLE_W as usize } else { 0 };

    // 畳んだ跡からは何段目にいるかが読み取れず、ステータスの一言はすぐ消える。
    let fold_depth = vs.active_fold_depth().map(|d| {
        let arrow = crate::icons::expand_arrow(false, app.config.ui.icon_set());
        format!("{arrow} {}/{} ", d.level, d.max)
    });
    let fold_depth_w = fold_depth.as_deref().map_or(0, display_width);

    // ] ボタン（と表示されていればトグル）と重ならないようタイトルを=>確保するのは 2 (枠線) + 段数 + トグル + expand_label の幅 + 1 (隙間)。
    let max_title_len = (area.width as usize)
        .saturating_sub(2 + fold_depth_w + md_toggle_w + expand_label.len() + 1);
    let title = match &vs.content.current_file {
        Some(path) => {
            let raw = if !vs.search.search_matches.is_empty() {
                format!(
                    " {} [{}/{}] ",
                    path,
                    vs.search.search_match_idx + 1,
                    vs.search.search_matches.len()
                )
            } else if !vs.search.search_query.is_empty() {
                format!(" {path} [no matches] ")
            } else {
                format!(" {path} ")
            };
            fit_title(&raw, max_title_len)
        }
        None => " (no file selected) ".to_string(),
    };

    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let title_style = if focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    // 1 つの右寄せ行にまとめてあるので、トグルの描画有無に関わらず [<=>] は同じ列に来る。
    let mut right_spans: Vec<Span> = Vec::new();
    if let Some(label) = fold_depth {
        right_spans.push(Span::styled(label, Style::default().fg(theme.accent)));
    }
    if show_md_toggle {
        right_spans.extend(toggle_spans(vs.is_showing_rendered_markdown(), theme));
    }
    right_spans.push(Span::styled(
        expand_label,
        Style::default().fg(expand_color),
    ));
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(Line::from(right_spans).alignment(Alignment::Right))
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    if vs.diff_view.diff_mode && !vs.diff_view.diff_view_lines.is_empty() {
        let diff = render_diff_view(frame, area, app, block);
        return RenderOutcome {
            screen_row_map: Some(diff.screen_row_map),
            screen_entry_map: Some(diff.screen_entry_map),
            tab_row: diff.tab_row,
            ..Default::default()
        };
    }

    if vs.is_current_file_media() {
        render_media_view(frame, area, app, block);
        return RenderOutcome::default();
    }

    // 行単位の描画より前で return するので screen_row_map は空のままになり、ガター・
    // コメントマーカー・スレッド行は一切描画されない (markdown_view を参照)。
    if vs.is_showing_rendered_markdown() {
        let markdown_scroll = Some(render_markdown_view(frame, area, app, block));
        return RenderOutcome {
            markdown_scroll,
            ..Default::default()
        };
    }

    if vs.content.file_content.is_empty() {
        // 本文が無い理由は 3 通りある。読めなかった場合を「ファイルを選んでください」と出すと、
        // 選んだのに反応しなかったように見える。
        let (text, style) = match (&vs.content.load_error, &vs.content.current_file) {
            (Some(err), Some(path)) => (
                format!("Could not read {path}\n{err}"),
                Style::default().fg(theme.error),
            ),
            (_, Some(_)) => (
                "This file is empty.".to_string(),
                Style::default().fg(theme.muted),
            ),
            (_, None) => (
                "Select a file to view its contents.".to_string(),
                Style::default().fg(theme.muted),
            ),
        };
        let mut body = String::new();
        if tab_row::is_visible(vs) {
            body.push('\n');
        }
        body.push_str(&text);
        let placeholder = Paragraph::new(body).style(style).block(block);
        frame.render_widget(placeholder, area);
        let tab_row = render_tab_row(frame, area, theme, vs);
        return RenderOutcome {
            tab_row,
            ..Default::default()
        };
    }

    let breadcrumb_visible = build_breadcrumb_line(app);

    let sticky_visible = sticky_declaration.and_then(|line| build_sticky_line(theme, vs, line));

    // パンくずバーとタブ行の高さぶんを見込む（表示されているときは各1行）。
    let breadcrumb_height: u16 = if breadcrumb_visible.is_some() { 1 } else { 0 };
    let sticky_height: u16 = if sticky_visible.is_some() { 1 } else { 0 };
    let tab_row_height: u16 = if tab_row::is_visible(vs) { 1 } else { 0 };
    let inner_height = (area
        .height
        .saturating_sub(2 + breadcrumb_height + sticky_height + tab_row_height))
        as usize;
    let gutter_width = digit_count(vs.content.file_content.len());

    let diff_annotations = app.viewer.content.cached_diff_annotations.as_ref().unwrap();

    let comment_lines: std::collections::HashSet<usize> =
        app.review_state.file_comments.keys().copied().collect();

    // コメントの「最終」行（各範囲の最後の行 — 💬 が表示される場所）を集める。
    let comment_end_lines: std::collections::HashSet<usize> = app
        .review_state
        .comments
        .iter()
        .filter(|c| app.viewer.content.current_file.as_deref() == Some(&*c.file_path))
        .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
        .collect();

    let expanded_threads = &app.viewer.inline.expanded;
    let inline_reply_line = app.viewer.inline.reply_line;
    let compose_anchor_end = new_comment_anchor_end(app);
    let mut lines: Vec<Line> = Vec::with_capacity(inner_height);
    let mut screen_row_map: Vec<crate::viewer::ScreenRow> = Vec::with_capacity(inner_height);
    let mut remaining = inner_height;

    let line_ctx = FileLineRenderCtx {
        vs,
        theme,
        tab_width,
        area_width: area.width,
        gutter_width,
        diff_annotations,
        comment_lines: &comment_lines,
        comment_end_lines: &comment_end_lines,
    };

    // どの行が画面に出るかを決めるのは FoldState ひとつで、ここは受け取った並びを流すだけ。
    let total_lines = vs.content.file_content.len();
    for line_1 in vs
        .content
        .folds
        .visible_from(vs.content.file_scroll + 1, total_lines)
    {
        if remaining == 0 {
            break;
        }
        let line_no = line_1 - 1;
        let content = &vs.content.file_content[line_no];

        let rows = render_code_line_rows(
            app,
            &line_ctx,
            line_no,
            content,
            expanded_threads,
            inline_reply_line,
            compose_anchor_end,
        );
        for (line, row_type) in rows {
            if remaining == 0 {
                break;
            }
            lines.push(line);
            screen_row_map.push(row_type);
            remaining -= 1;
        }
    }

    // screen_row_map を app に格納するのは、vs の借用がすべて終わった後（下記参照）。

    let mut all_lines = Vec::new();
    if tab_row_height > 0 {
        // タブ行は別ウィジェットとして重ねるので、ここでは場所だけ空ける。
        all_lines.push(Line::default());
    }
    if let Some(crumb_line) = breadcrumb_visible {
        all_lines.push(crumb_line);
    }
    if let Some(sticky) = sticky_visible {
        all_lines.push(sticky);
    }
    all_lines.extend(lines);

    frame.render_widget(ratatui::widgets::Clear, area);

    let paragraph = Paragraph::new(all_lines).block(block);
    frame.render_widget(paragraph, area);

    // 尺もつまみも可視行で数える。ファイルの総行数のままだと、畳んだ直後につまみだけが
    // 縮まずに残り、どれだけ隠れているのか読めなくなる。
    let visible_total = vs.visible_line_count();
    if visible_total > inner_height {
        let mut scrollbar_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        // トラックをパンくず行より下に置き、コード領域だけをカバーするようにする。
        let head_rows = breadcrumb_height + sticky_height + tab_row_height;
        scrollbar_area.y += head_rows;
        scrollbar_area.height = scrollbar_area.height.saturating_sub(head_rows);
        let mut scrollbar_state = ScrollbarState::new(visible_total.saturating_sub(inner_height))
            .position(vs.cursor_visible_index());
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

    if vs.search.search_active {
        render_search_box(
            frame,
            area,
            &vs.search.search_query,
            theme,
            app.is_any_overlay_active(),
        );
    }

    // パンくずバーは内部の先頭行を占めるがコード行ではないので、選択不可のプレースホルダー
    // を挿入して描画内容とマップを 1 対 1 に揃える。ずれるとクリック/ホバーが 1 行ずれる。
    if sticky_height > 0 {
        screen_row_map.insert(0, crate::viewer::ScreenRow::ThreadContent);
    }
    if breadcrumb_height > 0 {
        screen_row_map.insert(0, crate::viewer::ScreenRow::ThreadContent);
    }
    if tab_row_height > 0 {
        screen_row_map.insert(0, crate::viewer::ScreenRow::ThreadContent);
    }

    let tab_row = render_tab_row(frame, area, theme, vs);
    RenderOutcome {
        screen_row_map: Some(screen_row_map),
        tab_row,
        ..Default::default()
    }
}

/// ブロック内側の先頭行にタブ行を重ね、クリック領域を返す。タブ行を描かない
/// なら None（呼び出し側は tab_row_hits/tab_scroll をそのままにする）。
pub(super) fn render_tab_row(
    frame: &mut Frame,
    area: Rect,
    theme: &crate::theme::Theme,
    vs: &crate::viewer::ViewerState,
) -> Option<TabRowOutcome> {
    if !tab_row::is_visible(vs) || area.width < 3 || area.height < 3 {
        return None;
    }
    let row = Rect::new(area.x + 1, area.y + 1, area.width - 2, 1);
    let (hits, scroll) = tab_row::render(frame, row, theme, vs);
    Some(TabRowOutcome { hits, scroll })
}

/// Viewer のタイトルを max_w 表示カラム数に収める。左側から省略し、パスの末尾
/// (ファイル名) を残す。
///
/// バイト数や文字数ではなく表示カラム数で測る。CJK は 1 文字 2 カラム 3 バイトなので、
/// バイト基準だと右側のヘッダーコントロールに重なる。" … " を下回るならタイトルの行
/// そのものを明け渡す。
pub(super) fn fit_title(raw: &str, max_w: usize) -> String {
    if display_width(raw) <= max_w {
        return raw.to_string();
    }
    if max_w < 4 {
        return " ".repeat(max_w);
    }
    // 先頭のスペース＋省略記号＋末尾のスペースで3カラムを消費する。
    let budget = max_w - 3;
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0usize;
    for ch in raw.trim_end().chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > budget {
            break;
        }
        tail.push(ch);
        used += cw;
    }
    tail.reverse();
    let tail: String = tail.into_iter().collect();
    format!(" \u{2026}{tail} ")
}

fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// 画面の外へ出た宣言行を、コードの上に貼り付けて出す。
fn build_sticky_line(
    theme: &crate::theme::Theme,
    vs: &crate::viewer::ViewerState,
    declaration: usize,
) -> Option<Line<'static>> {
    let text = vs.content.file_content.get(declaration)?;
    let style = Style::default().fg(theme.muted);
    let width = digit_count(vs.content.file_content.len());
    let marker = " ".repeat(crate::viewer::COMMENT_MARKER_W as usize);
    let gutter = format!(" {:>width$}   \u{2502}   ", declaration + 1);
    Some(Line::from(vec![
        Span::styled(format!("{marker}{gutter}"), style),
        Span::styled(text.trim_end().to_string(), style),
    ]))
}

/// ジャンプ履歴＋現在位置からパンくずの Line を組み立てる。
/// エントリが2件未満（ナビゲーションが起きていない）場合は None を返す。
fn build_breadcrumb_line(app: &App) -> Option<Line<'static>> {
    let current_file = app.viewer.content.current_file.as_ref()?;
    let current = crate::viewer::jump_history::Location {
        file_path: current_file.clone(),
        line: app.viewer.content.file_scroll,
        h_scroll: app.viewer.content.h_scroll,
    };

    let (entries, cur_idx) = app.code_nav.history.breadcrumb_trail(&current, 7);

    let real_count = entries.iter().filter(|e| e.is_some()).count();
    if real_count <= 1 {
        return None;
    }

    let theme = &app.appearance.theme;
    let separator = Span::styled(" \u{203a} ", Style::default().fg(theme.muted)); // " › "
    let mut spans: Vec<Span<'static>> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(separator.clone());
        }
        match entry {
            None => {
                spans.push(Span::styled("\u{2026}", Style::default().fg(theme.muted)));
            }
            Some(loc) => {
                let label = breadcrumb_label(loc);
                let style = if i == cur_idx {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.muted)
                };
                spans.push(Span::styled(label, style));
            }
        }
    }

    spans.insert(0, Span::raw(" "));
    Some(Line::from(spans))
}

fn breadcrumb_label(loc: &crate::viewer::jump_history::Location) -> String {
    let filename = loc.file_path.rsplit('/').next().unwrap_or(&loc.file_path);
    format!("{}:{}", filename, loc.line + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 返り値がその列の予算を超えないこと。超えるとトグルや [<=>] に重なる。
    #[test]
    fn fitted_title_never_exceeds_its_budget() {
        let titles = [
            " src/main.rs ",
            " 設計メモ.md ",
            " docs/日本語/とても長い名前のファイル.markdown ",
            " a.rs ",
            " 🦀🦀🦀/emoji.md ",
            " ",
        ];
        for t in titles {
            for max_w in 0..40usize {
                let fitted = fit_title(t, max_w);
                assert!(
                    display_width(&fitted) <= max_w,
                    "title={t:?} max_w={max_w} -> {fitted:?} ({} cols)",
                    display_width(&fitted)
                );
            }
        }
    }

    #[test]
    fn short_titles_pass_through_untouched() {
        assert_eq!(fit_title(" a.rs ", 20), " a.rs ");
        assert_eq!(fit_title(" a.rs ", 6), " a.rs ");
    }

    #[test]
    fn elision_keeps_the_end_of_the_path() {
        let fitted = fit_title(" src/viewer/render/file_view.rs ", 16);
        assert!(fitted.starts_with(" \u{2026}"), "{fitted:?}");
        assert!(fitted.ends_with("file_view.rs "), "{fitted:?}");
    }

    /// 幅広グリフのタイトルはカラム数で予算配分されるので、バイト数から想定するより早く省略される。
    #[test]
    fn wide_glyphs_are_budgeted_by_column() {
        // CJK 4文字 = 8カラムだが、12バイト。
        let fitted = fit_title(" 設計メモ.md ", 10);
        assert!(display_width(&fitted) <= 10, "{fitted:?}");
        assert!(fitted.contains('\u{2026}'), "{fitted:?}");
    }
}
