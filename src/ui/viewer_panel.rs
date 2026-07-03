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

/// Shared definition of the inline-thread action row.
///
/// The renderer ([`build_inline_thread_lines`]) and the mouse hit-testing in
/// `event/mouse.rs` must agree on where each action sits; both derive their
/// layout from these constants so a label change cannot silently break
/// click targets.
pub(crate) mod thread_actions {
    pub const REPLY: &str = "\u{21a9} reply"; // ↩ reply
    pub const RESOLVE: &str = "\u{2713} resolve"; // ✓ resolve
    pub const UNRESOLVE: &str = "\u{21ba} unresolve"; // ↺ unresolve
    pub const DELETE: &str = "\u{2717} delete"; // ✗ delete
    pub const ASK_CLAUDE: &str = "\u{2728} ask claude"; // ✨ ask claude
    /// Columns of spacing between actions.
    pub const GAP: usize = 2;

    fn w(s: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(s)
    }

    /// Width the status (resolve/unresolve) slot is padded to, so the delete
    /// action starts at a stable column regardless of the current status.
    pub fn status_slot_width() -> usize {
        w(RESOLVE).max(w(UNRESOLVE))
    }

    /// Clicks left of this column (relative to the action-row content start)
    /// hit "reply".
    pub fn reply_end() -> usize {
        w(REPLY) + GAP
    }

    /// Clicks in `reply_end()..resolve_end()` hit "resolve"/"unresolve";
    /// clicks at or beyond it hit "delete" (or "ask claude" on the far right).
    pub fn resolve_end() -> usize {
        reply_end() + status_slot_width() + GAP
    }

