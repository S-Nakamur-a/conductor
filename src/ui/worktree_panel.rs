//! Worktree panel — left-most column showing the worktree list.
//!
//! Displays the list of worktrees with selection, status indicators,
//! and creation/deletion UI overlays.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, Focus};

/// Render the worktree panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Worktree;
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };

    let is_expanded = app.expanded_panel == Some(Focus::Worktree);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", Color::Yellow)
    } else {
        ("[<=>]", Color::DarkGray)
    };

    let title = if app.grabbed_branch.is_some() {
        " Worktrees [GRABBED] "
    } else {
        " Worktrees "
    };
    let title_style = if app.grabbed_branch.is_some() {
        Style::default().fg(Color::Rgb(255, 165, 0)).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(Line::from(Span::styled(expand_label, Style::default().fg(expand_color))).alignment(Alignment::Right))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    // Pulse phase: ~1s cycle at 60fps (30 frames on, 30 frames off).
    let pulse_on = (app.ui_tick / 30) % 2 == 0;

    // Check if this worktree is on a __grab branch (should be greyed out).
    let is_grab_branch = |wt: &crate::git_engine::WorktreeInfo| -> bool {
        wt.branch.ends_with("__grab")
    };

    let items: Vec<ListItem> = app
        .worktrees
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let is_waiting = app.cc_waiting_worktrees.contains(&wt.path);
            let is_grabbed = is_grab_branch(wt);

            let marker = if wt.is_main {
                "\u{25cf}" // ●
            } else if is_grabbed {
                "\u{1f512}" // 🔒
            } else if i == app.selected_worktree {
                "\u{25c9}" // ◉
            } else {
                "\u{25cb}" // ○
            };

            let marker_style = if is_grabbed {
                Style::default().fg(Color::DarkGray)
            } else if is_waiting {
                Style::default()
                    .fg(if pulse_on { Color::Rgb(255, 165, 0) } else { Color::Rgb(200, 120, 0) })
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

            let branch_style = if is_grabbed {
                Style::default().fg(Color::DarkGray)
            } else if is_waiting {
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

            // Grabbed indicator on __grab worktree.
            if is_grabbed {
                spans.push(Span::styled(
                    " (grabbed)",
                    Style::default().fg(Color::DarkGray),
                ));
            }

            // Tag main worktree when holding a grabbed branch.
            if wt.is_main && app.grabbed_branch.is_some() {
                spans.push(Span::styled(
                    " \u{2190}grabbed",
                    Style::default().fg(Color::Rgb(255, 165, 0)).add_modifier(Modifier::BOLD),
                ));
            }

            // Prominent waiting indicator with pulse animation.
            if is_waiting && !is_grabbed {
                let indicator = if pulse_on { " \u{25c6}" } else { " \u{25c7}" }; // ◆ / ◇
                spans.push(Span::styled(
                    indicator,
                    Style::default()
                        .fg(if pulse_on { Color::Rgb(255, 165, 0) } else { Color::Rgb(200, 120, 0) })
                        .add_modifier(Modifier::BOLD),
                ));
            }

            spans.push(Span::styled(
                format!(" {status_text}"),
                if is_grabbed {
                    Style::default().fg(Color::DarkGray)
                } else if wt.is_clean {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Magenta)
                },
            ));

            // Remote sync indicator (ahead/behind upstream).
            if !is_grabbed {
                match (wt.ahead, wt.behind) {
                    (Some(0), Some(0)) => {
                        // Synced with remote
                        spans.push(Span::styled(" ≡", Style::default().fg(Color::DarkGray)));
                    }
                    (Some(ahead), Some(behind)) => {
                        let mut parts = Vec::new();
                        if ahead > 0 {
                            parts.push(format!("↑{ahead}"));
                        }
                        if behind > 0 {
                            parts.push(format!("↓{behind}"));
                        }
                        spans.push(Span::styled(
                            format!(" {}", parts.join("")),
                            Style::default().fg(Color::Cyan),
                        ));
                    }
                    _ => {
                        // No upstream tracking
                    }
                }
            }

            let item = ListItem::new(Line::from(spans));

            // Apply background highlight to the entire row when waiting.
            if is_waiting && !is_grabbed {
                let bg = if pulse_on {
                    Color::Rgb(60, 35, 0)   // warm orange tint
                } else {
                    Color::Rgb(40, 22, 0)   // darker orange pulse
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
