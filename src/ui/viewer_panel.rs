//! Viewer panel — file content display with diff highlights and review comments.
//!
//! Shows the content of the selected file in the middle column. Lines that
//! have been modified (according to diff_state) are highlighted inline.
//! Review comments are shown as inline badges.

use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use crate::app::{App, Focus};
use crate::diff_state::{DiffLineTag, InlineSegment};
use crate::media_state::MediaContent;
use crate::review_state::ReviewInputMode;
use crate::review_store::ReviewComment;
use crate::theme::Theme;
use crate::viewer::UnifiedDiffEntry;

/// Render the viewer (file content) panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Populate diff annotations cache before taking any shared borrows.
    ensure_diff_annotations_cached(app);

    let theme = &app.theme;
    let vs = &app.viewer_state;
    let tab_width = app.config.viewer.tab_width;
    let focused = app.focus == Focus::Viewer;
    let border_color = if focused { theme.border_focused } else { theme.border_unfocused };

    let is_expanded = app.expanded_panel == Some(Focus::Viewer);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    // Truncate title so it doesn't overlap with the [<=>] button on the right.
    // Reserve: 2 (borders) + expand_label width + 1 (gap).
    let max_title_len = (area.width as usize).saturating_sub(2 + expand_label.len() + 1);
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
            if raw.len() > max_title_len && max_title_len > 4 {
                // Truncate with ellipsis: " …<tail> "
                let inner_max = max_title_len.saturating_sub(2); // leading/trailing spaces
                let tail: String = raw.trim().chars().rev().take(inner_max.saturating_sub(1)).collect::<Vec<_>>().into_iter().rev().collect();
                format!(" \u{2026}{tail} ")
            } else {
                raw
            }
        }
        None => " (no file selected) ".to_string(),
    };

    let block = Block::default()
        .title(title)
        .title_top(Line::from(Span::styled(expand_label, Style::default().fg(expand_color))).alignment(Alignment::Right))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    // Unified diff mode: delegate to dedicated renderer.
    if vs.diff_view.diff_mode && !vs.diff_view.diff_view_lines.is_empty() {
        render_diff_view(frame, area, app, block);
        return;
    }

    // Media file mode: render image/video as ASCII art.
    if vs.is_current_file_media() {
        render_media_view(frame, area, app, block);
        return;
    }

    if vs.content.file_content.is_empty() {
        let placeholder = Paragraph::new("Select a file to view its contents.")
            .style(Style::default().fg(theme.muted))
            .block(block);
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
    let diff_annotations = app.viewer_state.content.cached_diff_annotations.as_ref().unwrap();

    // Collect line numbers that have review comments (from in-memory cache).
    let comment_lines: std::collections::HashSet<usize> =
        app.review_state.file_comments.keys().copied().collect();

    let lines: Vec<Line> = vs
        .content.file_content
        .iter()
        .enumerate()
        .skip(vs.content.file_scroll)
        .take(inner_height)
        .map(|(line_no, content)| {
            let line_1 = line_no + 1;
            let is_selected = vs.is_line_selected(line_1);
            let is_hovered = vs.click.hover_line == Some(line_1);
            let is_gutter_hovered = vs.click.hover_gutter_line == Some(line_1);
            let is_in_pending_range = !is_selected && vs.selection.selected_line_start.is_some() && vs.selection.selected_line_end.is_none() && vs.click.hover_line.is_some() && {
                let start = vs.selection.selected_line_start.unwrap();
                let hover = vs.click.hover_line.unwrap();
                let (lo, hi) = if start <= hover { (start, hover) } else { (hover, start) };
                line_1 >= lo && line_1 <= hi
            };

            // Diff gutter marker.
            let annotation = diff_annotations.get(&line_1);
            let diff_tag = annotation.map(|(tag, _)| *tag);
            let (gutter_prefix, gutter_bg) = match diff_tag {
                Some(DiffLineTag::Insert) => ("+", Some(app.theme.diff_add_bg)),
                Some(DiffLineTag::Delete) => ("-", None),
                _ => (" ", None),
            };

            // Gutter (line number).
            let num = format!("{gutter_prefix}{line_1:>gutter_width$} \u{2502} ");
            let is_grep_highlight = vs.content.grep_highlight_line == Some(line_1);
            let gutter_style = if is_selected {
                Style::default()
                    .fg(theme.gutter_selected_fg)
                    .bg(theme.gutter_selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else if is_in_pending_range {
                Style::default()
                    .fg(theme.gutter_selected_fg)
                    .bg(theme.gutter_pending_bg)
            } else if is_grep_highlight {
                Style::default()
                    .fg(theme.search_current_fg)
                    .bg(theme.search_match_bg)
                    .add_modifier(Modifier::BOLD)
            } else if is_gutter_hovered {
                Style::default().fg(theme.gutter_hover_fg).bg(theme.gutter_hover_bg)
            } else if is_hovered {
                Style::default().fg(theme.gutter_hover_fg)
            } else if diff_tag == Some(DiffLineTag::Insert) {
                Style::default().fg(theme.diff_add)
            } else if diff_tag == Some(DiffLineTag::Delete) {
                Style::default().fg(theme.diff_del)
            } else {
                Style::default().fg(theme.muted)
            };
            let gutter_span = Span::styled(num, gutter_style);

            // Comment badge.
            let badge = if comment_lines.contains(&line_1) {
                Span::styled("\u{25c6} ", Style::default().fg(theme.accent))
            } else {
                Span::raw("  ")
            };

            // Content styling.
            let is_match = vs.search.search_matches.contains(&line_no);
            let is_current_match =
                vs.search.search_matches.get(vs.search.search_match_idx) == Some(&line_no);

            let content_spans: Vec<Span> = if is_current_match {
                vec![Span::styled(
                    content.to_string(),
                    Style::default().fg(theme.search_current_fg).bg(theme.search_match_bg),
                )]
            } else if is_match {
                vec![Span::styled(
                    content.to_string(),
                    Style::default()
                        .fg(theme.search_match_fg)
                        .add_modifier(Modifier::BOLD),
                )]
            } else if is_selected {
                vec![Span::styled(
                    content.to_string(),
                    Style::default().bg(theme.line_selected_bg).fg(theme.line_selected_fg),
                )]
            } else if is_in_pending_range {
                vec![Span::styled(
                    content.to_string(),
                    Style::default().bg(theme.line_pending_bg).fg(theme.line_selected_fg),
                )]
            } else if let Some((ann_tag, ann_segments)) = annotation {
                if !ann_segments.is_empty() {
                    // Word-level diff: render each segment with appropriate background.
                    let (diff_bg, emphasis_bg) = match ann_tag {
                        DiffLineTag::Insert => (app.theme.diff_add_bg, app.theme.diff_add_bg_emphasis),
                        DiffLineTag::Delete => (app.theme.diff_del_bg, app.theme.diff_del_bg_emphasis),
                        _ => (Color::Reset, Color::Reset),
                    };

                    if *ann_tag == DiffLineTag::Insert {
                        vs.content.highlighted_lines.get(line_no)
                            .filter(|t| !t.is_empty())
                            .and_then(|tokens| merge_syntax_with_inline(
                                ann_segments, tokens, diff_bg, emphasis_bg, tab_width,
                            ))
                            .unwrap_or_else(|| syntax_spans_for_line(vs, line_no, Some(diff_bg)))
                    } else {
                        render_inline_diff_spans(
                            ann_segments,
                            diff_bg,
                            emphasis_bg,
                            tab_width,
                        )
                    }
                } else {
                    // Line-level diff only: use syntax highlighting with diff bg.
                    let diff_bg = match ann_tag {
                        DiffLineTag::Insert => Some(app.theme.diff_add_bg),
                        DiffLineTag::Delete => Some(app.theme.diff_del_bg),
                        _ => None,
                    };
                    syntax_spans_for_line(vs, line_no, diff_bg)
                }
            } else {
                syntax_spans_for_line(vs, line_no, gutter_bg)
            };

            // Apply horizontal scroll to content spans, clipping to panel width.
            let content_max_w = (area.width as usize).saturating_sub(gutter_width + 8);
            let content_spans = h_scroll_spans(content_spans, vs.content.h_scroll, content_max_w);

            // Apply underline to hover symbol (Cmd+hover for jump targets).
            let content_spans = if let Some(ref hs) = vs.click.hover_symbol {
                if hs.line == line_1 {
                    apply_underline_range(content_spans, hs.start_col, hs.end_col, vs.content.h_scroll, theme.accent)
                } else {
                    content_spans
                }
            } else {
                content_spans
            };

            // Apply symbol hint labels (Vimium-style).
            let content_spans = if app.symbol_hint_overlay.active {
                let hints_on_line: Vec<_> = app.symbol_hint_overlay.hints.iter()
                    .filter(|h| h.line == line_1)
                    .collect();
                if hints_on_line.is_empty() {
                    content_spans
                } else {
                    apply_hint_labels(content_spans, &hints_on_line, &app.symbol_hint_overlay.input, vs.content.h_scroll, theme)
                }
            } else {
                content_spans
            };

            let mut spans = vec![gutter_span, badge];
            spans.extend(content_spans);
            Line::from(spans)
        })
        .collect();

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
            Style::default().fg(theme.gutter_selected_fg).bg(theme.gutter_selected_bg),
        ));
        frame.render_widget(hint_widget, hint_area);
    }

    // Show comment preview overlay when the cursor line has comments.
    if !vs.search.search_active && app.review_state.input_mode == ReviewInputMode::Normal {
        let cursor_line = if let Some(line) = vs.explorer.comment_preview_line {
            line
        } else if let Some((start, _)) = vs.selected_range() {
            start
        } else {
            vs.content.file_scroll + 1
        };
        if let Some(comments) = app.review_state.file_comments.get(&cursor_line) {
            let has_selection = vs.selected_range().is_some();
            render_comment_preview(
                frame,
                area,
                comments,
                cursor_line,
                has_selection,
                &app.review_state.reply_counts,
                theme,
            );
        }
    }

    // Show search input overlay (skip cursor positioning when a global overlay covers us).
    if vs.search.search_active {
        render_search_box(frame, area, &vs.search.search_query, theme, app.is_any_overlay_active());
    }
}

