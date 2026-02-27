//! Terminal Shell panel — bottom-right area showing shell PTY sessions.
//!
//! Displays session tabs and the PTY output of the active shell session.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::Frame;

use crate::app::{App, Focus};

/// Render the Shell terminal panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let focused = app.focus == Focus::TerminalShell;
    let border_color = if focused { theme.border_focused } else { theme.border_unfocused };

    let sessions = app.current_worktree_shell_sessions();

    if sessions.is_empty() {
        let block = Block::default()
            .title(" Shell ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let msg = Paragraph::new(" Enter / Click / Ctrl+t: new session")
            .style(Style::default().fg(theme.muted))
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    // Layout: session tabs (1 row) + PTY output (fill).
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .split(area);

    // Session tabs.
    let mut selected_tab: usize = 0;
    let tab_titles: Vec<Line> = sessions
        .iter()
        .enumerate()
        .map(|(tab_idx, (global_idx, _session))| {
            if Some(*global_idx) == app.active_shell_session {
                selected_tab = tab_idx;
            }
            let label = format!("[SH:{}]", tab_idx + 1);
            let is_active = Some(*global_idx) == app.active_shell_session;
            let close_style = if is_active {
                Style::default().fg(theme.error)
            } else {
                Style::default().fg(theme.muted)
            };
            Line::from(vec![
                Span::raw(label),
                Span::styled(" [x]", close_style),
            ])
        })
        .collect();

    // Add [+] and [<=>] tabs.
    let mut titles = tab_titles;
    titles.push(Line::from(Span::styled("[+]", Style::default().fg(theme.success))));
    let is_expanded = matches!(app.expanded_panel, Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell));
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };
    titles.push(Line::from(Span::styled(expand_label, Style::default().fg(expand_color))));

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
    let output_block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(border_color));

    if let Some(active_idx) = app.active_shell_session {
        if let Some(screen_arc) = app.pty_manager.get_screen(active_idx) {
            let inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);

            // When focused (or cache empty), do the expensive vt100 snapshot.
            // Otherwise, reuse cached lines for fast rendering.
            if focused || app.pty_cache_shell.lines.is_empty() {
                app.pty_cache_shell = crate::ui::common::build_pty_lines(
                    &screen_arc,
                    app.terminal_scroll_shell,
                    inner.height,
                    inner.width,
                );
            }
            crate::ui::common::render_pty_cached(frame, inner, &app.pty_cache_shell);
        } else {
            frame.render_widget(output_block, output_area);
        }
    } else {
        frame.render_widget(output_block, output_area);
    }
}