    /// Display width of the right-aligned "ask claude" button, for hit-testing
    /// against the panel's right edge.
    pub fn ask_claude_width() -> usize {
        w(ASK_CLAUDE)
    }
}

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
    let compose_anchor_end = new_comment_anchor_end(app);
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
        // │ on earlier lines in the range, and a GitHub-style "+" button on
        // gutter hover (click it to start a comment).
        let badge = if comment_end_lines.contains(&line_1) {
            Span::styled("💬", Style::default().fg(theme.accent))
        } else if comment_lines.contains(&line_1) {
            Span::styled("│ ", Style::default().fg(theme.accent))
        } else if is_gutter_hovered {
            Span::styled(
                "+ ",
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
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
                                party,
                            )
                        })
                        .unwrap_or_else(|| {
                            syntax_spans_for_line(vs, line_no, Some(diff_bg), theme.fg, party)
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
                syntax_spans_for_line(vs, line_no, diff_bg, theme.fg, party)
            }
        } else {
            syntax_spans_for_line(vs, line_no, gutter_bg, theme.fg, party)
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
                &app.syntax_set,
                &app.syntect_theme,
                &app.markdown_cache,
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

        // Inject the new-comment compose box under its anchored line.
        if remaining > 0 && compose_anchor_end == Some(line_1) {
            let compose = build_inline_compose_lines(
                app.review_state.input_kind,
                &app.review_state.input_buffer,
                gutter_width,
                area.width as usize,
                theme,
            );
            for (line, row_type) in compose {
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
    /// Party-mode rainbow phase (`None` when party mode is off).
    party: Option<f64>,
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
    let party = ctx.party;

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

    // Comment badge: 💬 on end lines, │ on earlier range lines, and a
    // GitHub-style "+" button on hovered gutter (click to start a comment).
    let badge = if new_line_no.is_some_and(|n| ctx.comment_end_lines.contains(&n)) {
        Span::styled("💬", Style::default().fg(theme.accent))
    } else if new_line_no.is_some_and(|n| ctx.comment_lines.contains(&n)) {
        Span::styled("│ ", Style::default().fg(theme.accent))
    } else if is_gutter_hovered {
        Span::styled(
            "+ ",
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
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
                                party,
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
                    syntax_spans_for_line(vs, line_no - 1, None, theme.fg, party)
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
                    syntax_spans_for_line(vs, line_no - 1, diff_bg, theme.fg, party)
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
                    syntax_spans_for_line(vs, line_no - 1, None, theme.fg, party)
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

/// Render the full change summary as a dedicated, scrollable, full-panel view —
/// the "SUMMARY" pseudo-file. This is the PR-description counterpart to the
/// line-anchored review comments; it gets the whole panel (no truncation) and
/// reuses the same j/k scroll the diff/file views use.
fn render_summary_view(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    let (block, lines): (Block, Vec<Line>) = {
        let theme = &app.theme;
        let border_color = if focused {
            theme.border_focused
        } else {
            theme.border_unfocused
        };
        let border_type = if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        };
        let title_style = if focused {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        let block = Block::default()
            .title(Span::styled(" \u{25A3} SUMMARY ", title_style))
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color));

        let summary = app
            .review_state
            .change_summary
            .as_deref()
            .unwrap_or("")
            .trim();

        let mut lines: Vec<Line> = Vec::new();
        if summary.is_empty() {
            for (text, _) in [
                ("(no change summary on this branch)", ()),
                ("", ()),
                ("Write one with the conductor `set_change_summary` MCP tool", ()),
                ("(e.g. via the /self-review skill).", ()),
            ] {
                lines.push(Line::from(Span::styled(
                    text,
                    Style::default().fg(theme.muted),
                )));
            }
        } else {
            // Render the summary as Markdown: headings/lists/quotes are
            // decorated and fenced code blocks are syntax-highlighted. Plain
            // text (no Markdown syntax) renders as ordinary paragraphs, so
            // existing summaries are unaffected.
            lines = crate::ui::markdown::render_markdown(
                summary,
                inner_width.saturating_sub(1),
                theme,
                &app.syntax_set,
                &app.syntect_theme,
            );
        }
        (block, lines)
    };

    // Record the total so the key handler can clamp scrolling, and write the
    // clamped scroll back so navigation stays responsive if the summary shrank.
    app.viewer_state.summary_total_lines = lines.len();
    let scroll = app
        .viewer_state
        .summary_scroll
        .min(lines.len().saturating_sub(1));
    app.viewer_state.summary_scroll = scroll;
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(inner_height).collect();

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Paragraph::new(visible).block(block), area);
}

fn render_diff_view(frame: &mut Frame, area: Rect, app: &mut App, block: Block<'_>) {
    let inner_height = area.height.saturating_sub(2) as usize;

    // Party-mode rainbow phase (None when off); computed before borrowing.
    let party = app.party_mode.then_some(app.ui_tick as f64 * 4.0);

    // Build the visible rows plus the screen-row → comment / entry maps. Inline
    // comment threads are injected after the last line of each commented range
    // (so review comments are visible right in the diff, expanded by default).
    let (lines, screen_row_map, screen_entry_map) = {
        let theme = &app.theme;
        let vs = &app.viewer_state;
        let tab_width = app.config.viewer.tab_width;
        let gutter_width = digit_count(vs.diff_view.diff_view_max_line_no);

        // Line numbers that have review comments (for the current file).
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
            party,
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

            // Inject the inline comment thread after the comment's last line.
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
                    &app.syntax_set,
                    &app.syntect_theme,
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

            // Inject the new-comment compose box under its anchored line.
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

    // Show selection hint overlay.
    let theme = &app.theme;
    let vs = &app.viewer_state;
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

    // Recover from a poisoned lock instead of panicking: the decode thread
    // holds this mutex while rendering, so a panic there (malformed media)
    // must not take down the whole TUI on the next frame.
    let content = vs
        .media_state
        .content
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

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

            render_media_info_bar(frame, info_area, dimensions, file_size, theme);
        }
        MediaContent::Pixel {
            protocol,
            dimensions,
            file_size,
        } => {
            frame.render_widget(ratatui::widgets::Clear, area);

            let inner = block.inner(area);
            frame.render_widget(block, area);

            // Reserve last line for info bar.
            let media_height = inner.height.saturating_sub(1);
            let media_area = Rect::new(inner.x, inner.y, inner.width, media_height);
            let info_area = Rect::new(inner.x, inner.y + media_height, inner.width, 1);

            // Pixel-quality image via the terminal graphics protocol. The
            // escape payload is embedded in the buffer cells, so ratatui's
            // diffing only re-transmits it when the cells actually change.
            frame.render_widget(ratatui_image::Image::new(protocol.as_ref()), media_area);

            render_media_info_bar(frame, info_area, dimensions, file_size, theme);
        }
        MediaContent::Error(msg) => {
            let error = Paragraph::new(msg)
                .style(Style::default().fg(theme.error))
                .block(block);
            frame.render_widget(error, area);
        }
    }
}

/// Render the media info bar (dimensions + file size) under the image.
fn render_media_info_bar(
    frame: &mut Frame,
    info_area: Rect,
    dimensions: (u32, u32),
    file_size: u64,
    theme: &crate::theme::Theme,
) {
    let size_str = if file_size >= 1_048_576 {
        format!("{:.1} MB", file_size as f64 / 1_048_576.0)
    } else if file_size >= 1024 {
        format!("{:.1} KB", file_size as f64 / 1024.0)
    } else {
        format!("{file_size} B")
    };
    let info = format!(" {}x{} | {} ", dimensions.0, dimensions.1, size_str);
    let info_widget = Paragraph::new(Span::styled(info, Style::default().fg(theme.muted)));
    frame.render_widget(info_widget, info_area);
}

/// Build inline thread rows for a comment at the given line.
///
/// Returns a vec of `Line`s representing the thread box:
/// top border, each comment + replies + action icons, bottom border.
#[allow(clippy::too_many_arguments)]
fn build_inline_thread_lines<'a>(
    line_1: usize,
    gutter_width: usize,
    panel_width: usize,
    review_state: &crate::review_state::ReviewState,
    reply_comment_id: Option<&str>,
    reply_buffer: &crate::text_input::TextInput,
    theme: &Theme,
    syntax_set: &syntect::parsing::SyntaxSet,
    syntect_theme: &syntect::highlighting::Theme,
    md_cache: &crate::ui::markdown::MarkdownCache,
) -> Vec<(Line<'a>, crate::viewer::ScreenRow)> {
    // An expanded thread shows ALL its comments, resolved included. Resolved
    // ones are merely collapsed *by default* (see `expand_threads_for_file`),
    // so once the user clicks the badge open they must be visible (with a
    // "resolved" marker in the byline) — otherwise the box renders empty.
    let comments: Vec<&crate::review_store::ReviewComment> =
        match review_state.file_comments.get(&line_1) {
            Some(c) if !c.is_empty() => c.iter().collect(),
            _ => return Vec::new(),
        };

    use crate::viewer::ScreenRow;
    let mut out: Vec<(Line, ScreenRow)> = Vec::new();
    let left_pad = gutter_width + 4 + 2; // gutter + badge
    let gutter_pad: String = " ".repeat(left_pad);
    let border_style = Style::default().fg(theme.accent);
    // Per-author surface tint so "who wrote this" reads at a glance: Claude's
    // comments/replies on the neutral surface, the user's on a distinct one.
    let author_bg = |a: crate::review_store::Author| match a {
        crate::review_store::Author::Claude => theme.comment_preview_bg,
        crate::review_store::Author::User => theme.comment_user_bg,
    };
    // Box inner width (between │ and │).
    let box_inner = panel_width.saturating_sub(left_pad + 4 + 2); // "  │ " left + " │" right
    // Indent inside the box, but never wider than the box itself (a fixed
    // floor of 20 used to overflow the border on narrow panels).
    let wrap_width = box_inner.saturating_sub(6).max(10).min(box_inner.max(1));

    // Helper: bordered content line with left │, filled to full width in `bg`.
    let make_line = |spans: Vec<Span<'a>>, bg: Color| -> (Line<'a>, ScreenRow) {
        let bg_style = Style::default().bg(bg);
        let mut all = vec![
            Span::styled(gutter_pad.clone(), bg_style),
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
            all.push(Span::styled(" ".repeat(remaining), bg_style));
        }
        (Line::from(all).style(bg_style), ScreenRow::ThreadContent)
    };

    // Helper: full-width border line, filled in `bg`.
    let make_border = |content: String, bg: Color| -> (Line<'a>, ScreenRow) {
        let bg_style = Style::default().bg(bg);
        let text = format!("{gutter_pad}{content}");
        let used = unicode_width::UnicodeWidthStr::width(text.as_str());
        let pad = panel_width.saturating_sub(used + 2);
        let mut spans = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled(content, border_style),
        ];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg_style));
        }
        (Line::from(spans).style(bg_style), ScreenRow::ThreadContent)
    };

    // Top border — tinted to the first comment's author.
    let top_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(
        format!("  ┌{top_fill}┐"),
        author_bg(comments[0].author),
    ));

    for (ci, comment) in comments.iter().enumerate() {
        // This comment's author surface; all of its rows use it.
        let cbg = author_bg(comment.author);
        let content_style = Style::default().fg(theme.fg).bg(cbg);
        let info_style = Style::default().fg(theme.info).bg(cbg);

        // Blank spacer line between comments in the same thread.
        if ci > 0 {
            out.push(make_line(vec![Span::styled("", content_style)], cbg));
        }

        let author_label = match comment.author {
            crate::review_store::Author::User => "you",
            crate::review_store::Author::Claude => "claude",
        };

        // Author byline: kind badge (💡/❓) + author, like a GitHub comment
        // header. A muted "✓ resolved" marker trails the byline for resolved
        // comments (these only appear when the thread is explicitly opened).
        let kind = crate::ui::review::kind_icon(comment.kind);
        let mut byline = vec![
            Span::styled(format!("{kind} "), content_style),
            Span::styled(
                author_label.to_string(),
                info_style.add_modifier(Modifier::BOLD),
            ),
        ];
        if comment.status == crate::review_store::CommentStatus::Resolved {
            byline.push(Span::styled(
                "  \u{2713} resolved".to_string(),
                Style::default().fg(theme.success).bg(cbg),
            ));
        }
        out.push(make_line(byline, cbg));

        // Comment body, rendered as GitHub-style Markdown onto the author
        // surface (headings, lists, fenced code cards, inline `code`, links, …).
        // Cached per comment id so it isn't re-parsed/highlighted every frame.
        let mut body_md =
            md_cache.render(&comment.id, &comment.body, wrap_width, theme, syntax_set, syntect_theme);
        crate::ui::markdown::apply_background(&mut body_md, cbg);
        for line in body_md {
            out.push(make_line(line.spans, cbg));
        }

        // Show replies if cached — each tinted to ITS OWN author, so a user
        // reply under a Claude comment (or vice-versa) is visibly distinct.
        if let Some(replies) = review_state.cached_replies.get(&comment.id) {
            for reply in replies {
                let rbg = author_bg(reply.author);
                let r_content = Style::default().fg(theme.fg).bg(rbg);
                let r_info = Style::default().fg(theme.info).bg(rbg);
                let reply_author = match reply.author {
                    crate::review_store::Author::User => "you",
                    crate::review_store::Author::Claude => "claude",
                };
                // Reply byline, indented under its parent with a ↳ marker.
                out.push(make_line(
                    vec![Span::styled(
                        format!("  \u{21b3} {reply_author}"),
                        r_info.add_modifier(Modifier::BOLD),
                    )],
                    rbg,
                ));
                // Reply body Markdown, indented two columns under the byline.
                // Cached per reply id.
                let mut reply_md = md_cache.render(
                    &reply.id,
                    &reply.body,
                    wrap_width.saturating_sub(2).max(1),
                    theme,
                    syntax_set,
                    syntect_theme,
                );
                crate::ui::markdown::apply_background(&mut reply_md, rbg);
                for line in reply_md {
                    let mut spans = vec![Span::styled("  ".to_string(), r_content)];
                    spans.extend(line.spans);
                    out.push(make_line(spans, rbg));
                }
            }
        }

        // Per-comment action icons row or active reply input.
        let is_replying_to_this = reply_comment_id == Some(comment.id.as_str());
        let action_row_type = ScreenRow::ThreadActions {
            comment_id: comment.id.clone(),
        };
        if is_replying_to_this {
            // GitHub-style multi-line reply form: a byline, the buffer rendered
            // line by line with a block cursor, then a key hint. The thread above
            // stays visible, so the parent comment is always in view while typing.
            let muted = Style::default().fg(theme.muted).bg(cbg);
            out.push(make_line(
                vec![Span::styled(
                    "\u{21b3} reply".to_string(),
                    info_style.add_modifier(Modifier::BOLD),
                )],
                cbg,
            ));
            if reply_buffer.is_empty() {
                out.push(make_line(
                    vec![
                        Span::styled("> ".to_string(), Style::default().fg(theme.accent).bg(cbg)),
                        Span::styled("Type reply\u{2026}".to_string(), muted),
                    ],
                    cbg,
                ));
            } else {
                // Block cursor sits between before/after text, like the modal.
                let display = format!(
                    "{}\u{2588}{}",
                    reply_buffer.text_before_cursor(),
                    reply_buffer.text_after_cursor()
                );
                for (li, seg) in display.split('\n').enumerate() {
                    let prefix = if li == 0 { "> " } else { "  " };
                    out.push(make_line(
                        vec![
                            Span::styled(
                                prefix.to_string(),
                                Style::default().fg(theme.accent).bg(cbg),
                            ),
                            Span::styled(seg.to_string(), content_style),
                        ],
                        cbg,
                    ));
                }
            }
            out.push(make_line(
                vec![Span::styled(
                    "Shift+Enter: newline  \u{b7}  Enter: send  \u{b7}  Esc: cancel".to_string(),
                    muted,
                )],
                cbg,
            ));
        } else {
            // Clickable action row. Labels and hit ranges both come from the
            // shared `thread_actions` module so the mouse handler stays in
            // sync with what is drawn here.
            let bg_style = Style::default().bg(cbg);
            let muted_style = Style::default().fg(theme.muted).bg(cbg);
            let reply_style = Style::default().fg(theme.info).bg(cbg);
            let resolve_style = Style::default().fg(theme.success).bg(cbg);
            let delete_style = Style::default().fg(theme.error).bg(cbg);
            let claude_style = Style::default().fg(Color::Rgb(180, 140, 255)).bg(cbg);
            let status_label = match comment.status {
                crate::review_store::CommentStatus::Pending => thread_actions::RESOLVE,
                crate::review_store::CommentStatus::Resolved => thread_actions::UNRESOLVE,
            };
            // Pad the status slot to a constant width so "delete" starts at a
            // stable column regardless of resolve/unresolve being shown.
            let status_pad = thread_actions::status_slot_width()
                .saturating_sub(unicode_width::UnicodeWidthStr::width(status_label));

            let gap = " ".repeat(thread_actions::GAP);
            let left_actions = vec![
                Span::styled(thread_actions::REPLY, reply_style),
                Span::styled(gap.clone(), muted_style),
                Span::styled(
                    format!("{status_label}{}", " ".repeat(status_pad)),
                    resolve_style,
                ),
                Span::styled(gap, muted_style),
                Span::styled(thread_actions::DELETE, delete_style),
            ];
            let right_label = thread_actions::ASK_CLAUDE;
            let right_label_w = thread_actions::ask_claude_width();

            let left_w: usize = left_actions
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let prefix_w = left_pad + 4; // gutter_pad + "  │ "
            let fill = panel_width.saturating_sub(prefix_w + left_w + right_label_w + 2 + 1);

            let mut spans = vec![
                Span::styled(gutter_pad.clone(), bg_style),
                Span::styled("  │ ", border_style),
            ];
            spans.extend(left_actions);
            if fill > 0 {
                spans.push(Span::styled(" ".repeat(fill), bg_style));
            }
            spans.push(Span::styled(right_label.to_string(), claude_style));
            spans.push(Span::styled(" ", bg_style)); // trailing pad

            let line = Line::from(spans).style(bg_style);
            out.push((line, action_row_type));
        }
    }

    // Bottom border — tinted to the last comment's author.
    let bot_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(
        format!("  └{bot_fill}┘"),
        author_bg(comments[comments.len() - 1].author),
    ));

    out
}

