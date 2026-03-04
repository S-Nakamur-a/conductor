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
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Worktree;
    let border_color = if focused { app.theme.border_focused } else { app.theme.border_unfocused };

    // Begin a scope so the `theme` borrow ends before the mutable zone-3 call.
    let theme = &app.theme;

    let is_expanded = app.expanded_panel == Some(Focus::Worktree);
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
        Style::default().fg(theme.waiting_primary).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    // ── Zone layout calculation ────────────────────────────────────
    // Zone 1: worktree list   — 30%
    // Zone 2: detail section  — 50%
    // Zone 3: session panel / decoration — 20%
    let decoration_mode = DecorationMode::from_str(&app.config.general.decoration);
    let has_sessions = !app.all_cc_sessions_by_worktree().is_empty();

    let zones = if area.height < 10 {
        // Too small: only show the list.
        Layout::vertical([
            Constraint::Percentage(100),
            Constraint::Length(0),
            Constraint::Length(0),
        ])
        .split(area)
    } else if has_sessions {
        // Sessions exist: always show session panel in zone 3.
        Layout::vertical([
            Constraint::Percentage(30),
            Constraint::Percentage(50),
            Constraint::Percentage(20),
        ])
        .split(area)
    } else if decoration_mode == DecorationMode::None {
        // No sessions, no decoration: split between list and detail only.
        Layout::vertical([
            Constraint::Percentage(30),
            Constraint::Percentage(70),
            Constraint::Length(0),
        ])
        .split(area)
    } else {
        Layout::vertical([
            Constraint::Percentage(30),
            Constraint::Percentage(50),
            Constraint::Percentage(20),
        ])
        .split(area)
    };

    // ── Zone 1: Worktree list ─────────────────────────────────────

    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(Line::from(Span::styled(expand_label, Style::default().fg(expand_color))).alignment(Alignment::Right))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    // Pulse phase: ~1s cycle at 60fps (30 frames on, 30 frames off).
    let pulse_on = (app.ui_tick / 30) % 2 == 0;

    // Determine the worktree path shown in the focused CC panel (if any)
    // so we can suppress blink for that worktree.
    let focused_cc_wt: Option<std::path::PathBuf> = if app.focus == Focus::TerminalClaude {
        Some(app.selected_worktree_path())
    } else {
        None
    };

    // Check if this worktree is on a __grab branch (should be greyed out).
    let is_grab_branch = |wt: &crate::git_engine::WorktreeInfo| -> bool {
        wt.branch.ends_with("__grab")
    };

    // Braille spinner frames for async operations.
    const BRAILLE_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let spinner_frame = BRAILLE_SPINNER[(app.ui_tick as usize / 4) % BRAILLE_SPINNER.len()];

    let mut items: Vec<ListItem> = app
        .worktrees
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let is_waiting = app.terminal.cc_waiting_worktrees.contains(&wt.path);
            let is_grabbed = is_grab_branch(wt);
            let is_pending_delete = app.is_worktree_pending_delete(&wt.path);
            let suppress_blink = is_waiting && focused_cc_wt.as_deref() == Some(wt.path.as_path());

            // Override marker and styles for pending-delete worktrees.
            if is_pending_delete {
                let spans = vec![
                    Span::styled(
                        format!(" {spinner_frame}\u{1f5d1} "),  // 🗑
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
                    .fg(if pulse_on { theme.waiting_primary } else { theme.waiting_secondary })
                    .add_modifier(Modifier::BOLD)
            } else if is_waiting {
                // Static style for focused CC session.
                Style::default().fg(theme.waiting_primary)
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
            if wt.is_main && app.worktree_mgr.grabbed_branch.is_some() {
                spans.push(Span::styled(
                    " \u{2190}grabbed",
                    Style::default().fg(theme.waiting_primary).add_modifier(Modifier::BOLD),
                ));
            }

            // Prominent waiting indicator with pulse animation.
            if is_waiting && !is_grabbed {
                let effective_pulse = !suppress_blink && pulse_on;
                let indicator = if effective_pulse { " \u{25c6}" } else { " \u{25c7}" }; // ◆ / ◇
                // When suppressed, show static ◇ with primary color (no blink).
                let indicator = if suppress_blink { " \u{25c6}" } else { indicator }; // ◆ (static)
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
            }

            spans.push(Span::styled(
                format!(" {status_text}"),
                if is_grabbed || wt.is_clean {
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
                let bg = if suppress_blink {
                    // Static background when focused on this session.
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
        })
        .collect();

    // Append pending-create worktrees at the end of the list.
    for pending in &app.worktree_mgr.pending_worktrees {
        if pending.op == crate::app::PendingWorktreeOp::Creating {
            let spans = vec![
                Span::styled(
                    format!(" {spinner_frame}\u{2728} "),  // ✨
                    Style::default().fg(theme.success),
                ),
                Span::styled(
                    pending.branch.clone(),
                    Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                ),
            ];
            items.push(ListItem::new(Line::from(spans)));
        }
    }

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

    if zones[1].height >= 3 {
        render_detail(frame, zones[1], app, theme, border_color);
    }

    // Drop the immutable `theme` borrow so zone 3 can take `&mut app`.
    #[allow(dropping_references)]
    drop(theme);

    // ── Zone 3: Session panel / Decoration ─────────────────────────

    if zones[2].height >= 4 {
        if has_sessions {
            render_session_status(frame, zones[2], app, border_color);
        } else {
            app.session_panel_area = None;
            decoration::render_decoration(
                frame,
                zones[2],
                &app.decoration_states,
                &app.theme,
                decoration_mode,
            );
        }
    } else {
        app.session_panel_area = None;
    }
}

/// Render the detail section: selected worktree info.
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

    let Some(wt) = app.worktrees.get(app.selected_worktree) else {
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // Branch name.
    lines.push(Line::from(vec![
        Span::styled(" Branch: ", Style::default().fg(theme.muted)),
        Span::styled(
            wt.branch.as_str(),
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Path (show last component for brevity).
    let path_display = wt
        .path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| wt.path.display().to_string());
    lines.push(Line::from(vec![
        Span::styled(" Path:   ", Style::default().fg(theme.muted)),
        Span::styled(path_display, Style::default().fg(theme.fg)),
    ]));

    // Status.
    let status_spans = if wt.is_clean {
        vec![
            Span::styled(" Status: ", Style::default().fg(theme.muted)),
            Span::styled("\u{2713} clean", Style::default().fg(theme.success)),
        ]
    } else {
        vec![
            Span::styled(" Status: ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("+{} ~{} -{}", wt.added, wt.modified, wt.deleted),
                Style::default().fg(Color::Magenta),
            ),
        ]
    };
    lines.push(Line::from(status_spans));

    // Remote sync.
    let remote_spans = match (wt.ahead, wt.behind) {
        (Some(0), Some(0)) => vec![
            Span::styled(" Remote: ", Style::default().fg(theme.muted)),
            Span::styled("\u{2261} synced", Style::default().fg(theme.success)),
        ],
        (Some(ahead), Some(behind)) => {
            let mut parts = Vec::new();
            if ahead > 0 {
                parts.push(format!("\u{2191}{ahead}"));
            }
            if behind > 0 {
                parts.push(format!("\u{2193}{behind}"));
            }
            vec![
                Span::styled(" Remote: ", Style::default().fg(theme.muted)),
                Span::styled(parts.join(" "), Style::default().fg(theme.info)),
            ]
        }
        _ => vec![
            Span::styled(" Remote: ", Style::default().fg(theme.muted)),
            Span::styled("no upstream", Style::default().fg(theme.muted)),
        ],
    };
    lines.push(Line::from(remote_spans));

    // ── Branch lineage & PR info ──────────────────────────────────
    let details = &app.branch_details;
    let is_main = wt.is_main;

    let has_lineage = details.initial_branch.is_some()
        || !details.derived_branches.is_empty()
        || (app.gh_available && !is_main);

    if has_lineage {
        lines.push(Line::from(""));

        // Parent branch.
        if let Some(ref base) = details.initial_branch {
            lines.push(Line::from(vec![
                Span::styled(" Parent: ", Style::default().fg(theme.muted)),
                Span::styled(base.as_str(), Style::default().fg(theme.fg)),
            ]));
        }

        // Derived (forked) branches — one per line for readability.
        if !details.derived_branches.is_empty() {
            // First fork on the label line.
            lines.push(Line::from(vec![
                Span::styled(" Forks:  ", Style::default().fg(theme.muted)),
                Span::styled(
                    details.derived_branches[0].as_str(),
                    Style::default().fg(theme.info),
                ),
            ]));
            // Additional forks indented on subsequent lines.
            for fork in &details.derived_branches[1..] {
                lines.push(Line::from(vec![
                    Span::styled("         ", Style::default().fg(theme.muted)),
                    Span::styled(fork.as_str(), Style::default().fg(theme.info)),
                ]));
            }
        }

        // PR URL.
        if app.gh_available && !is_main {
            if details.pr_loading {
                lines.push(Line::from(vec![
                    Span::styled(" PR:     ", Style::default().fg(theme.muted)),
                    Span::styled("loading...", Style::default().fg(theme.muted)),
                ]));
            } else if let Some(ref url) = details.pr_url {
                lines.push(Line::from(vec![
                    Span::styled(" PR:     ", Style::default().fg(theme.muted)),
                    Span::styled(
                        url.as_str(),
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(" PR:     ", Style::default().fg(theme.muted)),
                    Span::styled("none", Style::default().fg(theme.muted)),
                ]));
            }
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Render the session status panel showing Claude Code sessions across all worktrees.
fn render_session_status(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    border_color: Color,
) {
    // Clone theme colors to avoid borrow conflict with &mut app.
    let theme_muted = app.theme.muted;
    let theme_accent = app.theme.accent;
    let theme_fg = app.theme.fg;
    let theme_success = app.theme.success;
    let theme_waiting = app.theme.waiting_primary;

    // Gather session data before mutating app.
    let groups = app.all_cc_sessions_by_worktree();
    let waiting_flags: Vec<Vec<bool>> = groups
        .iter()
        .map(|(_, _, sessions)| {
            sessions
                .iter()
                .map(|(pty_idx, _)| app.terminal.pty_manager.is_waiting_for_input(*pty_idx))
                .collect()
        })
        .collect();

    let block = Block::default()
        .title(Span::styled(
            " Sessions ",
            Style::default().fg(theme_muted),
        ))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        app.session_panel_area = None;
        return;
    }

    app.session_panel_area = Some(area);

    // Build all content lines with their row metadata.
    let mut all_lines: Vec<(Line<'_>, usize, Option<usize>)> = Vec::new();

    for (group_i, (wt_idx, branch, sessions)) in groups.iter().enumerate() {
        // Worktree header line.
        all_lines.push((
            Line::from(Span::styled(
                format!(" {branch}"),
                Style::default()
                    .fg(theme_accent)
                    .add_modifier(Modifier::BOLD),
            )),
            *wt_idx,
            None,
        ));

        // Session lines.
        for (sess_i, (pty_idx, label)) in sessions.iter().enumerate() {
            let is_waiting = waiting_flags[group_i][sess_i];
            let (icon, icon_color) = if is_waiting {
                ("\u{1f4a4}", theme_waiting) // 💤
            } else {
                ("\u{1f3d7}\u{fe0f}", theme_success)  // 🏗️
            };
            let display_label = if label.is_empty() {
                format!("CC:{}", pty_idx + 1)
            } else {
                label.clone()
            };
            all_lines.push((
                Line::from(vec![
                    Span::styled(
                        format!("  {icon} "),
                        Style::default().fg(icon_color),
                    ),
                    Span::styled(
                        display_label,
                        Style::default().fg(theme_fg),
                    ),
                ]),
                *wt_idx,
                Some(*pty_idx),
            ));
        }
    }

    // Apply scroll.
    let max_scroll = all_lines.len().saturating_sub(inner.height as usize);
    if app.session_panel_scroll > max_scroll {
        app.session_panel_scroll = max_scroll;
    }

    // Populate row mapping and render.
    app.session_panel_rows.clear();
    let visible = all_lines
        .iter()
        .skip(app.session_panel_scroll)
        .take(inner.height as usize);

    let mut lines_to_render: Vec<Line<'_>> = Vec::new();
    for (row_offset, (line, wt_idx, pty_idx)) in visible.enumerate() {
        app.session_panel_rows
            .push((row_offset as u16, *wt_idx, *pty_idx));
        lines_to_render.push(line.clone());
    }

    let paragraph = Paragraph::new(lines_to_render);
    frame.render_widget(paragraph, inner);
}
