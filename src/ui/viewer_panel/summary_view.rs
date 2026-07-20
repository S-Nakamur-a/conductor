//! The "SUMMARY" pseudo-file view — the full-panel branch change-summary
//! renderer, counterpart to the line-anchored review comments.

use crate::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

/// Render the full change summary as a dedicated, scrollable, full-panel view —
/// the "SUMMARY" pseudo-file. This is the PR-description counterpart to the
/// line-anchored review comments; it gets the whole panel (no truncation) and
/// reuses the same j/k scroll the diff/file views use.
pub(super) fn render_summary_view(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
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
