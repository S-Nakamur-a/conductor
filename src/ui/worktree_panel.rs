//! Worktree panel — left-most column showing the worktree list.
//!
//! Displays the list of worktrees with selection, status indicators,
//! detail info, and an optional decoration zone (aquarium).

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::theme::Theme;
use crate::ui::decoration::{self, DecorationMode};

/// Render the worktree panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let focused = app.focus == Focus::Worktree;
    let border_color = if focused { theme.border_focused } else { theme.border_unfocused };

    let is_expanded = app.expanded_panel == Some(Focus::Worktree);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    let title = if app.grabbed_branch.is_some() {
        " Worktrees [GRABBED] "
    } else {
        " Worktrees "
    };
    let title_style = if app.grabbed_branch.is_some() {
        Style::default().fg(theme.waiting_primary).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    // ── Zone layout calculation ────────────────────────────────────
    // Zone 1: worktree list (fixed height = items + 2 for borders)
    let list_rows = (app.worktrees.len() as u16 + 2).max(5);
    // Zone 2: detail section (base branch + local branches)
    let detail_content_rows = 1 + app.local_branches.len() as u16; // "Base: ..." + branch lines
    let detail_rows = (detail_content_rows + 2).min(8); // +2 for top border + header line, capped at 8
    // Zone 3: decoration (whatever is left)
    let decoration_mode = DecorationMode::from_str(&app.config.general.decoration);
    let min_decoration_rows: u16 = if decoration_mode == DecorationMode::None { 0 } else { 4 };

    // Cap decoration height at 20% of the total panel area.
    let max_deco_h = area.height / 5;

    // If the area is too small to fit all zones, progressively hide decoration and detail.
    let total_needed = list_rows + detail_rows + min_decoration_rows;
    let (zone1_h, zone2_h, zone3_constraint) = if area.height >= total_needed {
        // All zones fit — cap decoration at 20%.
        let remaining = area.height.saturating_sub(list_rows + detail_rows);
        let deco_h = remaining.min(max_deco_h);
        (list_rows, detail_rows, Constraint::Length(deco_h))
    } else if area.height >= list_rows + detail_rows {
        // Detail fits but decoration might be tiny — show what's left, capped.
        let remaining = area.height.saturating_sub(list_rows + detail_rows);
        let deco_h = remaining.min(max_deco_h);
        (list_rows, detail_rows, Constraint::Length(deco_h))
    } else if area.height >= list_rows + 3 {
        // Squeeze detail, no decoration.
        let remaining = area.height.saturating_sub(list_rows);
        (list_rows, remaining, Constraint::Length(0))
    } else {
        // Only list fits.
        (area.height, 0, Constraint::Length(0))
    };

    let zones = Layout::vertical([
        Constraint::Length(zone1_h),
        Constraint::Min(zone2_h),  // Detail absorbs leftover space from capped decoration
        zone3_constraint,
    ])
    .split(area);

    // ── Zone 1: Worktree list ─────────────────────────────────────

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
                Style::default().fg(theme.muted)
            } else if is_waiting {
                Style::default()
                    .fg(if pulse_on { theme.waiting_primary } else { theme.waiting_secondary })
                    .add_modifier(Modifier::BOLD)
            } else if i == app.selected_worktree {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let status_text = if wt.is_clean {
                "\u{2713}".to_string()
            } else {
                format!("+{} ~{} -{}", wt.added, wt.modified, wt.deleted)
            };

            let branch_style = if is_grabbed {
                Style::default().fg(theme.muted)
            } else if is_waiting {
                Style::default()
                    .fg(theme.fg)
                    .add_modifier(Modifier::BOLD)
            } else if i == app.selected_worktree {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.success)
            };

            let mut spans = vec![
                Span::styled(format!(" {marker} "), marker_style),
                Span::styled(wt.branch.clone(), branch_style),
            ];

            // Grabbed indicator on __grab worktree.
            if is_grabbed {
                spans.push(Span::styled(
                    " (grabbed)",
                    Style::default().fg(theme.muted),
                ));
            }

            // Tag main worktree when holding a grabbed branch.
            if wt.is_main && app.grabbed_branch.is_some() {
                spans.push(Span::styled(
                    " \u{2190}grabbed",
                    Style::default().fg(theme.waiting_primary).add_modifier(Modifier::BOLD),
                ));
            }

            // Prominent waiting indicator with pulse animation.
            if is_waiting && !is_grabbed {
                let indicator = if pulse_on { " \u{25c6}" } else { " \u{25c7}" }; // ◆ / ◇
                spans.push(Span::styled(
                    indicator,
                    Style::default()
                        .fg(if pulse_on { theme.waiting_primary } else { theme.waiting_secondary })
                        .add_modifier(Modifier::BOLD),
                ));
            }

            spans.push(Span::styled(
                format!(" {status_text}"),
                if is_grabbed {
                    Style::default().fg(theme.muted)
                } else if wt.is_clean {
                    Style::default().fg(theme.muted)
                } else {
                    Style::default().fg(Color::Magenta)
                },
            ));

            // Remote sync indicator (ahead/behind upstream).
            if !is_grabbed {
                match (wt.ahead, wt.behind) {
                    (Some(0), Some(0)) => {
                        // Synced with remote
                        spans.push(Span::styled(" ≡", Style::default().fg(theme.muted)));
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
                            Style::default().fg(theme.info),
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
                    Theme::darken(theme.waiting_primary, 0.24)
                } else {
                    Theme::darken(theme.waiting_primary, 0.16)
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
                .bg(theme.selected_bg_inactive)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    state.select(Some(app.selected_worktree));

    frame.render_stateful_widget(list, zones[0], &mut state);

    // ── Zone 2: Detail section ────────────────────────────────────

    if zone2_h >= 3 {
        render_detail(frame, zones[1], app, theme, border_color);
    }

    // ── Zone 3: Decoration ────────────────────────────────────────

    if zones[2].height >= min_decoration_rows {
        decoration::render_decoration(
            frame,
            zones[2],
            &app.aquarium_state,
            theme,
            decoration_mode,
        );
    }
}

/// Render the detail section: base branch and local branch list.
fn render_detail(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    border_color: Color,
) {
    let block = Block::default()
        .title(Span::styled(
            " Detail ",
            Style::default().fg(theme.muted),
        ))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // Base branch.
    lines.push(Line::from(vec![
        Span::styled(" Base: ", Style::default().fg(theme.muted)),
        Span::styled(
            app.config.general.main_branch.as_str(),
            Style::default().fg(theme.info),
        ),
    ]));

    // Branches header.
    if !app.local_branches.is_empty() {
        lines.push(Line::from(Span::styled(
            " Branches:",
            Style::default().fg(theme.muted),
        )));

        let max_branch_lines = inner.height.saturating_sub(2) as usize;
        for branch in app.local_branches.iter().take(max_branch_lines) {
            let style = if Some(branch.as_str()) == app.worktrees.get(app.selected_worktree).map(|w| w.branch.as_str()) {
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            lines.push(Line::from(Span::styled(format!("  {branch}"), style)));
        }
        if app.local_branches.len() > max_branch_lines {
            lines.push(Line::from(Span::styled(
                format!("  +{} more", app.local_branches.len() - max_branch_lines),
                Style::default().fg(theme.muted),
            )));
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
