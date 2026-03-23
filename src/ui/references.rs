//! References overlay popup — shows search results for "Find References".

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::app::App;

/// Render the references overlay popup centered over `area`.
pub fn render_references_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let overlay = &app.references_overlay;
    let theme = &app.theme;

    // Calculate popup dimensions: 70% width, 60% height, centered.
    let popup_width = (area.width as f32 * 0.7).clamp(40.0, 100.0) as u16;
    let popup_height = (area.height as f32 * 0.6).clamp(10.0, 40.0) as u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let title = format!(" References: {} ({} results) ", overlay.symbol_name, overlay.results.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if overlay.results.is_empty() {
        let msg = Paragraph::new("No references found.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(msg, inner);
        return;
    }

    // Split inner area: list area + hint line at bottom.
    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let list_area = chunks[0];
    let hint_area = chunks[1];

    let visible_height = list_area.height as usize;
    let scroll = overlay.scroll;

    let items: Vec<ListItem> = overlay
        .results
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, reference)| {
            let is_selected = i == overlay.selected;
            let file_span = Span::styled(
                format!("{}:{}", reference.file_path, reference.line),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() }),
            );
            let content_span = Span::styled(
                format!("  {}", reference.content.trim()),
                Style::default().fg(if is_selected { theme.fg } else { theme.muted }),
            );
            let style = if is_selected {
                Style::default().bg(theme.selected_bg).fg(theme.fg)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![file_span, content_span])).style(style)
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, list_area);

    // Hint line.
    let hint = Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(theme.accent)),
        Span::styled(": navigate  ", Style::default().fg(theme.muted)),
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::styled(": jump  ", Style::default().fg(theme.muted)),
        Span::styled("Esc", Style::default().fg(theme.accent)),
        Span::styled(": close", Style::default().fg(theme.muted)),
    ]));
    frame.render_widget(hint, hint_area);
}
