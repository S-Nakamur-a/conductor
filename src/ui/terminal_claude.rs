//! Terminal Claude panel — top-right area showing Claude Code PTY sessions.
//!
//! Displays session tabs and the PTY output of the active Claude Code session.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::Frame;

use crate::app::{App, Focus};

/// Render the Claude Code terminal panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::TerminalClaude;
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };

    let sessions = app.current_worktree_claude_sessions();

    if sessions.is_empty() {
        let block = Block::default()
            .title(" Claude Code ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let msg = Paragraph::new(" Enter / Click / Ctrl+n: new session")
            .style(Style::default().fg(Color::DarkGray))
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
            if Some(*global_idx) == app.active_claude_session {
                selected_tab = tab_idx;
            }
            let is_waiting = app.pty_manager.is_waiting_for_input(*global_idx);
            let label = format!("[CC:{}]", tab_idx + 1);
            let pulse_on = (app.ui_tick / 30) % 2 == 0;
            let label_style = if is_waiting {
                Style::default()
                    .fg(if pulse_on { Color::Rgb(255, 165, 0) } else { Color::Rgb(200, 120, 0) })
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let is_active = Some(*global_idx) == app.active_claude_session;
            let close_style = if is_active {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(" [x]", close_style),
            ])
        })
        .collect();

    // Add [+] and [<=>] tabs.
    let mut titles = tab_titles;
    titles.push(Line::from(Span::styled("[+]", Style::default().fg(Color::Green))));
    let (expand_label, expand_color) = if app.terminal_expanded {
        ("[>=<]", Color::Yellow)
    } else {
        ("[<=>]", Color::DarkGray)
    };
    titles.push(Line::from(Span::styled(expand_label, Style::default().fg(expand_color))));

    let tabs = Tabs::new(titles)
        .select(selected_tab)
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
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

    if let Some(active_idx) = app.active_claude_session {
        if let Some(screen_arc) = app.pty_manager.get_screen(active_idx) {
            let inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);
            crate::ui::common::render_pty_output(frame, inner, &screen_arc, app.terminal_scroll_claude);
        } else {
            frame.render_widget(output_block, output_area);
        }
    } else {
        frame.render_widget(output_block, output_area);
    }
}
