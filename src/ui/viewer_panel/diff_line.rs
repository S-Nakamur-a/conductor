//! Rendering of a single unified-diff content line (context / addition /
//! deletion row), including gutter, comment badge, syntax/word-diff styled
//! content, and GitHub-style full-width background fill.

use crate::diff_state::{DiffLineTag, InlineSegment};
use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::span_utils::h_scroll_spans;
use super::syntax::{merge_syntax_with_inline, render_inline_diff_spans, syntax_spans_for_line};

/// Shared per-frame context for rendering a single unified-diff content line.
pub(super) struct DiffLineRenderCtx<'a> {
    pub(super) vs: &'a crate::viewer::ViewerState,
    pub(super) theme: &'a Theme,
    pub(super) gutter_width: usize,
    pub(super) tab_width: usize,
    pub(super) area_width: u16,
    pub(super) comment_lines: &'a std::collections::HashSet<usize>,
    pub(super) comment_end_lines: &'a std::collections::HashSet<usize>,
    /// Inclusive new-side line range of the walkthrough step currently
    /// selected in review mode, when it points at the file being rendered.
    /// `None` outside review mode, with no walkthrough, or for other files.
    pub(super) walkthrough_highlight: Option<(usize, usize)>,
    /// Party-mode rainbow phase (`None` when party mode is off).
    pub(super) party: Option<f64>,
}

/// Build the display line for a single diff content line (context / addition /
/// deletion), including the gutter, comment badge, syntax/word-diff styled
/// content, and GitHub-style full-width background fill.
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
    let is_in_walkthrough_highlight = new_line_no.is_some_and(|n| {
        ctx.walkthrough_highlight
            .is_some_and(|(lo, hi)| n >= lo && n <= hi)
    });

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

    // Comment-marker column (far left, BEFORE the line numbers): 💬 on end
    // lines, │ on earlier range lines. Clicking it toggles the thread.
    let marker = if new_line_no.is_some_and(|n| ctx.comment_end_lines.contains(&n)) {
        Span::styled("💬", Style::default().fg(theme.accent))
    } else if new_line_no.is_some_and(|n| ctx.comment_lines.contains(&n)) {
        Span::styled("│ ", Style::default().fg(theme.accent))
    } else {
        Span::raw("  ")
    };

    // Badge column (right of the line numbers): a GitHub-style "+" button on
    // hovered gutter (click to start a comment) — regardless of existing
    // comments. (The diff view draws no ▶ test markers.)
    let badge = if is_gutter_hovered {
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

    // Apply horizontal scroll, clipping to panel width
    // (borders + marker column + gutter + badge).
    let content_max_w = (ctx.area_width as usize)
        .saturating_sub(crate::viewer::COMMENT_MARKER_W as usize + gutter_width + 8);
    let content_spans = h_scroll_spans(content_spans, vs.content.h_scroll, content_max_w);

    // Underline the current walkthrough step's line range — a highlight that
    // doesn't fight the existing selection/diff background colors, since it
    // only lasts while this step stays current.
    let content_spans: Vec<Span> = if is_in_walkthrough_highlight {
        content_spans
            .into_iter()
            .map(|s| Span::styled(s.content, s.style.add_modifier(Modifier::UNDERLINED)))
            .collect()
    } else {
        content_spans
    };

    let mut spans = vec![marker, gutter_span, badge];
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
