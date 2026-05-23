//! Viewer panel — file content display with diff highlights and review comments.
//!
//! Shows the content of the selected file in the middle column. Lines that
//! have been modified (according to diff_state) are highlighted inline.
//! Review comments are shown as inline badges.

use crate::app::{App, Focus};
use crate::diff_state::{DiffLineTag, InlineSegment};
use crate::media_state::MediaContent;
use crate::theme::Theme;
use crate::viewer::UnifiedDiffEntry;
use ratatui::Frame;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

/// Render the viewer (file content) panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Clear screen-row map so stale data isn't used in diff/media modes.
    app.viewer_state.content.screen_row_map.clear();

    // Populate diff annotations cache before taking any shared borrows.
    ensure_diff_annotations_cached(app);

    let theme = &app.theme;
    let vs = &app.viewer_state;
    let tab_width = app.config.viewer.tab_width;
    let focused = app.focus == Focus::Viewer;
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border_unfocused
    };

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
                let tail: String = raw
                    .trim()
                    .chars()
                    .rev()
                    .take(inner_max.saturating_sub(1))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                format!(" \u{2026}{tail} ")
            } else {
                raw
            }
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
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(
            Line::from(Span::styled(
                expand_label,
                Style::default().fg(expand_color),
            ))
            .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
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
    let mut lines: Vec<Line> = Vec::with_capacity(inner_height);
    let mut screen_row_map: Vec<crate::viewer::ScreenRow> = Vec::with_capacity(inner_height);
    let mut remaining = inner_height;

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

        let line_1 = line_no + 1;
        let is_selected = vs.is_line_selected(line_1);
        let is_hovered = vs.click.hover_line == Some(line_1);
        let is_gutter_hovered = vs.click.hover_gutter_line == Some(line_1);
        let is_in_pending_range =
            !is_selected && vs.is_selection_pending() && vs.click.hover_line.is_some() && {
                let start = match vs.selection {
                    crate::viewer::LineSelection::Pending { start } => start,
                    _ => 0,
                };
                let hover = vs.click.hover_line.unwrap();
                let (lo, hi) = if start <= hover {
                    (start, hover)
                } else {
                    (hover, start)
                };
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
            Style::default()
                .fg(theme.gutter_hover_fg)
                .bg(theme.gutter_hover_bg)
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

        // Comment badge: 💬 on the last line of a comment range,
        // │ on earlier lines in the range, 💬 (muted) on gutter hover.
        let badge = if comment_end_lines.contains(&line_1) {
            Span::styled("💬", Style::default().fg(theme.accent))
        } else if comment_lines.contains(&line_1) {
            Span::styled("│ ", Style::default().fg(theme.accent))
        } else if is_gutter_hovered {
            Span::styled("💬", Style::default().fg(theme.muted))
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
                Style::default()
                    .fg(theme.search_current_fg)
                    .bg(theme.search_match_bg),
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
                Style::default()
                    .bg(theme.line_selected_bg)
                    .fg(theme.line_selected_fg),
            )]
        } else if is_in_pending_range {
            vec![Span::styled(
                content.to_string(),
                Style::default()
                    .bg(theme.line_pending_bg)
                    .fg(theme.line_selected_fg),
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
                    vs.content
                        .highlighted_lines
                        .get(line_no)
                        .filter(|t| !t.is_empty())
                        .and_then(|tokens| {
                            merge_syntax_with_inline(
                                ann_segments,
                                tokens,
                                diff_bg,
                                emphasis_bg,
                                tab_width,
                            )
                        })
                        .unwrap_or_else(|| {
                            syntax_spans_for_line(vs, line_no, Some(diff_bg), theme.fg)
                        })
                } else {
                    render_inline_diff_spans(
                        ann_segments,
                        diff_bg,
                        emphasis_bg,
                        theme.fg,
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
                syntax_spans_for_line(vs, line_no, diff_bg, theme.fg)
            }
        } else {
            syntax_spans_for_line(vs, line_no, gutter_bg, theme.fg)
        };

        // Apply horizontal scroll to content spans, clipping to panel width.
        let content_max_w = (area.width as usize).saturating_sub(gutter_width + 8);
        let content_spans = h_scroll_spans(content_spans, vs.content.h_scroll, content_max_w);

        // Apply underline to hover symbol (Cmd+hover for jump targets).
        let content_spans = if let Some(ref hs) = vs.click.hover_symbol {
            if hs.line == line_1 {
                apply_underline_range(
                    content_spans,
                    hs.start_col,
                    hs.end_col,
                    vs.content.h_scroll,
                    theme.accent,
                )
            } else {
                content_spans
            }
        } else {
            content_spans
        };

        // Apply symbol hint labels (Vimium-style).
        let content_spans = if app.symbol_hint_overlay.active {
            let hints_on_line: Vec<_> = app
                .symbol_hint_overlay
                .hints
                .iter()
                .filter(|h| h.line == line_1)
                .collect();
            if hints_on_line.is_empty() {
                content_spans
            } else {
                apply_hint_labels(
                    content_spans,
                    &hints_on_line,
                    &app.symbol_hint_overlay.input,
                    vs.content.h_scroll,
                    theme,
                )
            }
        } else {
            content_spans
        };

        let mut spans = vec![gutter_span, badge];
        spans.extend(content_spans);
        lines.push(Line::from(spans));
        screen_row_map.push(crate::viewer::ScreenRow::Code(line_1));
        remaining -= 1;

        // Inject inline thread rows below the LAST line of a comment range.
        if remaining > 0 && expanded_threads.contains(&line_1) {
            let reply_cid = if inline_reply_line == Some(line_1) {
                app.viewer_state.explorer.inline_reply_comment_id.as_deref()
            } else {
                None
            };
            let thread_lines = build_inline_thread_lines(
                line_1,
                gutter_width,
                area.width as usize,
                &app.review_state,
                reply_cid,
                &app.viewer_state.explorer.inline_reply_buffer,
                theme,
            );
            for (line, row_type) in thread_lines {
                if remaining == 0 {
                    break;
                }
                lines.push(line);
                screen_row_map.push(row_type);
                remaining -= 1;
            }
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
    app.viewer_state.content.screen_row_map = screen_row_map;
}

/// Build the display line for a hunk separator (a collapsed gap between hunks),
/// optionally annotated with the enclosing function header.
fn render_hunk_separator(
    func_header: &Option<String>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
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

/// Build the display line for an expandable context block, showing how many
/// lines are hidden and an optional function header.
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

/// Render the unified diff view (GitHub-style).
/// Shared per-frame context for rendering a single unified-diff content line.
struct DiffLineRenderCtx<'a> {
    vs: &'a crate::viewer::ViewerState,
    theme: &'a Theme,
    gutter_width: usize,
    tab_width: usize,
    area_width: u16,
    comment_lines: &'a std::collections::HashSet<usize>,
    comment_end_lines: &'a std::collections::HashSet<usize>,
}

/// Build the display line for a single diff content line (context / addition /
/// deletion), including the gutter, comment badge, syntax/word-diff styled
/// content, and GitHub-style full-width background fill.
fn render_diff_content_line(
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

    let is_selected = new_line_no.map(|n| vs.is_line_selected(n)).unwrap_or(false);
    let is_hovered = new_line_no
        .map(|n| vs.click.hover_line == Some(n))
        .unwrap_or(false);
    let is_gutter_hovered = new_line_no
        .map(|n| vs.click.hover_gutter_line == Some(n))
        .unwrap_or(false);
    let is_in_pending_range = !is_selected
        && new_line_no.is_some()
        && vs.is_selection_pending()
        && vs.click.hover_line.is_some()
        && {
            let n = new_line_no.unwrap();
            let start = match vs.selection {
                crate::viewer::LineSelection::Pending { start } => start,
                _ => 0,
            };
            let hover = vs.click.hover_line.unwrap();
            let (lo, hi) = if start <= hover {
                (start, hover)
            } else {
                (hover, start)
            };
            n >= lo && n <= hi
        };

    // Gutter marker.
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

    // Comment badge: 💬 on end lines, │ on earlier range lines,
    // 💬 (muted) on hovered gutter.
    let badge = if new_line_no.is_some_and(|n| ctx.comment_end_lines.contains(&n)) {
        Span::styled("💬", Style::default().fg(theme.accent))
    } else if new_line_no.is_some_and(|n| ctx.comment_lines.contains(&n)) {
        Span::styled("│ ", Style::default().fg(theme.accent))
    } else if is_gutter_hovered {
        Span::styled("💬", Style::default().fg(theme.muted))
    } else {
        Span::raw("  ")
    };

    // Content styling.
    let content_spans: Vec<Span> = if is_selected {
        vec![Span::styled(
            content.to_string(),
            Style::default()
                .bg(theme.line_selected_bg)
                .fg(theme.line_selected_fg),
        )]
    } else if is_in_pending_range {
        vec![Span::styled(
            content.to_string(),
            Style::default()
                .bg(theme.line_pending_bg)
                .fg(theme.line_selected_fg),
        )]
    } else if !inline_segments.is_empty() {
        match tag {
            DiffLineTag::Insert => {
                // Try syntax highlighting + word-diff merge.
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
        // No inline segments — use syntax highlighting or plain.
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

    // Apply horizontal scroll, clipping to panel width.
    let content_max_w = (ctx.area_width as usize).saturating_sub(gutter_width + 8);
    let content_spans = h_scroll_spans(content_spans, vs.content.h_scroll, content_max_w);

    let mut spans = vec![gutter_span, badge];
    spans.extend(content_spans);

    // Extend background color to the end of the line for
    // Insert/Delete rows (GitHub-style block coloring).
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

    // Collect the *end* lines of comments (last line of each range).
    let comment_end_lines: std::collections::HashSet<usize> = app
        .review_state
        .comments
        .iter()
        .filter(|c| app.viewer_state.content.current_file.as_deref() == Some(&*c.file_path))
        .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
        .collect();

    let line_ctx = DiffLineRenderCtx {
        vs,
        theme,
        gutter_width,
        tab_width,
        area_width: area.width,
        comment_lines: &comment_lines,
        comment_end_lines: &comment_end_lines,
    };

    let lines: Vec<Line> = vs
        .diff_view
        .diff_view_lines
        .iter()
        .skip(vs.diff_view.diff_view_scroll)
        .take(inner_height)
        .map(|entry| match entry {
            UnifiedDiffEntry::HunkSeparator { func_header } => {
                let width = area.width.saturating_sub(2) as usize;
                render_hunk_separator(func_header, width, theme)
            }
            UnifiedDiffEntry::ExpandableContext {
                hidden_count,
                func_header,
                ..
            } => {
                let width = area.width.saturating_sub(2) as usize;
                render_expandable_context(*hidden_count, func_header, width, theme)
            }
            UnifiedDiffEntry::Line {
                tag,
                new_line_no,
                content,
                inline_segments,
            } => render_diff_content_line(tag, new_line_no, content, inline_segments, &line_ctx),
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
            Style::default()
                .fg(theme.gutter_selected_fg)
                .bg(theme.gutter_selected_bg),
        ));
        frame.render_widget(hint_widget, hint_area);
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
        MediaContent::Rendered {
            lines,
            dimensions,
            file_size,
        } => {
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
            let info = format!(" {}x{} | {} ", dimensions.0, dimensions.1, size_str,);
            let info_widget = Paragraph::new(Span::styled(info, Style::default().fg(theme.muted)));
            frame.render_widget(info_widget, info_area);
        }
        MediaContent::Error(msg) => {
            let error = Paragraph::new(msg)
                .style(Style::default().fg(theme.error))
                .block(block);
            frame.render_widget(error, area);
        }
    }
}

/// Soft-wrap a string at word boundaries to fit within `max_width` columns.
fn soft_wrap(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut col = 0;
    for word in text.split_inclusive(|c: char| c.is_whitespace()) {
        let wlen = unicode_width::UnicodeWidthStr::width(word);
        if col + wlen > max_width && col > 0 {
            lines.push(current.trim_end().to_string());
            current = String::new();
            col = 0;
        }
        current.push_str(word);
        col += wlen;
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current.trim_end().to_string());
    }
    lines
}

/// Build inline thread rows for a comment at the given line.
///
/// Returns a vec of `Line`s representing the thread box:
/// top border, each comment + replies + action icons, bottom border.
fn build_inline_thread_lines<'a>(
    line_1: usize,
    gutter_width: usize,
    panel_width: usize,
    review_state: &crate::review_state::ReviewState,
    reply_comment_id: Option<&str>,
    reply_buffer: &crate::text_input::TextInput,
    theme: &Theme,
) -> Vec<(Line<'a>, crate::viewer::ScreenRow)> {
    let comments = match review_state.file_comments.get(&line_1) {
        Some(c) if !c.is_empty() => c,
        _ => return Vec::new(),
    };

    use crate::viewer::ScreenRow;
    let mut out: Vec<(Line, ScreenRow)> = Vec::new();
    let left_pad = gutter_width + 4 + 2; // gutter + badge
    let gutter_pad: String = " ".repeat(left_pad);
    let border_style = Style::default().fg(theme.accent);
    let thread_bg = Style::default().bg(theme.comment_preview_bg);
    let content_style = Style::default().fg(theme.fg).bg(theme.comment_preview_bg);
    let muted_style = Style::default()
        .fg(theme.muted)
        .bg(theme.comment_preview_bg);
    let info_style = Style::default().fg(theme.info).bg(theme.comment_preview_bg);
    // Box inner width (between │ and │).
    let box_inner = panel_width.saturating_sub(left_pad + 4 + 2); // "  │ " left + " │" right
    let wrap_width = box_inner.saturating_sub(6).max(20); // indent inside box

    // Helper: make a bordered content line with left │, padded to full width.
    let make_line = |spans: Vec<Span<'a>>| -> (Line<'a>, ScreenRow) {
        let mut all = vec![
            Span::styled(gutter_pad.clone(), thread_bg),
            Span::styled("  │ ", border_style),
        ];
        all.extend(spans);
        // Pad the line to panel_width so the background color fills the entire row.
        let used: usize = all
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let remaining = panel_width.saturating_sub(used + 2); // +2 for block borders
        if remaining > 0 {
            all.push(Span::styled(" ".repeat(remaining), thread_bg));
        }
        (Line::from(all).style(thread_bg), ScreenRow::ThreadContent)
    };

    // Helper: make a full-width border line with bg fill.
    let make_border = |content: String| -> (Line<'a>, ScreenRow) {
        let text = format!("{gutter_pad}{content}");
        let used = unicode_width::UnicodeWidthStr::width(text.as_str());
        let pad = panel_width.saturating_sub(used + 2);
        let mut spans = vec![
            Span::styled(gutter_pad.clone(), thread_bg),
            Span::styled(content, border_style),
        ];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), thread_bg));
        }
        (Line::from(spans).style(thread_bg), ScreenRow::ThreadContent)
    };

    // Top border.
    let top_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  ┌{top_fill}┐")));

    for (ci, comment) in comments.iter().enumerate() {
        // Blank spacer line between comments in the same thread.
        if ci > 0 {
            out.push(make_line(vec![Span::styled("", content_style)]));
        }

        let author_label = match comment.author {
            crate::review_store::Author::User => "you",
            crate::review_store::Author::Claude => "claude",
        };

        // Comment body lines (with soft wrap). No kind badge / author icon.
        let header_prefix_len = author_label.len() + 2; // "author: "
        let first_line_wrap = wrap_width.saturating_sub(header_prefix_len).max(20);
        let mut is_first = true;
        for body_line in comment.body.split('\n') {
            if is_first {
                is_first = false;
                let wrapped = soft_wrap(body_line, first_line_wrap);
                for (wi, wline) in wrapped.iter().enumerate() {
                    if wi == 0 {
                        out.push(make_line(vec![
                            Span::styled(format!("{author_label}: "), info_style),
                            Span::styled(wline.clone(), content_style),
                        ]));
                    } else {
                        out.push(make_line(vec![Span::styled(
                            format!("  {wline}"),
                            content_style,
                        )]));
                    }
                }
            } else {
                let wrapped = soft_wrap(body_line, wrap_width);
                for wline in &wrapped {
                    out.push(make_line(vec![Span::styled(
                        format!("  {wline}"),
                        content_style,
                    )]));
                }
            }
        }

        // Show replies if cached.
        if let Some(replies) = review_state.cached_replies.get(&comment.id) {
            for reply in replies {
                let reply_author = match reply.author {
                    crate::review_store::Author::User => "you",
                    crate::review_store::Author::Claude => "claude",
                };
                let reply_header_len = reply_author.len() + 4; // "  author: "
                let reply_first_wrap = wrap_width.saturating_sub(reply_header_len).max(20);
                let mut is_first_reply_line = true;
                for reply_line in reply.body.split('\n') {
                    let w = if is_first_reply_line {
                        reply_first_wrap
                    } else {
                        wrap_width
                    };
                    let wrapped = soft_wrap(reply_line, w);
                    for (wi, wline) in wrapped.iter().enumerate() {
                        if is_first_reply_line && wi == 0 {
                            is_first_reply_line = false;
                            out.push(make_line(vec![
                                Span::styled(format!("  {reply_author}: "), info_style),
                                Span::styled(wline.clone(), content_style),
                            ]));
                        } else {
                            out.push(make_line(vec![Span::styled(
                                format!("    {wline}"),
                                content_style,
                            )]));
                        }
                    }
                }
            }
        }

        // Per-comment action icons row or active reply input.
        let is_replying_to_this = reply_comment_id == Some(comment.id.as_str());
        let action_row_type = ScreenRow::ThreadActions {
            comment_id: comment.id.clone(),
        };
        if is_replying_to_this {
            let buf_text = reply_buffer.text().to_string();
            let (line, _) = make_line(vec![
                Span::styled(
                    "> ",
                    Style::default()
                        .fg(theme.accent)
                        .bg(theme.comment_preview_bg),
                ),
                Span::styled(
                    if buf_text.is_empty() {
                        "Type reply...".to_string()
                    } else {
                        buf_text
                    },
                    content_style,
                ),
            ]);
            out.push((line, action_row_type));
        } else {
            // Clickable action icons: ↩ reply  ✔ resolve  x delete  ...  ✨ ask claude
            let reply_style = Style::default().fg(theme.info).bg(theme.comment_preview_bg);
            let resolve_style = Style::default()
                .fg(theme.success)
                .bg(theme.comment_preview_bg);
            let delete_style = Style::default()
                .fg(theme.error)
                .bg(theme.comment_preview_bg);
            let claude_style = Style::default()
                .fg(Color::Rgb(180, 140, 255))
                .bg(theme.comment_preview_bg);
            let status_label = match comment.status {
                crate::review_store::CommentStatus::Pending => "✔ resolve",
                crate::review_store::CommentStatus::Resolved => "↺ unresolve",
            };

            // Build the left-side actions and right-side "✨ ask claude".
            let left_actions = vec![
                Span::styled("↩ reply", reply_style),
                Span::styled("  ", muted_style),
                Span::styled(status_label.to_string(), resolve_style),
                Span::styled("  ", muted_style),
                Span::styled("x delete", delete_style),
            ];
            let right_label = "✨ ask claude";
            let right_label_w = unicode_width::UnicodeWidthStr::width(right_label);

            let left_w: usize = left_actions
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let prefix_w = left_pad + 4; // gutter_pad + "  │ "
            let gap = panel_width.saturating_sub(prefix_w + left_w + right_label_w + 2 + 1);

            let mut spans = vec![
                Span::styled(gutter_pad.clone(), thread_bg),
                Span::styled("  │ ", border_style),
            ];
            spans.extend(left_actions);
            if gap > 0 {
                spans.push(Span::styled(" ".repeat(gap), thread_bg));
            }
            spans.push(Span::styled(right_label.to_string(), claude_style));
            spans.push(Span::styled(" ", thread_bg)); // trailing pad

            let line = Line::from(spans).style(thread_bg);
            out.push((line, action_row_type));
        }
    }

    // Bottom border.
    let bot_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  └{bot_fill}┘")));

    out
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
        let insert_annotations = |file_diff: &FileDiff,
                                  map: &mut std::collections::HashMap<
            usize,
            (DiffLineTag, Vec<InlineSegment>),
        >| {
            for hunk in &file_diff.hunks {
                for line in &hunk.lines {
                    if line.tag == DiffLineTag::Insert
                        && let Some(n) = line.new_line_no
                    {
                        map.entry(n)
                            .or_insert_with(|| (DiffLineTag::Insert, line.inline_segments.clone()));
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

/// Render intra-line diff segments with emphasis highlighting.
/// Used for Delete lines where syntax tokens are unavailable; `fg` is the
/// plain text color (the active theme's foreground).
fn render_inline_diff_spans(
    segments: &[InlineSegment],
    diff_bg: Color,
    emphasis_bg: Color,
    fg: Color,
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
            Span::styled(text, Style::default().fg(fg).bg(bg))
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
    // Tabs are expanded with a *shared* column counter across segments so the
    // result matches the column-correct expansion of the syntax tokens below.
    let mut expanded_text = String::new();
    let mut byte_emphasis: Vec<bool> = Vec::new();

    let mut col = 0;
    for seg in segments {
        let trimmed = seg.text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs_at(trimmed, tab_width, &mut col);
        byte_emphasis.resize(byte_emphasis.len() + expanded.len(), seg.emphasized);
        expanded_text.push_str(&expanded);
    }

    // Build per-byte fg style from syntax tokens. The syntax cache stores raw
    // (un-expanded) tabs, so expand them here too — using the same shared
    // column counter — otherwise any line containing a tab would fail the
    // equality check below and silently lose its syntax + emphasis colouring.
    let mut syntax_text = String::new();
    let mut byte_fg: Vec<Style> = Vec::new();

    let mut col = 0;
    for (style, text) in syntax_tokens {
        let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs_at(trimmed, tab_width, &mut col);
        byte_fg.resize(byte_fg.len() + expanded.len(), *style);
        syntax_text.push_str(&expanded);
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

        result.push(Span::styled(expanded_text[start..i].to_string(), fg.bg(bg)));
    }

    Some(result)
}

/// Expand tab characters to spaces, matching the viewer's tab expansion.
fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut col = 0;
    expand_tabs_at(line, tab_width, &mut col)
}

/// Expand tabs starting from column `col`, advancing `col` past the piece.
///
/// Threading a shared `col` across consecutive pieces of one line keeps tab
/// stops column-correct, so two different tokenisations of the same line
/// (word-diff segments vs. syntax tokens) expand to identical text.
fn expand_tabs_at(piece: &str, tab_width: usize, col: &mut usize) -> String {
    let mut result = String::with_capacity(piece.len());
    for ch in piece.chars() {
        if ch == '\t' {
            let spaces = tab_width - (*col % tab_width);
            for _ in 0..spaces {
                result.push(' ');
            }
            *col += spaces;
        } else {
            result.push(ch);
            *col += 1;
        }
    }
    result
}

fn render_search_box(
    frame: &mut Frame,
    area: Rect,
    query: &crate::text_input::TextInput,
    theme: &Theme,
    suppress_cursor: bool,
) {
    let height = 1_u16;
    let y = area.y + area.height.saturating_sub(height + 1);
    let search_area = Rect::new(area.x + 1, y, area.width.saturating_sub(2), height);

    frame.render_widget(ratatui::widgets::Clear, search_area);

    let text = format!(
        "/{}\u{2588}{}",
        query.text_before_cursor(),
        query.text_after_cursor()
    );
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
    fg: Color,
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
        // Fallback: plain text in the theme foreground.
        let text = vs
            .content
            .file_content
            .get(line_no)
            .cloned()
            .unwrap_or_default();
        vec![Span::styled(text, Style::default().fg(fg))]
    }
}

/// Skip `offset` characters from the beginning of a sequence of `Span`s and
/// truncate to at most `max_width` characters, preserving per-span styling.
fn h_scroll_spans(
    spans: Vec<Span<'static>>,
    offset: usize,
    max_width: usize,
) -> Vec<Span<'static>> {
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
    sorted.sort_by_key(|h| std::cmp::Reverse(h.start_col));

    for hint in sorted {
        let vis_start = hint.start_col.saturating_sub(h_scroll);
        let label_len = hint.label.chars().count();
        let vis_end = vis_start + label_len;

        // Determine if this hint matches the current input.
        let is_matching = input.is_empty() || hint.label.starts_with(input);
        let label_style = if is_matching {
            Style::default()
                .fg(theme.selected_fg)
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
    let filename = loc.file_path.rsplit('/').next().unwrap_or(&loc.file_path);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, emphasized: bool) -> InlineSegment {
        InlineSegment {
            text: text.to_string(),
            emphasized,
        }
    }

    /// Concatenate all span contents of a line into a single string.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn merge_handles_tabbed_lines() {
        // A line "\tlet x" highlighted as two syntax tokens carrying a raw tab.
        // The word-diff segments expand the tab; the syntax tokens must be
        // expanded the same way or the merge silently drops to plain rendering.
        let segments = vec![seg("\tlet ", false), seg("x", true)];
        let syntax_tokens = vec![
            (Style::default().fg(Color::Red), "\t".to_string()),
            (Style::default().fg(Color::Blue), "let x".to_string()),
        ];
        let merged = merge_syntax_with_inline(
            &segments,
            &syntax_tokens,
            Color::Rgb(0, 40, 0),
            Color::Rgb(0, 80, 0),
            4,
        );
        // Before the tab fix this returned None (texts mismatched on the tab).
        let spans = merged.expect("tabbed line should merge, not fall back to plain");
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "    let x"); // tab expanded to 4 spaces at column 0
    }

    #[test]
    fn merge_bails_on_text_mismatch() {
        // Genuinely different text (not just tabs) must still bail out so the
        // caller can fall back to plain rendering.
        let segments = vec![seg("foo", false)];
        let syntax_tokens = vec![(Style::default(), "bar".to_string())];
        let merged = merge_syntax_with_inline(
            &segments,
            &syntax_tokens,
            Color::Rgb(0, 40, 0),
            Color::Rgb(0, 80, 0),
            4,
        );
        assert!(merged.is_none());
    }

    #[test]
    fn hunk_separator_with_header_includes_header() {
        let theme = Theme::default();
        let line = render_hunk_separator(&Some("fn foo()".to_string()), 40, &theme);
        let text = line_text(&line);
        assert!(text.starts_with(" ··· "));
        assert!(text.contains("fn foo()"));
        // 3 spans: prefix, header, suffix+fill.
        assert_eq!(line.spans.len(), 3);
    }

    #[test]
    fn hunk_separator_without_header_is_single_fill() {
        let theme = Theme::default();
        let line = render_hunk_separator(&None, 20, &theme);
        let text = line_text(&line);
        assert!(text.starts_with(" ··· "));
        // Padded with the fill character up to the requested width.
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
