//! Terminal Claude panel — top-right area showing Claude Code PTY sessions.
//!
//! Displays session tabs and the PTY output of the active Claude Code session.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Tabs};

use crate::app::{App, Focus};

/// Render the Claude Code terminal panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let focused = app.focus == Focus::TerminalClaude;
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border_unfocused
    };

    let is_grabbed = app.is_selected_worktree_grabbed();

    let sessions = app.current_worktree_claude_sessions();

    let is_expanded = matches!(
        app.expanded_panel,
        Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
    );

    // If the selected worktree is grabbed, show a locked overlay instead of sessions.
    if is_grabbed {
        let block = if is_expanded {
            Block::default().title(" Claude Code \u{1f512} ")
        } else {
            Block::default()
                .title(Span::styled(
                    " Claude Code \u{1f512} ",
                    Style::default().fg(theme.muted),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.muted))
        };
        let msg = Paragraph::new(vec![
            ratatui::text::Line::from(""),
            ratatui::text::Line::from(Span::styled(
                "  This worktree is grabbed.",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            )),
            ratatui::text::Line::from(Span::styled(
                "  Sessions are running on main.",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            )),
        ])
        .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    if sessions.is_empty() {
        let title_style = if focused {
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        let block = if is_expanded {
            Block::default().title(Span::styled(" Claude Code ", title_style))
        } else {
            Block::default()
                .title(Span::styled(" Claude Code ", title_style))
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(Style::default().fg(border_color))
        };
        let msg = Paragraph::new(" Enter / Click / Ctrl+n: new session")
            .style(Style::default().fg(theme.muted))
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    // Layout: session tabs (1 row) + PTY output (fill).
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);

    // Session tabs.
    let mut selected_tab: usize = 0;
    let tab_titles: Vec<Line> = sessions
        .iter()
        .enumerate()
        .map(|(tab_idx, (global_idx, session))| {
            if Some(*global_idx) == app.terminal.active_claude_session {
                selected_tab = tab_idx;
            }
            let is_waiting = app.terminal.pty_manager.is_waiting_for_input(*global_idx);
            let label = format!("[{}]", session.label);
            let is_active = Some(*global_idx) == app.terminal.active_claude_session;
            let suppress_blink = focused;
            let pulse_on = (app.ui_tick / 30).is_multiple_of(2);
            let label_style = if is_waiting {
                if suppress_blink {
                    // Static style when this panel is focused on this session.
                    Style::default().fg(theme.waiting_primary)
                } else {
                    Style::default()
                        .fg(if pulse_on {
                            theme.waiting_primary
                        } else {
                            theme.waiting_secondary
                        })
                        .add_modifier(Modifier::BOLD)
                }
            } else {
                Style::default()
            };
            let close_style = if is_active {
                Style::default().fg(theme.error)
            } else {
                Style::default().fg(theme.muted)
            };
            Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(" [x]", close_style),
            ])
        })
        .collect();

    // Add [+] and [<=>] tabs.
    let mut titles = tab_titles;
    titles.push(Line::from(Span::styled(
        "[+]",
        Style::default().fg(theme.success),
    )));
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };
    titles.push(Line::from(Span::styled(
        expand_label,
        Style::default().fg(expand_color),
    )));

    let tabs = Tabs::new(titles)
        .select(selected_tab)
        .highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" "))
        .padding("", "");
    frame.render_widget(tabs, chunks[0]);

    // PTY output.
    let output_area = chunks[1];
    let output_block = if is_expanded {
        Block::default()
    } else {
        // Border color while reflow is active: the complement of the accent is
        // the persistent read-mode cue. During the entry transition the border
        // glides smoothly from the accent to that complement (a single gentle
        // gradient, no flicker); leaving the view is instant, so there is no
        // exit gradient; otherwise it's the normal focus/unfocus color.
        // Focus-gated to match the reflow render guard below: while the panel is
        // unfocused it shows the live PTY (reflow is preserved but not rendered),
        // so the read-mode complement would be a misleading border cue there.
        let effective_border = if app.reflow.active && app.focus == Focus::TerminalClaude {
            let complement = crate::theme::Theme::complement(theme.accent);
            if let Some(sweep) = &app.reflow.sweep {
                let p = crate::event::reflow::sweep_progress(
                    &sweep.start,
                    crate::event::reflow::TRANSITION_DURATION_MS,
                );
                let t = crate::event::reflow::transition_eased(p);
                // Entering read mode: accent → complement.
                crate::theme::Theme::lerp(theme.accent, complement, t)
            } else {
                // Steady read mode: rest on the complement.
                complement
            }
        } else {
            border_color
        };
        Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(border_type)
            .border_style(Style::default().fg(effective_border))
    };

    // If the reflow transcript view is active *and* this panel has focus,
    // hand off rendering to the reflow view.  The focus guard prevents a
    // stale reflow from rendering after a worktree switch or focus move that
    // didn't go through close_reflow (belt-and-suspenders; F4 closes it in
    // set_focus/on_worktree_changed, but the guard keeps rendering safe).
    if app.reflow.active && app.focus == Focus::TerminalClaude {
        let inner = output_block.inner(output_area);
        frame.render_widget(output_block, output_area);
        crate::ui::reflow_view::render(frame, inner, app);
        return;
    }

    if let Some(active_idx) = app.terminal.active_claude_session {
        if let Some(screen_arc) = app.terminal.pty_manager.get_screen(active_idx) {
            let inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);

            // Rebuild PTY snapshot only when new output arrives (dirty flag)
            // or cache is empty. Uses try_lock to avoid blocking when the
            // PTY reader thread holds the vt100 mutex — keeps UI responsive.
            let scroll_changed =
                app.terminal.cache_claude.effective_offset != app.terminal.scroll_claude;
            if (app.terminal.cache_claude.lines.is_empty()
                || (focused && app.terminal.dirty_claude)
                || scroll_changed)
                && let Some(cache) = crate::ui::common::build_pty_lines(
                    &screen_arc,
                    app.terminal.scroll_claude,
                    inner.height,
                    inner.width,
                )
            {
                // Sync scroll offset with the actual clamped position from vt100
                // to prevent infinite rebuilds when scroll exceeds scrollback buffer.
                app.terminal.scroll_claude = cache.effective_offset;
                app.terminal.cache_claude = cache;
                app.terminal.dirty_claude = false;
            }
            // If try_lock failed (reader thread busy), keep using old cache.
            crate::ui::common::render_pty_cached(
                frame,
                inner,
                &app.terminal.cache_claude,
                &app.theme,
            );

            // Set cursor position for IME when focused, not scrolled back,
            // and no overlay is covering this panel.
            if focused
                && !app.is_any_overlay_active()
                && let Some((row, col)) = app.terminal.cache_claude.cursor_position
            {
                let cursor_x = inner.x + col;
                let cursor_y = inner.y + row;
                if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
                    frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
                }
            }
        } else {
            frame.render_widget(output_block, output_area);
        }
    } else {
        frame.render_widget(output_block, output_area);
    }
}