/// Render the unified diff view (GitHub-style).
fn render_diff_view(frame: &mut Frame, area: Rect, app: &App, block: Block<'_>) {
    let theme = &app.theme;
    let vs = &app.viewer_state;
    let tab_width = app.config.viewer.tab_width;
    let inner_height = area.height.saturating_sub(2) as usize;

    // Use cached max line number (computed in build_unified_diff_view).
    let gutter_width = digit_count(vs.diff_view.diff_view_max_line_no);

    // Collect line numbers that have review comments.
    let comment_lines: std::collections::HashSet<usize> =
        app.review_state.file_comments.keys().copied().collect();

    let lines: Vec<Line> = vs
        .diff_view.diff_view_lines
        .iter()
        .skip(vs.diff_view.diff_view_scroll)
        .take(inner_height)
        .map(|entry| {
            match entry {
                UnifiedDiffEntry::HunkSeparator { func_header } => {
                    let width = area.width.saturating_sub(2) as usize;
                    match func_header {
                        Some(header) => {
                            let prefix = " ··· ";
                            let suffix = " ───";
                            // Fill the rest with ─
                            let header_display = format!("{prefix}{header}{suffix}");
                            let fill_len = width.saturating_sub(header_display.chars().count());
                            let fill: String = "─".repeat(fill_len);
                            Line::from(vec![
                                Span::styled(prefix, Style::default().fg(theme.muted)),
                                Span::styled(
                                    header.clone(),
                                    Style::default().fg(theme.diff_section_header).add_modifier(Modifier::ITALIC),
                                ),
                                Span::styled(
                                    format!("{suffix}{fill}"),
                                    Style::default().fg(theme.muted),
                                ),
                            ])
                        }
                        None => {
                            let sep = format!(
                                "{:─<width$}",
                                " ··· ",
                                width = width,
                            );
                            Line::from(Span::styled(
                                sep,
                                Style::default().fg(theme.muted),
                            ))
                        }
                    }
                }
                UnifiedDiffEntry::Line {
                    tag,
                    new_line_no,
                    content,
                    inline_segments,
                } => {
                    let is_selected = new_line_no
                        .map(|n| vs.is_line_selected(n))
                        .unwrap_or(false);
                    let is_hovered = new_line_no
                        .map(|n| vs.click.hover_line == Some(n))
                        .unwrap_or(false);
                    let is_gutter_hovered = new_line_no
                        .map(|n| vs.click.hover_gutter_line == Some(n))
                        .unwrap_or(false);
                    let is_in_pending_range = !is_selected && new_line_no.is_some() && vs.selection.selected_line_start.is_some() && vs.selection.selected_line_end.is_none() && vs.click.hover_line.is_some() && {
                        let n = new_line_no.unwrap();
                        let start = vs.selection.selected_line_start.unwrap();
                        let hover = vs.click.hover_line.unwrap();
                        let (lo, hi) = if start <= hover { (start, hover) } else { (hover, start) };
                        n >= lo && n <= hi
                    };

                    // Gutter marker.
                    let (gutter_prefix, diff_bg, emphasis_bg) = match tag {
                        DiffLineTag::Insert => ("+", Some(app.theme.diff_add_bg), Some(app.theme.diff_add_bg_emphasis)),
                        DiffLineTag::Delete => ("-", Some(app.theme.diff_del_bg), Some(app.theme.diff_del_bg_emphasis)),
                        DiffLineTag::Equal => (" ", None, None),
                    };

                    // Line number (blank for Delete lines).
                    let line_num_str = match new_line_no {
                        Some(n) => format!("{n:>gutter_width$}"),
                        None => " ".repeat(gutter_width),
                    };

                    let num = format!("{gutter_prefix}{line_num_str} \u{2502} ");
                    let gutter_style = if is_selected {
                        Style::default()
                            .fg(theme.gutter_selected_fg)
                            .bg(theme.gutter_selected_bg)
                            .add_modifier(Modifier::BOLD)
                    } else if is_in_pending_range {
                        Style::default()
                            .fg(theme.gutter_selected_fg)
                            .bg(theme.gutter_pending_bg)
                    } else if is_gutter_hovered {
                        Style::default().fg(theme.gutter_hover_fg).bg(theme.gutter_hover_bg)
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

                    // Comment badge (only for lines with new_line_no).
                    let badge = if new_line_no.is_some_and(|n| comment_lines.contains(&n)) {
                        Span::styled("\u{25c6} ", Style::default().fg(theme.accent))
                    } else {
                        Span::raw("  ")
                    };

                    // Content styling.
                    let content_spans: Vec<Span> = if is_selected {
                        vec![Span::styled(
                            content.clone(),
                            Style::default().bg(theme.line_selected_bg).fg(theme.line_selected_fg),
                        )]
                    } else if is_in_pending_range {
                        vec![Span::styled(
                            content.clone(),
                            Style::default().bg(theme.line_pending_bg).fg(theme.line_selected_fg),
                        )]
                    } else if !inline_segments.is_empty() {
                        match tag {
                            DiffLineTag::Insert => {
                                // Try syntax highlighting + word-diff merge.
                                if let Some(line_no) = new_line_no {
                                    let idx = line_no - 1;
                                    vs.content.highlighted_lines.get(idx)
                                        .filter(|t| !t.is_empty())
                                        .and_then(|tokens| merge_syntax_with_inline(
                                            inline_segments, tokens,
                                            diff_bg.unwrap_or(Color::Reset),
                                            emphasis_bg.unwrap_or(Color::Reset),
                                            tab_width,
                                        ))
                                        .unwrap_or_else(|| render_inline_diff_spans(
                                            inline_segments,
                                            diff_bg.unwrap_or(Color::Reset),
                                            emphasis_bg.unwrap_or(Color::Reset),
                                            tab_width,
                                        ))
                                } else {
                                    render_inline_diff_spans(
                                        inline_segments,
                                        diff_bg.unwrap_or(Color::Reset),
                                        emphasis_bg.unwrap_or(Color::Reset),
                                        tab_width,
                                    )
                                }
                            }
                            DiffLineTag::Delete => {
                                render_inline_diff_spans(
                                    inline_segments,
                                    diff_bg.unwrap_or(Color::Reset),
                                    emphasis_bg.unwrap_or(Color::Reset),
                                    tab_width,
                                )
                            }
                            DiffLineTag::Equal => {
                                if let Some(line_no) = new_line_no {
                                    syntax_spans_for_line(vs, line_no - 1, None)
                                } else {
                                    vec![Span::styled(content.clone(), Style::default().fg(theme.fg))]
                                }
                            }
                        }
                    } else {
                        // No inline segments — use syntax highlighting or plain.
                        match tag {
                            DiffLineTag::Insert => {
                                if let Some(line_no) = new_line_no {
                                    syntax_spans_for_line(vs, line_no - 1, diff_bg)
                                } else {
                                    vec![Span::styled(
                                        content.clone(),
                                        Style::default().fg(theme.fg).bg(diff_bg.unwrap_or(Color::Reset)),
                                    )]
                                }
                            }
                            DiffLineTag::Delete => {
                                vec![Span::styled(
                                    content.clone(),
                                    Style::default().fg(theme.fg).bg(diff_bg.unwrap_or(Color::Reset)),
                                )]
                            }
                            DiffLineTag::Equal => {
                                if let Some(line_no) = new_line_no {
                                    syntax_spans_for_line(vs, line_no - 1, None)
                                } else {
                                    vec![Span::styled(content.clone(), Style::default().fg(theme.fg))]
                                }
                            }
                        }
                    };

                    // Apply horizontal scroll, clipping to panel width.
                    let content_max_w = (area.width as usize).saturating_sub(gutter_width + 8);
                    let content_spans = h_scroll_spans(content_spans, vs.content.h_scroll, content_max_w);

                    let mut spans = vec![gutter_span, badge];
                    spans.extend(content_spans);
                    Line::from(spans)
                }
            }
        })
        .collect();

    frame.render_widget(ratatui::widgets::Clear, area);

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);

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
            Style::default().fg(theme.gutter_selected_fg).bg(theme.gutter_selected_bg),
        ));
        frame.render_widget(hint_widget, hint_area);
    }

    // Show comment preview overlay.
    if !vs.search.search_active && app.review_state.input_mode == ReviewInputMode::Normal {
        let cursor_line = if let Some(line) = vs.explorer.comment_preview_line {
            line
        } else if let Some((start, _)) = vs.selected_range() {
            start
        } else {
            // Determine current line from diff_view_scroll position.
            vs.diff_view.diff_view_lines.get(vs.diff_view.diff_view_scroll)
                .and_then(|e| match e {
                    UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
                    _ => None,
                })
                .unwrap_or(0)
        };
        if cursor_line > 0 {
            if let Some(comments) = app.review_state.file_comments.get(&cursor_line) {
                let has_selection = vs.selected_range().is_some();
                render_comment_preview(
                    frame,
                    area,
                    comments,
                    cursor_line,
                    has_selection,
                    &app.review_state.reply_counts,
                    theme,
                );
            }
        }
    }
}