/// The line under which the new-comment compose box should render, if a new
/// comment is being composed and its anchor is in the current file. Returns the
/// end line of the anchored range (where the box is injected).
fn new_comment_anchor_end(app: &App) -> Option<usize> {
    if app.review_state.input_mode != crate::review_state::ReviewInputMode::AddingComment {
        return None;
    }
    let (file, start, end) = app.review_state.input_anchor.as_ref()?;
    if Some(file.as_str()) != app.viewer_state.content.current_file.as_deref() {
        return None;
    }
    Some(end.unwrap_or(*start) as usize)
}

/// Build the inline **new-comment** compose box, injected under the anchored
/// line when `ReviewInputMode::AddingComment` is active. A GitHub-style form:
/// a kind header, the body buffer with a block cursor, and a key hint — drawn
/// on the user surface (`comment_user_bg`) like a user-authored comment.
fn build_inline_compose_lines<'a>(
    kind: crate::review_store::CommentKind,
    input: &crate::text_input::TextInput,
    gutter_width: usize,
    panel_width: usize,
    theme: &Theme,
) -> Vec<(Line<'a>, crate::viewer::ScreenRow)> {
    use crate::viewer::ScreenRow;
    let bg = theme.comment_user_bg;
    let left_pad = gutter_width + 4 + 2;
    let gutter_pad: String = " ".repeat(left_pad);
    let border_style = Style::default().fg(theme.accent);
    let bg_style = Style::default().bg(bg);
    let content_style = Style::default().fg(theme.fg).bg(bg);
    let muted = Style::default().fg(theme.muted).bg(bg);
    let accent_bg = Style::default().fg(theme.accent).bg(bg);
    let box_inner = panel_width.saturating_sub(left_pad + 4 + 2);

    let make_line = |spans: Vec<Span<'a>>| -> (Line<'a>, ScreenRow) {
        let mut all = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled("  │ ", border_style),
        ];
        all.extend(spans);
        let used: usize = all
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let rem = panel_width.saturating_sub(used + 2);
        if rem > 0 {
            all.push(Span::styled(" ".repeat(rem), bg_style));
        }
        (Line::from(all).style(bg_style), ScreenRow::ThreadContent)
    };
    let make_border = |content: String| -> (Line<'a>, ScreenRow) {
        let used = unicode_width::UnicodeWidthStr::width(format!("{gutter_pad}{content}").as_str());
        let pad = panel_width.saturating_sub(used + 2);
        let mut spans = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled(content, border_style),
        ];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg_style));
        }
        (Line::from(spans).style(bg_style), ScreenRow::ThreadContent)
    };

    let mut out = Vec::new();
    let top_fill = "\u{2500}".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  \u{250c}{top_fill}\u{2510}")));

    // Kind header (toggle with Tab).
    let (icon, label) = match kind {
        crate::review_store::CommentKind::Suggest => ("\u{1f4a1}", "New Suggest"),
        crate::review_store::CommentKind::Question => ("\u{2753}", "New Question"),
    };
    out.push(make_line(vec![
        Span::styled(format!("{icon} "), content_style),
        Span::styled(
            label.to_string(),
            Style::default().fg(theme.info).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (Tab: toggle kind)".to_string(), muted),
    ]));

    // Body with a block cursor between before/after text.
    if input.is_empty() {
        out.push(make_line(vec![
            Span::styled("> ".to_string(), accent_bg),
            Span::styled("Write a comment\u{2026}".to_string(), muted),
        ]));
    } else {
        let display = format!(
            "{}\u{2588}{}",
            input.text_before_cursor(),
            input.text_after_cursor()
        );
        for (li, seg) in display.split('\n').enumerate() {
            let prefix = if li == 0 { "> " } else { "  " };
            out.push(make_line(vec![
                Span::styled(prefix.to_string(), accent_bg),
                Span::styled(seg.to_string(), content_style),
            ]));
        }
    }

    out.push(make_line(vec![Span::styled(
        "Shift+Enter: newline  \u{b7}  Enter: submit  \u{b7}  Esc: cancel".to_string(),
        muted,
    )]));
    let bot_fill = "\u{2500}".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  \u{2514}{bot_fill}\u{2518}")));
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
    party: Option<f64>,
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

    // Party mode: recolour the merged tokens with a flowing rainbow while
    // keeping their diff backgrounds intact.
    if let Some(phase) = party {
        for (idx, span) in result.iter_mut().enumerate() {
            span.style.fg = Some(crate::ui::party::rainbow(phase + idx as f64 * 23.0));
        }
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
    party: Option<f64>,
) -> Vec<Span<'static>> {
    if let Some(tokens) = vs.content.highlighted_lines.get(line_no) {
        tokens
            .iter()
            .enumerate()
            .map(|(idx, (style, text))| {
                let mut s = if let Some(bg) = diff_bg {
                    // Keep token fg, override bg with diff colour.
                    style.bg(bg)
                } else {
                    *style
                };
                // Party mode: recolour each token (boundaries preserved) with a
                // flowing rainbow so the whole line goes flashy.
                if let Some(phase) = party {
                    s.fg = Some(crate::ui::party::rainbow(
                        phase + line_no as f64 * 7.0 + idx as f64 * 23.0,
                    ));
                }
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
        let color = match party {
            Some(phase) => crate::ui::party::rainbow(phase + line_no as f64 * 7.0),
            None => fg,
        };
        vec![Span::styled(text, Style::default().fg(color))]
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
            None,
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
            None,
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
