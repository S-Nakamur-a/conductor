//! Worktree panel — left-most column showing the worktree list.
//!
//! Displays the list of worktrees with selection, status indicators,
//! and creation/deletion UI overlays.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, Focus};

/// Render the worktree panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Worktree;
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .title(" Worktrees ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    // Pulse phase: ~1s cycle at 60fps (30 frames on, 30 frames off).
    let pulse_on = (app.ui_tick / 30) % 2 == 0;

    let items: Vec<ListItem> = app
        .worktrees
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let is_waiting = app.cc_waiting_worktrees.contains(&wt.path);

            let marker = if wt.is_main {
                "\u{25cf}" // ●
            } else if i == app.selected_worktree {
                "\u{25c9}" // ◉
            } else {
                "\u{25cb}" // ○
            };

            let marker_style = if is_waiting {
                Style::default()
                    .fg(if pulse_on { Color::Yellow } else { Color::Cyan })
                    .add_modifier(Modifier::BOLD)
            } else if i == app.selected_worktree {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let status_text = if wt.is_clean {
                "\u{2713}".to_string()
            } else {
                format!("+{} ~{} -{}", wt.added, wt.modified, wt.deleted)
            };

            let branch_style = if is_waiting {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if i == app.selected_worktree {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            };

            let mut spans = vec![
                Span::styled(format!(" {marker} "), marker_style),
                Span::styled(wt.branch.clone(), branch_style),
            ];

            // Prominent waiting indicator with pulse animation.
            if is_waiting {
                let indicator = if pulse_on { " \u{25c6}" } else { " \u{25c7}" }; // ◆ / ◇
                spans.push(Span::styled(
                    indicator,
                    Style::default()
                        .fg(if pulse_on { Color::Yellow } else { Color::Cyan })
                        .add_modifier(Modifier::BOLD),
                ));
            }

            spans.push(Span::styled(
                format!(" {status_text}"),
                if wt.is_clean {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Magenta)
                },
            ));

            let item = ListItem::new(Line::from(spans));

            // Apply background highlight to the entire row when waiting.
            if is_waiting {
                let bg = if pulse_on {
                    Color::Rgb(0, 50, 65)
                } else {
                    Color::Rgb(0, 30, 40)
                };
                item.style(Style::default().bg(bg))
            } else {
                item
            }
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    state.select(Some(app.selected_worktree));

    frame.render_stateful_widget(list, area, &mut state);
}
