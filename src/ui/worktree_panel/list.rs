//! Rendering of the worktree panel's zone 1: the worktree + inline-session
//! list, with selection, waiting/active indicators, and status markers.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use unicode_width::UnicodeWidthChar;

use crate::app::{App, WorktreeListRow};
use crate::theme::Theme;

/// Truncate a string to fit within `max_width` display columns.
/// Appends "..." if truncation occurs.
pub(super) fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut end = s.len();
    for (i, ch) in s.char_indices() {
        let cw = ch.width().unwrap_or(0);
        if width + cw > max_width {
            end = i;
            break;
        }
        width += cw;
    }
    if end < s.len() {
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

/// Render the worktree + inline-session list (zone 1).
pub(super) fn render_worktree_list(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    focused: bool,
    border_color: Color,
) {
    let theme = &app.theme;

    let is_expanded = app.expanded_panel == Some(crate::app::Focus::Worktree);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    let title = if app.worktree_mgr.grabbed_branch.is_some() {
        " Worktrees [GRABBED] "
    } else {
        " Worktrees "
    };
    let title_style = if app.worktree_mgr.grabbed_branch.is_some() {
        Style::default()
            .fg(theme.waiting_primary)
            .add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(
            Line::from(Span::styled(
                expand_label,
                Style::default().fg(expand_color),
            ))
            .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    // Pulse phase: ~1s cycle at 60fps (30 frames on, 30 frames off).
    let pulse_on = (app.ui_tick / 30).is_multiple_of(2);

    // Determine the worktree path shown in the focused CC panel (if any)
    // so we can suppress blink for that worktree.
    let focused_cc_wt: Option<std::path::PathBuf> = if app.focus == crate::app::Focus::TerminalClaude {
        Some(app.selected_worktree_path())
    } else {
        None
    };

    // Check if this worktree is on a __grab branch (should be greyed out).
    let is_grab_branch =
        |wt: &crate::git_engine::WorktreeInfo| -> bool { wt.branch.ends_with("__grab") };

    // Braille spinner frame for async operations.
    let spinner_frame = super::super::common::spinner_frame(app.ui_tick);

    // Pre-compute session data for inline display.
    let session_groups = app.all_cc_sessions_by_worktree();
    // Map pty_idx → whether its parent worktree is in cc_waiting/cc_active.
    let session_waiting: std::collections::HashMap<usize, bool> = session_groups
        .iter()
        .flat_map(|(wt_idx, _, sessions)| {
            let wt_path = &app.worktrees[*wt_idx].path;
            let waiting = app.terminal.cc_waiting_worktrees.contains(wt_path);
            sessions.iter().map(move |(pty_idx, _)| (*pty_idx, waiting))
        })
        .collect();
    let session_active: std::collections::HashMap<usize, bool> = session_groups
        .iter()
        .flat_map(|(wt_idx, _, sessions)| {
            let wt_path = &app.worktrees[*wt_idx].path;
            let active = app.terminal.cc_active_worktrees.contains(wt_path);
            sessions.iter().map(move |(pty_idx, _)| (*pty_idx, active))
        })
        .collect();

    let mut items: Vec<ListItem> = app
        .worktree_list_rows
        .iter()
        .enumerate()
        .map(|(row_idx, row)| {
            match *row {
                WorktreeListRow::Session { pty_idx, .. } => {
                    // Inline session row — indented under parent worktree.
                    let is_waiting = session_waiting.get(&pty_idx).copied().unwrap_or(false);
                    let label = session_groups
                        .iter()
                        .flat_map(|(_, _, sessions)| sessions.iter())
                        .find(|(idx, _)| *idx == pty_idx)
                        .map(|(_, l)| l.as_str())
                        .unwrap_or("CC");
                    let display_label = if label.is_empty() {
                        format!("CC:{}", pty_idx + 1)
                    } else {
                        label.to_string()
                    };
                    let is_selected = row_idx == app.worktree_list_selected;
                    let label_style = if is_selected {
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let is_active = session_active.get(&pty_idx).copied().unwrap_or(false);
                    let spans = if is_waiting {
                        vec![
                            Span::styled(
                                "   \u{23f3} ",
                                Style::default().fg(theme.waiting_primary),
                            ), // ⏳
                            Span::styled(display_label, label_style),
                        ]
                    } else if is_active {
                        vec![
                            Span::styled(
                                format!("   {spinner_frame} "),
                                Style::default().fg(theme.accent),
                            ),
                            Span::styled(display_label, label_style),
                        ]
                    } else {
                        vec![
                            Span::raw("   \u{25b8} "), // ▸
                            Span::styled(display_label, label_style),
                        ]
                    };
                    ListItem::new(Line::from(spans))
                }
                WorktreeListRow::Worktree(i) => {
                    let wt = &app.worktrees[i];
                    let is_waiting = app.terminal.cc_waiting_worktrees.contains(&wt.path);
                    let is_active = app.terminal.cc_active_worktrees.contains(&wt.path);
                    let is_grabbed = is_grab_branch(wt);
                    let is_pending_delete = app.is_worktree_pending_delete(&wt.path);
                    let suppress_blink =
                        is_waiting && focused_cc_wt.as_deref() == Some(wt.path.as_path());

                    // Override marker and styles for pending-delete worktrees.
                    if is_pending_delete {
                        let spans = vec![
                            Span::styled(
                                format!(" {spinner_frame}\u{1f5d1} "), // 🗑
                                Style::default().fg(theme.error),
                            ),
                            Span::styled(
                                wt.branch.clone(),
                                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                            ),
                        ];
                        return ListItem::new(Line::from(spans));
                    }

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
                    } else if is_waiting && !suppress_blink {
                        Style::default()
                            .fg(if pulse_on {
                                theme.waiting_primary
                            } else {
                                theme.waiting_secondary
                            })
                            .add_modifier(Modifier::BOLD)
                    } else if is_waiting {
                        Style::default().fg(theme.waiting_primary)
                    } else if i == app.selected_worktree {
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };

                    let status_spans: Vec<Span> = if wt.is_clean {
                        vec![Span::styled(" \u{2713}", Style::default().fg(theme.muted))]
                    } else {
                        let mut parts = Vec::new();
                        if wt.added > 0 {
                            parts.push(Span::styled(
                                format!(" +{}", wt.added),
                                if is_grabbed {
                                    Style::default().fg(theme.muted)
                                } else {
                                    Style::default().fg(theme.success)
                                },
                            ));
                        }
                        if wt.modified > 0 {
                            parts.push(Span::styled(
                                format!(" ~{}", wt.modified),
                                if is_grabbed {
                                    Style::default().fg(theme.muted)
                                } else {
                                    Style::default().fg(theme.warning)
                                },
                            ));
                        }
                        if wt.deleted > 0 {
                            parts.push(Span::styled(
                                format!(" -{}", wt.deleted),
                                if is_grabbed {
                                    Style::default().fg(theme.muted)
                                } else {
                                    Style::default().fg(theme.error)
                                },
                            ));
                        }
                        parts
                    };

                    let branch_style = if is_grabbed {
                        Style::default().fg(theme.muted)
                    } else if is_waiting {
                        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
                    } else if i == app.selected_worktree {
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.success)
                    };

                    let is_new = app.new_worktree_paths.contains(&wt.path);

                    let mut spans = vec![
                        Span::styled(format!(" {marker} "), marker_style),
                        Span::styled(wt.branch.clone(), branch_style),
                    ];

                    if is_new {
                        spans.push(Span::styled(
                            " \u{1F331}", // 🌱
                            Style::default()
                                .fg(theme.success)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }

                    if is_grabbed {
                        spans.push(Span::styled(" (grabbed)", Style::default().fg(theme.muted)));
                    }

                    if wt.is_main && app.worktree_mgr.grabbed_branch.is_some() {
                        spans.push(Span::styled(
                            " \u{1f4e5}grabbed", // 📥grabbed
                            Style::default()
                                .fg(theme.waiting_primary)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }

                    if is_waiting && !is_grabbed {
                        let effective_pulse = !suppress_blink && pulse_on;
                        let indicator = if effective_pulse {
                            " \u{25c6}"
                        } else {
                            " \u{25c7}"
                        };
                        let indicator = if suppress_blink {
                            " \u{25c6}"
                        } else {
                            indicator
                        };
                        let indicator_fg = if suppress_blink || effective_pulse {
                            theme.waiting_primary
                        } else {
                            theme.waiting_secondary
                        };
                        spans.push(Span::styled(
                            indicator,
                            Style::default()
                                .fg(indicator_fg)
                                .add_modifier(Modifier::BOLD),
                        ));
                    } else if is_active && !is_grabbed {
                        spans.push(Span::styled(
                            format!(" {spinner_frame}"),
                            Style::default()
                                .fg(theme.accent)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }

                    spans.extend(status_spans);

                    if !is_grabbed {
                        match (wt.ahead, wt.behind) {
                            (Some(0), Some(0)) => {
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
                            _ => {}
                        }
                    }

                    let item = ListItem::new(Line::from(spans));

                    if is_waiting && !is_grabbed {
                        let bg = if suppress_blink {
                            Theme::darken(theme.waiting_primary, 0.20)
                        } else if pulse_on {
                            Theme::darken(theme.waiting_primary, 0.24)
                        } else {
                            Theme::darken(theme.waiting_primary, 0.16)
                        };
                        item.style(Style::default().bg(bg))
                    } else {
                        item
                    }
                }
            }
        })
        .collect();

    // Append pending-create worktrees at the end of the list.
    for pending in &app.worktree_mgr.pending_worktrees {
        if pending.op == crate::app::PendingWorktreeOp::Creating
            || pending.op == crate::app::PendingWorktreeOp::SmartCreating
        {
            let is_smart = pending.op == crate::app::PendingWorktreeOp::SmartCreating;
            let icon = if is_smart { "\u{1F9E0}" } else { "\u{2728}" }; // 🧠 vs ✨
            let display_name = if pending.branch.is_empty() {
                let max_width = 30;
                truncate_to_width(&pending.description, max_width)
            } else {
                pending.branch.clone()
            };
            let spans = vec![
                Span::styled(
                    format!(" {spinner_frame}{icon} "),
                    Style::default().fg(theme.success),
                ),
                Span::styled(
                    display_name,
                    Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                ),
            ];
            items.push(ListItem::new(Line::from(spans)));
        }
    }

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.worktree_list_selected));

    frame.render_stateful_widget(list, area, &mut state);
}
