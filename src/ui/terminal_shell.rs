//! Terminal Shell panel — bottom-right area showing shell PTY sessions.
//!
//! Displays session tabs and the PTY output of the active shell session.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, Focus};

/// Render the Shell terminal panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let focused = app.focus == Focus::TerminalShell;
    let border_color = app.animated_border_color(Focus::TerminalShell);

    let is_grabbed = app.is_selected_worktree_grabbed();

    let sessions = app.current_worktree_shell_sessions();

    let is_expanded = matches!(
        app.expanded_panel,
        Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
    );

    // If the selected worktree is grabbed, show a locked overlay instead of sessions.
    if is_grabbed {
        let block = if is_expanded {
            Block::default().title(" Shell \u{1f512} ")
        } else {
            Block::default()
                .title(Span::styled(
                    " Shell \u{1f512} ",
                    Style::default().fg(theme.muted),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.muted))
        };
        let msg = Paragraph::new(Span::styled(
            "  Locked — worktree is grabbed",
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ))
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
            Block::default().title(Span::styled(" Shell ", title_style))
        } else {
            Block::default()
                .title(Span::styled(" Shell ", title_style))
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(Style::default().fg(border_color))
        };
        let msg = Paragraph::new(" Enter / Click / Ctrl+t: new session")
            .style(Style::default().fg(theme.muted))
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    // Layout: session tabs (1 row) + PTY output (fill).
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);

    // Session tabs — scrolling strip with pinned [+]/expand (see `tab_bar`).
    let tab_items: Vec<crate::ui::tab_bar::TabItem> = sessions
        .iter()
        .map(|(global_idx, session)| crate::ui::tab_bar::TabItem {
            global_idx: *global_idx,
            label: format!("[{}]", session.label),
            is_active: Some(*global_idx) == app.terminal.active_shell_session,
            label_style: Style::default(),
        })
        .collect();
    let (hits, scroll) = crate::ui::tab_bar::render(
        frame,
        chunks[0],
        theme,
        &tab_items,
        app.terminal.shell_tab_scroll,
        app.terminal.shell_tab_reveal,
        is_expanded,
        app.terminal.shell_tab_hover,
    );
    app.terminal.shell_tab_hits = hits;
    app.terminal.shell_tab_scroll = scroll;
    app.terminal.shell_tab_reveal = false;

    // PTY output.
    let output_area = chunks[1];
    let output_block = if is_expanded {
        Block::default()
    } else {
        Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color))
    };

    if let Some(active_idx) = app.terminal.active_shell_session {
        if let Some(screen_arc) = app.terminal.pty_manager.get_screen(active_idx) {
            let inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);

            // Rebuild PTY snapshot only when new output arrives (dirty flag)
            // or cache is empty. Uses try_lock to avoid blocking when the
            // PTY reader thread holds the vt100 mutex.
            let scroll_changed =
                app.terminal.cache_shell.effective_offset != app.terminal.scroll_shell;
            if (app.terminal.cache_shell.lines.is_empty()
                || (focused && app.terminal.dirty_shell)
                || scroll_changed)
                && let Some(cache) = crate::ui::common::build_pty_lines(
                    &screen_arc,
                    app.terminal.scroll_shell,
                    inner.height,
                    inner.width,
                )
            {
                // Sync scroll offset with the actual clamped position from vt100
                // to prevent infinite rebuilds when scroll exceeds scrollback buffer.
                app.terminal.scroll_shell = cache.effective_offset;
                app.terminal.cache_shell = cache;
                app.terminal.dirty_shell = false;
            }
            crate::ui::common::render_pty_cached(
                frame,
                inner,
                &app.terminal.cache_shell,
                &app.theme,
            );

            // Set cursor position for IME when focused, not scrolled back,
            // and no overlay is covering this panel.
            if focused
                && !app.is_any_overlay_active()
                && let Some((row, col)) = app.terminal.cache_shell.cursor_position
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
