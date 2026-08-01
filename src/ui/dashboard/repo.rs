//! Repository selection overlays: switch-repo picker, open-repo path input,
//! and the PR-review intake input.

use super::input::{format_input_with_cursor, set_cursor_for_input};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

/// Render the repo selector overlay.
pub fn render_repo_selector_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 50_u16.min(area.width.saturating_sub(4));
    let content_lines = app.repo.known.len() as u16;
    let popup_height = (content_lines + 2)
        .min(12)
        .min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Switch Repository (Enter: select, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.repo.known.is_empty() {
        let paragraph =
            Paragraph::new("  No repositories configured.").style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .repo.known
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            let full_path = path.display().to_string();

            let active_marker = if i == app.repo.known_index {
                "\u{25cf} "
            } else {
                "  "
            };

            let style = if i == app.overlays.repo_selector.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let line = Line::from(vec![
                Span::styled(
                    format!(" {active_marker}"),
                    if i == app.repo.known_index {
                        Style::default().fg(theme.success)
                    } else {
                        Style::default().fg(theme.muted)
                    },
                ),
                Span::styled(name, style),
                Span::styled(format!("  {full_path}"), Style::default().fg(theme.muted)),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.overlays.repo_selector.selected));

    frame.render_stateful_widget(list, inner, &mut state);
}

/// Render the "open repository" path input overlay.
pub fn render_open_repo_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 5_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Open Repository (Enter: open, Esc: cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let input_text = format_input_with_cursor(&app.overlays.open_repo.buffer);
    let paragraph = Paragraph::new(Span::styled(input_text, Style::default().fg(theme.fg)))
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
    set_cursor_for_input(frame, inner, &app.overlays.open_repo.buffer);
}

/// Render the PR-number/URL input overlay ("Review: Review Pull Request…").
///
/// Below the input line, shows either a loading notice while the background
/// gh/git intake runs, or the last failure's message — the overlay stays
/// open on failure (input preserved) so the user can correct and retry.
pub fn render_pr_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let overlay = &app.overlays.pr_input;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 6_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let title = if overlay.loading {
        " Review Pull Request (fetching...) "
    } else {
        " Review Pull Request (Enter: fetch & review, Esc: cancel) "
    };
    let border_color = if overlay.error.is_some() {
        theme.error
    } else {
        theme.border_focused
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);

    let input_text = format_input_with_cursor(&overlay.buffer);
    let input_style = if overlay.loading {
        Style::default().fg(theme.muted)
    } else {
        Style::default().fg(theme.fg)
    };
    frame.render_widget(Paragraph::new(Span::styled(input_text, input_style)), chunks[0]);
    if !overlay.loading {
        set_cursor_for_input(frame, chunks[0], &overlay.buffer);
    }

    let message = if overlay.loading {
        Some(("Fetching PR metadata via gh...".to_string(), theme.muted))
    } else {
        overlay
            .error
            .as_ref()
            .map(|e| (e.clone(), theme.error))
    };
    if let Some((text, color)) = message {
        let paragraph =
            Paragraph::new(Span::styled(text, Style::default().fg(color)))
                .wrap(ratatui::widgets::Wrap { trim: false });
        frame.render_widget(paragraph, chunks[1]);
    }
}