/// Render media file (image/video) as ASCII art in the viewer panel.
fn render_media_view(frame: &mut Frame, area: Rect, app: &App, block: Block<'_>) {
    let theme = &app.theme;
    let vs = &app.viewer_state;

    let content = vs.media_state.content.lock().unwrap().clone();

    match content {
        MediaContent::Loading => {
            let loading = Paragraph::new("Loading media...")
                .style(Style::default().fg(theme.muted))
                .block(block);
            frame.render_widget(loading, area);
        }
        MediaContent::Rendered { lines, dimensions, file_size } => {
            frame.render_widget(ratatui::widgets::Clear, area);

            let inner = block.inner(area);
            frame.render_widget(block, area);

            // Reserve last line for info bar.
            let media_height = inner.height.saturating_sub(1) as usize;
            let media_area = Rect::new(inner.x, inner.y, inner.width, media_height as u16);
            let info_area = Rect::new(inner.x, inner.y + media_height as u16, inner.width, 1);

            // Render the media lines.
            let visible_lines: Vec<Line> = lines.into_iter().take(media_height).collect();
            let paragraph = Paragraph::new(visible_lines);
            frame.render_widget(paragraph, media_area);

            // Info bar: dimensions + file size.
            let size_str = if file_size >= 1_048_576 {
                format!("{:.1} MB", file_size as f64 / 1_048_576.0)
            } else if file_size >= 1024 {
                format!("{:.1} KB", file_size as f64 / 1024.0)
            } else {
                format!("{file_size} B")
            };
            let info = format!(
                " {}x{} | {} ",
                dimensions.0, dimensions.1, size_str,
            );
            let info_widget = Paragraph::new(Span::styled(
                info,
                Style::default().fg(theme.muted),
            ));
            frame.render_widget(info_widget, info_area);
        }
        MediaContent::Error(msg) => {
            let error = Paragraph::new(msg)
                .style(Style::default().fg(Color::Red))
                .block(block);
            frame.render_widget(error, area);
        }
    }
}

