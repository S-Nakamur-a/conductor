//! Per-line row building for the plain (non-diff) file view: builds the code
//! line's spans plus any inline comment-thread / new-comment compose rows
//! anchored below it.

use crate::app::App;
use crate::diff_state::{DiffLineTag, InlineSegment};
use crate::theme::Theme;
use crate::viewer::ScreenRow;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::comment_thread::{build_inline_compose_lines, build_inline_thread_lines};
use super::span_utils::{apply_hint_labels, apply_underline_range, h_scroll_spans};
use super::syntax::{merge_syntax_with_inline, render_inline_diff_spans, syntax_spans_for_line};

/// Shared per-frame context for rendering the plain file view's code lines.
pub(super) struct FileLineRenderCtx<'a> {
    pub(super) vs: &'a crate::viewer::ViewerState,
    pub(super) theme: &'a Theme,
    pub(super) tab_width: usize,
    pub(super) area_width: u16,
    pub(super) gutter_width: usize,
    pub(super) diff_annotations:
        &'a std::collections::HashMap<usize, (DiffLineTag, Vec<InlineSegment>)>,
    pub(super) comment_lines: &'a std::collections::HashSet<usize>,
    pub(super) comment_end_lines: &'a std::collections::HashSet<usize>,
    /// Party-mode rainbow phase (`None` when party mode is off).
    pub(super) party: Option<f64>,
}

/// Build the row(s) for one source line: the code line itself, plus any
/// inline comment-thread or new-comment compose rows anchored below it.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_code_line_rows(
    app: &App,
    ctx: &FileLineRenderCtx,
    line_no: usize,
    content: &str,
    expanded_threads: &std::collections::HashSet<usize>,
    inline_reply_line: Option<usize>,
    compose_anchor_end: Option<usize>,
) -> Vec<(Line<'static>, ScreenRow)> {
    let vs = ctx.vs;
    let theme = ctx.theme;
    let tab_width = ctx.tab_width;
    let party = ctx.party;
    let gutter_width = ctx.gutter_width;

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
    let annotation = ctx.diff_annotations.get(&line_1);
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

    // Comment-marker column (far left, BEFORE the line numbers): 💬 on
    // the last line of a comment range, │ on earlier lines in the range.
    // Clicking it toggles the thread — kept out of the gutter/badge side
    // so starting a new comment works identically on every line.
    let marker = if ctx.comment_end_lines.contains(&line_1) {
        Span::styled("💬", Style::default().fg(theme.accent))
    } else if ctx.comment_lines.contains(&line_1) {
        Span::styled("│ ", Style::default().fg(theme.accent))
    } else {
        Span::raw("  ")
    };

    // Badge column (right of the line numbers): ▶ on runnable test lines,
    // otherwise a GitHub-style "+" button on gutter hover (click it to
    // start a comment) — shown regardless of existing comments.
    let badge = if vs.content.test_runs.contains_key(&line_1) {
        // Runnable test line: a ▶ button that sends the test command
        // (`go test …` / `cargo test …`) to the Shell PTY (handled in
        // event/mouse.rs).
        Span::styled(
            "\u{25b6} ",
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        )
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
                render_inline_diff_spans(ann_segments, diff_bg, emphasis_bg, theme.fg, tab_width)
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

    // Apply horizontal scroll to content spans, clipping to panel width
    // (borders + marker column + gutter + badge).
    let content_max_w = (ctx.area_width as usize)
        .saturating_sub(crate::viewer::COMMENT_MARKER_W as usize + gutter_width + 8);
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

    let mut spans = vec![marker, gutter_span, badge];
    spans.extend(content_spans);

    let mut rows: Vec<(Line<'static>, ScreenRow)> =
        vec![(Line::from(spans), ScreenRow::Code(line_1))];

    // Inline thread rows below the LAST line of a comment range.
    if expanded_threads.contains(&line_1) {
        let reply_cid = if inline_reply_line == Some(line_1) {
            app.viewer_state.explorer.inline_reply_comment_id.as_deref()
        } else {
            None
        };
        let thread_lines = build_inline_thread_lines(
            line_1,
            gutter_width,
            ctx.area_width as usize,
            &app.review_state,
            reply_cid,
            &app.viewer_state.explorer.inline_reply_buffer,
            theme,
            &app.syntax_set,
            &app.syntect_theme,
            &app.markdown_cache,
        );
        rows.extend(thread_lines);
    }

    // The new-comment compose box under its anchored line.
    if compose_anchor_end == Some(line_1) {
        let compose = build_inline_compose_lines(
            app.review_state.input_kind,
            &app.review_state.input_buffer,
            gutter_width,
            ctx.area_width as usize,
            theme,
        );
        rows.extend(compose);
    }

    rows
}
