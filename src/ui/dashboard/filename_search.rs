//! Fuzzy filename-search ("jump to file") overlay.

use super::input::{format_input_with_cursor, set_cursor_for_input};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

/// Render the fuzzy filename-search ("jump to file") modal as a centered popup.
///
/// Rendered at the top level so it stays visible regardless of which panel is
/// focused or maximized — in particular, when the viewer is maximized and the
/// file tree column is collapsed to zero width.
pub fn render_filename_search_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let vs = &app.viewer_state.filename_search;

    let popup_width = 80_u16.min(area.width.saturating_sub(4));
    let popup_height = 24_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let chunks = Layout::vertical([
        Constraint::Length(3), // Search input
        Constraint::Min(1),    // Results
    ])
    .split(popup_area);

    // Search input.
    let total_files = vs.filename_search_all_files.len();
    let match_count = vs.filename_search_results.len();
    let input_block = Block::default()
        .title(format!(
            " Jump to file ({match_count}/{total_files}) — Enter: open, Esc: cancel "
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let input_inner = input_block.inner(chunks[0]);
    frame.render_widget(input_block, chunks[0]);

    let query_text = format_input_with_cursor(&vs.filename_search_query);
    frame.render_widget(
        Paragraph::new(Span::styled(query_text, Style::default().fg(theme.fg))),
        input_inner,
    );
    set_cursor_for_input(frame, input_inner, &vs.filename_search_query);

    // Results list.
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    if vs.filename_search_results.is_empty() {
        let msg = if vs.filename_search_query.is_empty() {
            "  Type to search files…"
        } else {
            "  No matches."
        };
        frame.render_widget(
            Paragraph::new(msg).style(Style::default().fg(theme.muted)),
            list_inner,
        );
        return;
    }

    let items: Vec<ListItem> = vs
        .filename_search_results
        .iter()
        .enumerate()
        .map(|(i, result)| {
            let selected = i == vs.filename_search_selected;
            let style = if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let line = Line::from(vec![
                Span::styled(
                    if selected { " > " } else { "   " },
                    Style::default().fg(theme.accent),
                ),
                Span::styled(result.path.clone(), style),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(Some(vs.filename_search_selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}
