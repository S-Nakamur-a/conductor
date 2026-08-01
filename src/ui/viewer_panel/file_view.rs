//! Default (plain/annotated file content) rendering mode of the viewer panel.
//!
//! Draws line numbers, diff gutter markers, comment markers, syntax
//! highlighting, and inline review-comment threads for the currently open
//! file. Delegates to [`super::diff_view`] for unified-diff mode and to
//! [`super::media_view`] / [`super::summary_view`] for the other pseudo-modes.

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::code_line::{render_code_line_rows, FileLineRenderCtx};
use super::comment_thread::new_comment_anchor_end;
use super::diff_view::{build_walkthrough_banner, render_diff_view, render_walkthrough_banner};
use super::markdown_view::{render_markdown_view, toggle_segments, toggle_spans, TOGGLE_W};
use super::media_view::render_media_view;
use super::search_box::render_search_box;
use super::span_utils::digit_count;
use super::summary_view::render_summary_view;
use super::syntax::ensure_diff_annotations_cached;

/// Render the viewer (file content) panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Clear screen-row map so stale data isn't used in diff/media modes.
    app.viewer_state.content.screen_row_map.clear();

    // Summary pseudo-file: the branch change summary gets the whole panel.
    // Checked before any shared borrows so the renderer can take `&mut App`.
    if app.viewer_state.is_summary() {
        let focused = app.focus == Focus::Viewer;
        render_summary_view(frame, area, app, focused);
        return;
    }

    // Populate diff annotations cache before taking any shared borrows.
    ensure_diff_annotations_cached(app);

    // Party-mode rainbow phase, advanced by the UI tick (None when off).
    let party = app.party_mode.then_some(app.ui_tick as f64 * 4.0);

    let theme = &app.theme;
    let vs = &app.viewer_state;
    let tab_width = app.config.viewer.tab_width;
    let focused = app.focus == Focus::Viewer;
    let border_color = app.animated_border_color(Focus::Viewer);

    let is_expanded = app.expanded_panel == Some(Focus::Viewer);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    // Raw/Rendered toggle — markdown files in the plain-file view only, and
    // only when the column is wide enough to hold it (`toggle_segments`, the
    // same function the click hit-test consults).
    let show_md_toggle =
        vs.markdown_toggle_available() && toggle_segments(area.x, area.width).is_some();
    let md_toggle_w = if show_md_toggle { TOGGLE_W as usize } else { 0 };

    // Truncate title so it doesn't overlap with the [<=>] button (and the
    // toggle, when shown) on the right. Reserve: 2 (borders) + toggle +
    // expand_label width + 1 (gap).
    let max_title_len =
        (area.width as usize).saturating_sub(2 + md_toggle_w + expand_label.len() + 1);
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
    // One right-aligned line holds both controls, so `[<=>]` keeps its exact
    // columns (and its existing hit-test) whether or not the toggle is drawn.
    let mut right_spans: Vec<Span> = Vec::new();
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

    // Unified diff mode: delegate to dedicated renderer. When touring a
    // walkthrough step anchored to this file, carve a full-width banner off
    // the top for the step's explanation so it reads at Viewer width instead
    // of being cramped in the Explorer pane.
    if vs.diff_view.diff_mode && !vs.diff_view.diff_view_lines.is_empty() {
        let diff_area = match build_walkthrough_banner(app, area.width) {
            Some((title, lines)) if area.height > 8 => {
                // Reserve up to ~2/5 of the Viewer height for the banner, and
                // never leave the diff fewer than 3 rows.
                let content_h = (lines.len() as u16).saturating_add(2);
                let max_h = (area.height * 2 / 5).max(4);
                let banner_h = content_h.min(max_h).min(area.height.saturating_sub(3));
                let banner_area = Rect::new(area.x, area.y, area.width, banner_h);
                let diff_area =
                    Rect::new(area.x, area.y + banner_h, area.width, area.height - banner_h);
                render_walkthrough_banner(frame, banner_area, app, &title, &lines);
                diff_area
            }
            _ => area,
        };
        render_diff_view(frame, diff_area, app, block);
        return;
    }

    // Media file mode: render image/video as ASCII art.
    if vs.is_current_file_media() {
        render_media_view(frame, area, app, block);
        return;
    }

    // Rendered-markdown mode. Returns before any of the line-oriented rendering
    // below, so `screen_row_map` stays empty (cleared at function entry) and no
    // gutter, comment marker, or thread row is ever drawn — see `markdown_view`.
    if vs.is_showing_rendered_markdown() {
        render_markdown_view(frame, area, app, block);
        return;
    }

    if vs.content.file_content.is_empty() {
        // 本文が無い理由は 3 通りあり、どれなのかを言い当てる。読めなかった場合を
        // 「ファイルを選んでください」と出すと、選んだのに反応しなかったように
        // 見えてしまう (タイトルには選択中のファイル名が出ているのに、本文は
        // 未選択の案内、という食い違いになる)。
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
        let placeholder = Paragraph::new(text).style(style).block(block);
        frame.render_widget(placeholder, area);
        return;
    }

    // Build breadcrumb trail from jump history.
    let breadcrumb_visible = build_breadcrumb_line(app);

    // Account for breadcrumb bar height (1 row when visible).
    let breadcrumb_height: u16 = if breadcrumb_visible.is_some() { 1 } else { 0 };
    let inner_height = (area.height.saturating_sub(2 + breadcrumb_height)) as usize;
    let gutter_width = digit_count(vs.content.file_content.len());

    // Diff annotations are cached in ViewerState (populated at function entry).
    let diff_annotations = app
        .viewer_state
        .content
        .cached_diff_annotations
        .as_ref()
        .unwrap();

    // Collect line numbers that have review comments (from in-memory cache).
    let comment_lines: std::collections::HashSet<usize> =
        app.review_state.file_comments.keys().copied().collect();

    // Collect the *end* lines of comments (last line of each range — where 💬 appears).
    let comment_end_lines: std::collections::HashSet<usize> = app
        .review_state
        .comments
        .iter()
        .filter(|c| app.viewer_state.content.current_file.as_deref() == Some(&*c.file_path))
        .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
        .collect();

    // Build visible lines, inserting inline thread rows after comment lines.
    let expanded_threads = &app.viewer_state.explorer.expanded_inline_threads;
    let inline_reply_line = app.viewer_state.explorer.inline_reply_line;
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
        party,
    };

    for (line_no, content) in vs
        .content
        .file_content
        .iter()
        .enumerate()
        .skip(vs.content.file_scroll)
    {
        if remaining == 0 {
            break;
        }

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

    // screen_row_map is stored into app after all borrows of vs are done (see below).

    // Prepend breadcrumb bar as the first line inside the block.
    let mut all_lines = Vec::new();
    if let Some(crumb_line) = breadcrumb_visible {
        all_lines.push(crumb_line);
    }
    all_lines.extend(lines);

    // Clear the area first to avoid stale content when scrolling.
    frame.render_widget(ratatui::widgets::Clear, area);

    let paragraph = Paragraph::new(all_lines).block(block);
    frame.render_widget(paragraph, area);

    // Render scrollbar when the file has more lines than fit in the panel —
    // same trigger and look as the Explorer file tree.
    if vs.content.file_content.len() > inner_height {
        let mut scrollbar_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        // Keep the track below the breadcrumb row so it spans only the code area.
        scrollbar_area.y += breadcrumb_height;
        scrollbar_area.height = scrollbar_area.height.saturating_sub(breadcrumb_height);
        let mut scrollbar_state =
            ScrollbarState::new(vs.content.file_content.len().saturating_sub(inner_height))
                .position(vs.content.file_scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    // Show selection hint overlay.
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

    // Show search input overlay (skip cursor positioning when a global overlay covers us).
    if vs.search.search_active {
        render_search_box(
            frame,
            area,
            &vs.search.search_query,
            theme,
            app.is_any_overlay_active(),
        );
    }

    // Store the screen-row mapping for mouse event handling.
    // Must be after all borrows of `vs` (&app.viewer_state) are dropped.
    //
    // The breadcrumb bar occupies the first inner row but is not a code line and
    // was *not* part of `screen_row_map`, so every row below it mapped one line
    // too high (clicks/hover landed a line off). Insert a non-selectable
    // placeholder so the map lines up 1:1 with what's drawn.
    if breadcrumb_height > 0 {
        screen_row_map.insert(0, crate::viewer::ScreenRow::ThreadContent);
    }
    app.viewer_state.content.screen_row_map = screen_row_map;
}

/// Fit the Viewer's title into `max_w` **display columns**, eliding from the
/// left (`" …tail "`) so the filename — the informative end of a path —
/// survives.
///
/// Columns, not bytes or chars: a CJK filename costs 2 columns per character
/// while measuring 3 bytes, so a byte- or char-budgeted cut yields a title
/// that overruns its allowance and paints over the header controls to its
/// right (the `[Raw|Rendered]` toggle and `[<=>]`). Below the width of
/// `" … "` nothing meaningful fits, so the title yields the row entirely
/// rather than overlapping.
pub(super) fn fit_title(raw: &str, max_w: usize) -> String {
    if display_width(raw) <= max_w {
        return raw.to_string();
    }
    if max_w < 4 {
        return " ".repeat(max_w);
    }
    // Leading space + ellipsis + trailing space cost 3 columns.
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

/// Build the breadcrumb `Line` from jump history + current position.
/// Returns `None` when there are fewer than 2 entries (no navigation happened).
fn build_breadcrumb_line(app: &App) -> Option<Line<'static>> {
    let current_file = app.viewer_state.content.current_file.as_ref()?;
    let current = crate::jump_history::Location {
        file_path: current_file.clone(),
        line: app.viewer_state.content.file_scroll,
        h_scroll: app.viewer_state.content.h_scroll,
    };

    let (entries, cur_idx) = app.code_nav.history.breadcrumb_trail(&current, 7);

    // Don't show breadcrumb if there's only the current entry (no navigation).
    let real_count = entries.iter().filter(|e| e.is_some()).count();
    if real_count <= 1 {
        return None;
    }

    let theme = &app.theme;
    let separator = Span::styled(" \u{203a} ", Style::default().fg(theme.muted)); // " › "
    let mut spans: Vec<Span<'static>> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(separator.clone());
        }
        match entry {
            None => {
                // Ellipsis sentinel for trimmed older entries.
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

    // Prepend a small left-padding.
    spans.insert(0, Span::raw(" "));
    Some(Line::from(spans))
}

/// Format a location as a short breadcrumb label: `filename:line`.
fn breadcrumb_label(loc: &crate::jump_history::Location) -> String {
    let filename = loc.file_path.rsplit('/').next().unwrap_or(&loc.file_path);
    format!("{}:{}", filename, loc.line + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant the header controls depend on: whatever comes back never
    /// exceeds its column budget. Anything wider paints over the toggle and the
    /// `[<=>]` button sharing that row.
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
        // Exactly at budget still passes through.
        assert_eq!(fit_title(" a.rs ", 6), " a.rs ");
    }

    /// Eliding keeps the tail, because the filename is at the end of a path.
    #[test]
    fn elision_keeps_the_end_of_the_path() {
        let fitted = fit_title(" src/ui/viewer_panel/file_view.rs ", 16);
        assert!(fitted.starts_with(" \u{2026}"), "{fitted:?}");
        assert!(fitted.ends_with("file_view.rs "), "{fitted:?}");
    }

    /// A wide-glyph title is budgeted in columns, so it elides *sooner* than a
    /// byte- or char-count would suggest — this is the case that used to
    /// overrun into the header controls.
    #[test]
    fn wide_glyphs_are_budgeted_by_column() {
        // 4 CJK chars = 8 columns, but 12 bytes.
        let fitted = fit_title(" 設計メモ.md ", 10);
        assert!(display_width(&fitted) <= 10, "{fitted:?}");
        assert!(fitted.contains('\u{2026}'), "{fitted:?}");
    }
}