/// Render a comment preview overlay at the bottom of the viewer area.
fn render_comment_preview(
    frame: &mut Frame,
    area: Rect,
    comments: &[ReviewComment],
    line: usize,
    has_selection: bool,
    reply_counts: &std::collections::HashMap<String, usize>,
    theme: &Theme,
) {
    let max_comments: usize = 3;
    let max_body_lines: usize = 3;
    let width = area.width.saturating_sub(2);

    // Pre-build all lines to know the exact height.
    let mut lines = Vec::new();

    // Header line.
    let count_label = if comments.len() == 1 {
        "1 comment".to_string()
    } else {
        format!("{} comments", comments.len())
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!(" L{line} "),
            Style::default().fg(theme.search_current_fg).bg(theme.search_match_bg),
        ),
        Span::styled(
            format!(" {count_label}"),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            "  Space: view thread",
            Style::default().fg(theme.muted),
        ),
    ]));

    // Comment bodies (multi-line preview).
    for comment in comments.iter().take(max_comments) {
        let kind_badge = crate::ui::review::kind_badge_span(comment.kind);
        let author_label = match comment.author {
            crate::review_store::Author::User => "you",
            crate::review_store::Author::Claude => "claude",
        };

        // Reply count badge.
        let reply_count = reply_counts.get(&comment.id).copied().unwrap_or(0);
        let reply_badge = if reply_count > 0 {
            Span::styled(
                format!(" [{reply_count} replies]"),
                Style::default().fg(theme.info),
            )
        } else {
            Span::raw("")
        };

        // First line: kind badge + author + reply count.
        lines.push(Line::from(vec![
            kind_badge,
            Span::styled(
                format!("{author_label}: "),
                Style::default().fg(theme.info),
            ),
            reply_badge,
        ]));

        // Body lines (up to max_body_lines).
        let body_lines: Vec<&str> = comment.body.split('\n').collect();
        let show_count = body_lines.len().min(max_body_lines);
        let truncated = body_lines.len() > max_body_lines;
        for body_line in body_lines.iter().take(show_count) {
            let max_chars = (width as usize).saturating_sub(4); // "  " indent
            let display: String = body_line.chars().take(max_chars).collect();
            lines.push(Line::from(Span::styled(
                format!("  {display}"),
                Style::default().fg(theme.fg),
            )));
        }
        if truncated {
            lines.push(Line::from(Span::styled(
                "  ...",
                Style::default().fg(theme.muted),
            )));
        }
    }

    if comments.len() > max_comments {
        lines.push(Line::from(Span::styled(
            format!("  +{} more", comments.len() - max_comments),
            Style::default().fg(theme.muted),
        )));
    }

    let height = lines.len() as u16;

    // Position above the selection hint overlay if a selection is active.
    let offset_for_selection = if has_selection { 1_u16 } else { 0 };
    let max_height = area.height.saturating_sub(2 + offset_for_selection);
    let clamped_height = height.min(max_height);
    let y = area
        .y
        .saturating_add(area.height)
        .saturating_sub(clamped_height + 1 + offset_for_selection);
    let preview_area = Rect::new(area.x + 1, y, width, clamped_height);

    frame.render_widget(ratatui::widgets::Clear, preview_area);

    let paragraph = Paragraph::new(lines).style(Style::default().bg(theme.comment_preview_bg));
    frame.render_widget(paragraph, preview_area);
}

