//! Symbol action overlay — shows navigation options for a selected symbol.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

/// Render the symbol action overlay centered over `area`.
pub fn render_symbol_action_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let overlay = &app.symbol_action_overlay;
    let theme = &app.theme;

    if overlay.actions.is_empty() {
        return;
    }

    // Calculate popup dimensions.
    let content_width = overlay
        .actions
        .iter()
        .map(|a| {
            // [d] Go to definition  src/app.rs:123
            format!("[{}] {}  {}:{}", a.key, a.label, a.file_path, a.line).len()
        })
        .max()
        .unwrap_or(30)
        + 4; // padding
    let popup_width = (content_width as u16).clamp(30, area.width.saturating_sub(4));
    let popup_height = (overlay.actions.len() as u16 + 3).min(area.height.saturating_sub(2)); // +3 = borders + hint line
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let title = format!(" {} ", overlay.symbol_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = overlay
        .actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let is_selected = i == overlay.selected;
            let key_style = Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD);
            let label_style = if is_selected {
                Style::default().fg(theme.fg).bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.fg)
            };
            let path_style = if is_selected {
                Style::default().fg(theme.muted).bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.muted)
            };
            Line::from(vec![
                Span::styled(format!("[{}] ", action.key), key_style),
                Span::styled(format!("{:<30}", action.label), label_style),
                Span::styled(format!("{}:{}", action.file_path, action.line), path_style),
            ])
        })
        .collect();

    // Hint line.
    if inner.height as usize > lines.len() {
        lines.push(Line::from(vec![
            Span::styled("d/i/r", Style::default().fg(theme.accent)),
            Span::styled(": jump  ", Style::default().fg(theme.muted)),
            Span::styled("Esc", Style::default().fg(theme.accent)),
            Span::styled(": cancel", Style::default().fg(theme.muted)),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
