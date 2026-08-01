//! Unified diff mode of the viewer panel — the GitHub-style diff view used
//! when browsing an unstaged/staged/committed change instead of a plain file.

use crate::app::App;
use crate::theme::Theme;
use crate::viewer::UnifiedDiffEntry;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::comment_thread::{build_inline_compose_lines, build_inline_thread_lines, new_comment_anchor_end};
use super::diff_line::{render_diff_content_line, DiffLineRenderCtx};
use super::search_box::render_search_box;
use super::span_utils::digit_count;

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

/// When the Explorer is in walkthrough-reading mode and the Viewer is showing
/// the file the *selected* walkthrough step is anchored to, build a full-width
/// banner (step title + Markdown-rendered body) to sit above the diff. This is
/// the fix for the walkthrough's explanation being confined to the narrow
/// Explorer pane: here the prose reads at full Viewer width, right above the
/// code it describes. Returns `None` (no banner) unless we're actively touring
/// this step's file.
pub(super) fn build_walkthrough_banner(app: &App, width: u16) -> Option<(String, Vec<Line<'static>>)> {
    if app.viewer_state.explorer.explorer_bottom_view
        != crate::viewer::ExplorerBottomView::Walkthrough
    {
        return None;
    }
    let steps = &app.walkthrough.current.as_ref()?.steps;
    // The banner follows the *jumped-to* step, not the list cursor, so
    // browsing the step list with j/k leaves the Viewer untouched.
    let step = steps.get(app.viewer_state.explorer.walkthrough_viewing?)?;
    // Only when the diff on screen is actually this step's file.
    if app.viewer_state.content.current_file.as_deref() != Some(step.file_path.as_str()) {
        return None;
    }
    let title = format!(
        " {} {} — {} ",
        crate::ui::walkthrough_pane::step_icon(step.kind),
        step.kind,
        step.title
    );
    let lines = crate::ui::markdown::render_markdown(
        &step.body,
        (width as usize).saturating_sub(3),
        &app.theme,
        &app.highlight.syntax_set,
        &app.highlight.theme,
    );
    Some((title, lines))
}

/// Render the walkthrough step banner into `area`: a titled box holding the
/// step's Markdown body, clipped to the available height with a hint pointing
/// at the `space` full-text overlay when the body overflows.
pub(super) fn render_walkthrough_banner(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    lines: &[Line<'static>],
) {
    let theme = &app.theme;
    let block = Block::default()
        .title(Span::styled(
            title.to_string(),
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.border_secondary));
    let inner_h = area.height.saturating_sub(2) as usize;
    let visible: Vec<Line> = if inner_h > 0 && lines.len() > inner_h {
        let mut v: Vec<Line> = lines.iter().take(inner_h.saturating_sub(1)).cloned().collect();
        v.push(Line::from(Span::styled(
            "…  (space: 全文)",
            Style::default().fg(theme.muted),
        )));
        v
    } else {
        lines.to_vec()
    };
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Paragraph::new(visible).block(block), area);
}

/// Render the unified diff view (GitHub-style).
pub(super) fn render_diff_view(frame: &mut Frame, area: Rect, app: &mut App, block: Block<'_>) {
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

        // The selected walkthrough step's line range, if it's anchored to
        // the file currently open in this pane.
        let walkthrough_highlight = (|| {
            let steps = &app.walkthrough.current.as_ref()?.steps;
            // Underline the jumped-to step's range, not the list cursor's, so
            // it stays put while j/k only moves the Explorer selection.
            let step = steps.get(vs.explorer.walkthrough_viewing?)?;
            if vs.content.current_file.as_deref() != Some(step.file_path.as_str()) {
                return None;
            }
            let start = step.line_start?;
            let end = step.line_end.unwrap_or(start);
            Some((start as usize, end as usize))
        })();

        let line_ctx = DiffLineRenderCtx {
            vs,
            theme,
            gutter_width,
            tab_width,
            area_width: area.width,
            comment_lines: &comment_lines,
            comment_end_lines: &comment_end_lines,
            walkthrough_highlight,
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

    // Render scrollbar when the diff has more rows than fit in the panel —
    // same trigger and look as the Explorer file tree.
    if vs.diff_view.diff_view_lines.len() > inner_height {
        let scrollbar_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        let mut scrollbar_state =
            ScrollbarState::new(vs.diff_view.diff_view_lines.len().saturating_sub(inner_height))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    /// Concatenate all span contents of a line into a single string.
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