/// Ensure the diff annotations cache in `ViewerState` is populated for the
/// currently viewed file. Only rebuilds if the file changed or the cache was
/// invalidated (e.g. after `load_diff()`).
fn ensure_diff_annotations_cached(app: &mut App) {
    use crate::diff_state::FileDiff;

    let current_file = app.viewer_state.content.current_file.clone();

    // Check if cache is still valid.
    if app.viewer_state.content.cached_diff_annotations.is_some()
        && app.viewer_state.content.cached_diff_annotations_file == current_file
    {
        return;
    }

    let mut annotations = std::collections::HashMap::new();

    if let Some(ref current) = current_file {
        let insert_annotations =
            |file_diff: &FileDiff,
             map: &mut std::collections::HashMap<usize, (DiffLineTag, Vec<InlineSegment>)>| {
                for hunk in &file_diff.hunks {
                    for line in &hunk.lines {
                        if line.tag == DiffLineTag::Insert {
                            if let Some(n) = line.new_line_no {
                                map.entry(n).or_insert_with(|| {
                                    (DiffLineTag::Insert, line.inline_segments.clone())
                                });
                            }
                        }
                    }
                }
            };

        // Uncommitted first (takes priority in the viewer).
        for file_diff in &app.diff_state.uncommitted_files {
            if file_diff.path == *current {
                insert_annotations(file_diff, &mut annotations);
                break;
            }
        }

        // Committed second (or_insert prevents overwriting uncommitted).
        for file_diff in &app.diff_state.committed_files {
            if file_diff.path == *current {
                insert_annotations(file_diff, &mut annotations);
                break;
            }
        }
    }

    app.viewer_state.content.cached_diff_annotations = Some(annotations);
    app.viewer_state.content.cached_diff_annotations_file = current_file;
}

