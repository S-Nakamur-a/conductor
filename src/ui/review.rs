//! Review overlay renderers — input box and template picker.
//!
//! These are rendered as overlays on top of the main layout when active.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::review_state::{ReviewInputMode, ReviewState};
use crate::review_store::CommentKind;

/// Render an input box overlay when adding or editing a comment.
pub fn render_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let popup_height = 5_u16;
    let popup_width = area.width.saturating_sub(8).min(80);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + area.height.saturating_sub(popup_height + 2);
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let title = match app.review_state.input_mode {
        ReviewInputMode::AddingComment => {
            let kind_label = match app.review_state.input_kind {
                CommentKind::Suggest => "Suggest",
                CommentKind::Question => "Question",
            };
            format!(" New {kind_label} (Tab: toggle, file:line body) ")
        }
        ReviewInputMode::EditingComment => " Edit Comment ".to_string(),
        ReviewInputMode::ReplyingToComment => " Reply to Comment ".to_string(),
        ReviewInputMode::Normal => unreachable!(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let input_text = format!("{}\u{2588}", app.review_state.input_buffer);
    let paragraph = Paragraph::new(Span::styled(
        input_text,
        Style::default().fg(Color::White),
    ))
    .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Render a centered popup for the comment template picker.
pub fn render_template_picker_overlay(frame: &mut Frame, area: Rect, state: &ReviewState) {
    let popup_width = 60_u16.min(area.width.saturating_sub(4));
    let popup_height = 15_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Templates (Enter: use, x: delete, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if state.templates.is_empty() {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  No templates saved. Use T to save a comment as template.",
            Style::default().fg(Color::DarkGray),
        )]));
        frame.render_widget(empty, inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, tmpl) in state.templates.iter().enumerate() {
        let is_selected = i == state.template_selected;

        let kind_badge = match tmpl.kind {
            CommentKind::Suggest => {
                Span::styled("[S] ", Style::default().fg(Color::Green))
            }
            CommentKind::Question => {
                Span::styled("[Q] ", Style::default().fg(Color::Magenta))
            }
        };

        let max_body_len =
            (popup_width as usize).saturating_sub(tmpl.name.chars().count() + 10);
        let body_preview: String = tmpl.body.chars().take(max_body_len).collect();
        let body_preview = body_preview.replace('\n', " ");

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let prefix = if is_selected { "> " } else { "  " };

        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            kind_badge,
            Span::styled(&tmpl.name, style),
        ]));
        lines.push(Line::from(vec![Span::styled(
            format!("    {body_preview}"),
            Style::default().fg(Color::DarkGray),
        )]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