/// Render intra-line diff segments with emphasis highlighting (plain white fg).
/// Used for Delete lines where syntax tokens are unavailable.
fn render_inline_diff_spans(
    segments: &[InlineSegment],
    diff_bg: Color,
    emphasis_bg: Color,
    tab_width: usize,
) -> Vec<Span<'static>> {
    segments
        .iter()
        .map(|seg| {
            let bg = if seg.emphasized { emphasis_bg } else { diff_bg };
            let text = expand_tabs(
                seg.text.trim_end_matches('\n').trim_end_matches('\r'),
                tab_width,
            );
            Span::styled(
                text,
                Style::default().fg(Color::White).bg(bg),
            )
        })
        .collect()
}

/// Merge syntax highlighting foreground colours with word-diff background
/// colours. Returns `None` if the expanded segment text does not match the
/// syntax token text (so the caller can fall back to plain rendering).
fn merge_syntax_with_inline(
    segments: &[InlineSegment],
    syntax_tokens: &[(Style, String)],
    diff_bg: Color,
    emphasis_bg: Color,
    tab_width: usize,
) -> Option<Vec<Span<'static>>> {
    // Build expanded text and per-byte emphasis flag from inline segments.
    let mut expanded_text = String::new();
    let mut byte_emphasis: Vec<bool> = Vec::new();

    for seg in segments {
        let trimmed = seg.text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs(trimmed, tab_width);
        byte_emphasis.resize(byte_emphasis.len() + expanded.len(), seg.emphasized);
        expanded_text.push_str(&expanded);
    }

    // Build per-byte fg style from syntax tokens.
    let mut syntax_text = String::new();
    let mut byte_fg: Vec<Style> = Vec::new();

    for (style, text) in syntax_tokens {
        byte_fg.resize(byte_fg.len() + text.len(), *style);
        syntax_text.push_str(text);
    }

    // The texts must match after tab expansion; bail out otherwise.
    if expanded_text != syntax_text {
        return None;
    }

    let len = expanded_text.len();
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut i = 0;

    while i < len {
        let start = i;
        let emph = byte_emphasis[i];
        let fg = byte_fg[i];
        let bg = if emph { emphasis_bg } else { diff_bg };

        i += 1;
        while i < len {
            let next_emph = byte_emphasis[i];
            let next_fg_color = byte_fg[i].fg;
            if next_emph != emph || next_fg_color != fg.fg {
                break;
            }
            i += 1;
        }

        // Ensure we land on a UTF-8 char boundary.
        while i < len && !expanded_text.is_char_boundary(i) {
            i += 1;
        }

        result.push(Span::styled(
            expanded_text[start..i].to_string(),
            fg.bg(bg),
        ));
    }

    Some(result)
}

/// Expand tab characters to spaces, matching the viewer's tab expansion.
fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut result = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = tab_width - (col % tab_width);
            for _ in 0..spaces {
                result.push(' ');
            }
            col += spaces;
        } else {
            result.push(ch);
            col += 1;
        }
    }
    result
}

fn render_search_box(frame: &mut Frame, area: Rect, query: &crate::text_input::TextInput, theme: &Theme, suppress_cursor: bool) {
    let height = 1_u16;
    let y = area.y + area.height.saturating_sub(height + 1);
    let search_area = Rect::new(area.x + 1, y, area.width.saturating_sub(2), height);

    frame.render_widget(ratatui::widgets::Clear, search_area);

    let text = format!("/{}\u{2588}{}", query.text_before_cursor(), query.text_after_cursor());
    let paragraph = Paragraph::new(Span::styled(
        text,
        Style::default().fg(theme.search_match_fg),
    ));
    frame.render_widget(paragraph, search_area);
    if !suppress_cursor {
        // +1 for the leading '/' character
        let cursor_x = search_area.x + 1 + query.display_width_before_cursor() as u16;
        if cursor_x < search_area.x + search_area.width {
            frame.set_cursor_position(Position::new(cursor_x, search_area.y));
        }
    }
}

// ── Syntax highlighting via cached syntect data ─────────────────────────

/// Return ratatui `Span`s for a single line from the syntect highlight cache.
///
/// If a `diff_bg` is provided, the token foreground colours are preserved but
/// the background is overridden with the diff colour.  When no cache entry
/// exists for the line, a plain white fallback is returned.
fn syntax_spans_for_line(
    vs: &crate::viewer::ViewerState,
    line_no: usize,
    diff_bg: Option<Color>,
) -> Vec<Span<'static>> {
    if let Some(tokens) = vs.content.highlighted_lines.get(line_no) {
        tokens
            .iter()
            .map(|(style, text)| {
                let s = if let Some(bg) = diff_bg {
                    // Keep token fg, override bg with diff colour.
                    style.bg(bg)
                } else {
                    *style
                };
                Span::styled(text.clone(), s)
            })
            .collect()
    } else {
        // Fallback: plain white text.
        let text = vs
            .content.file_content
            .get(line_no)
            .cloned()
            .unwrap_or_default();
        vec![Span::styled(text, Style::default().fg(Color::White))]
    }
}

/// Skip `offset` characters from the beginning of a sequence of `Span`s and
/// truncate to at most `max_width` characters, preserving per-span styling.
fn h_scroll_spans(spans: Vec<Span<'static>>, offset: usize, max_width: usize) -> Vec<Span<'static>> {
    let mut remaining_skip = offset;
    let mut remaining_width = max_width;
    let mut result: Vec<Span<'static>> = Vec::new();
    for span in spans {
        if remaining_width == 0 {
            break;
        }
        let char_count = span.content.chars().count();
        // Left clipping: skip characters for horizontal scroll offset.
        if remaining_skip > 0 {
            if remaining_skip >= char_count {
                remaining_skip -= char_count;
                continue;
            }
            let s: String = span.content.chars().skip(remaining_skip).collect();
            let len = s.chars().count();
            if len <= remaining_width {
                remaining_width -= len;
                result.push(Span::styled(s, span.style));
            } else {
                let truncated: String = s.chars().take(remaining_width).collect();
                remaining_width = 0;
                result.push(Span::styled(truncated, span.style));
            }
            remaining_skip = 0;
        } else {
            // Right clipping: truncate to remaining panel width.
            if char_count <= remaining_width {
                remaining_width -= char_count;
                result.push(span);
            } else {
                let truncated: String = span.content.chars().take(remaining_width).collect();
                remaining_width = 0;
                result.push(Span::styled(truncated, span.style));
            }
        }
    }
    result
}

fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

/// Apply underline + accent fg to spans within `[start_col..end_col)` of the original content.
/// `h_scroll` is the horizontal scroll offset already applied to the spans.
fn apply_underline_range(
    spans: Vec<Span<'static>>,
    start_col: usize,
    end_col: usize,
    h_scroll: usize,
    accent: Color,
) -> Vec<Span<'static>> {
    // Convert original content cols to visible cols (after h_scroll).
    let vis_start = start_col.saturating_sub(h_scroll);
    let vis_end = end_col.saturating_sub(h_scroll);
    if vis_start >= vis_end {
        return spans;
    }

    let mut result = Vec::with_capacity(spans.len() + 4);
    let mut pos: usize = 0;
    for span in spans {
        let span_len = span.content.chars().count();
        let span_end = pos + span_len;

        if span_end <= vis_start || pos >= vis_end {
            // Entirely outside the underline range.
            result.push(span);
        } else {
            // This span overlaps the underline range.
            let rel_start = vis_start.saturating_sub(pos);
            let rel_end = vis_end.saturating_sub(pos).min(span_len);

            let chars: Vec<char> = span.content.chars().collect();

            // Before underline.
            if rel_start > 0 {
                let before: String = chars[..rel_start].iter().collect();
                result.push(Span::styled(before, span.style));
            }
            // Underline portion.
            let underlined: String = chars[rel_start..rel_end].iter().collect();
            result.push(Span::styled(
                underlined,
                span.style.fg(accent).add_modifier(Modifier::UNDERLINED),
            ));
            // After underline.
            if rel_end < span_len {
                let after: String = chars[rel_end..].iter().collect();
                result.push(Span::styled(after, span.style));
            }
        }
        pos = span_end;
    }
    result
}

/// Apply Vimium-style hint labels to spans, replacing the first 2 characters of each
/// hinted symbol with the label text in accent color + bold.
fn apply_hint_labels(
    spans: Vec<Span<'static>>,
    hints: &[&crate::overlay::SymbolHint],
    input: &str,
    h_scroll: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut result = spans;
    // Process hints in reverse order so earlier replacements don't shift positions of later ones.
    let mut sorted: Vec<&&crate::overlay::SymbolHint> = hints.iter().collect();
    sorted.sort_by(|a, b| b.start_col.cmp(&a.start_col));

    for hint in sorted {
        let vis_start = hint.start_col.saturating_sub(h_scroll);
        let label_len = hint.label.chars().count();
        let vis_end = vis_start + label_len;

        // Determine if this hint matches the current input.
        let is_matching = input.is_empty() || hint.label.starts_with(input);
        let label_style = if is_matching {
            Style::default()
                .fg(Color::Black)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(Color::Reset)
        };

        // Replace characters at vis_start..vis_end with the label.
        result = replace_span_range(result, vis_start, vis_end, &hint.label, label_style);
    }
    result
}

/// Replace characters in the range `[start..end)` of the span list with `replacement` text
/// in the given style.
/// Build the breadcrumb `Line` from jump history + current position.
/// Returns `None` when there are fewer than 2 entries (no navigation happened).
fn build_breadcrumb_line(app: &App) -> Option<Line<'static>> {
    let current_file = app.viewer_state.content.current_file.as_ref()?;
    let current = crate::jump_history::Location {
        file_path: current_file.clone(),
        line: app.viewer_state.content.file_scroll,
        h_scroll: app.viewer_state.content.h_scroll,
    };

    let (entries, cur_idx) = app.jump_history.breadcrumb_trail(&current, 7);

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
    let filename = loc
        .file_path
        .rsplit('/')
        .next()
        .unwrap_or(&loc.file_path);
    format!("{}:{}", filename, loc.line + 1)
}

fn replace_span_range(
    spans: Vec<Span<'static>>,
    start: usize,
    end: usize,
    replacement: &str,
    style: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::with_capacity(spans.len() + 4);
    let mut pos: usize = 0;

    for span in spans {
        let span_len = span.content.chars().count();
        let span_end = pos + span_len;

        if span_end <= start || pos >= end {
            // Entirely outside the replacement range.
            result.push(span);
        } else {
            let chars: Vec<char> = span.content.chars().collect();
            let rel_start = start.saturating_sub(pos);
            let rel_end = end.saturating_sub(pos).min(span_len);

            // Before replacement.
            if rel_start > 0 {
                let before: String = chars[..rel_start].iter().collect();
                result.push(Span::styled(before, span.style));
            }
            // Replacement portion (only emit once, from the first overlapping span).
            if pos <= start {
                result.push(Span::styled(replacement.to_string(), style));
            }
            // After replacement.
            if rel_end < span_len {
                let after: String = chars[rel_end..].iter().collect();
                result.push(Span::styled(after, span.style));
            }
        }
        pos = span_end;
    }
    result
}
