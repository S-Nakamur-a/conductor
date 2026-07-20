//! The title bar at the top of the screen (repository badge, branch, path,
//! and right-aligned workspace/usage stats).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::color::{format_tokens, name_to_color};

/// Render the title bar at the top showing worktree name and working directory.
pub fn render_title_bar(frame: &mut Frame, area: Rect, app: &mut crate::app::App) {
    let theme = &app.theme;
    let wt_name = app
        .worktrees
        .get(app.selected_worktree)
        .map(|w| w.branch.as_str())
        .unwrap_or("—");
    let wt_path = app
        .worktrees
        .get(app.selected_worktree)
        .map(|w| w.path.display().to_string())
        .unwrap_or_else(|| app.repo_path.display().to_string());

    let (badge_bg, badge_fg, branch_fg) = name_to_color(&app.main_repo_name);

    // Use Color::Reset so the terminal's own background (including any
    // background image) shows through the title bar.
    let bar_bg = Color::Reset;
    let conductor_bg = badge_bg;
    let conductor_fg = badge_fg;

    let badge_text = format!(" {} ", app.main_repo_name);
    let line = Line::from(vec![
        Span::styled(
            &badge_text,
            Style::default()
                .fg(conductor_fg)
                .bg(conductor_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            wt_name,
            Style::default().fg(branch_fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(theme.muted)),
        Span::styled(wt_path, Style::default().fg(theme.dir_fg)),
    ]);
    let paragraph = Paragraph::new(line).style(Style::default().bg(bar_bg));
    frame.render_widget(paragraph, area);

    // ── Right-aligned stats display (today's activity + ccusage) ──────────
    {
        let sep = Span::styled(" | ", Style::default().fg(theme.muted).bg(bar_bg));
        let mut spans: Vec<Span> = Vec::new();

        // Workspace overview: worktree count, running Claude Code sessions,
        // and the current Conductor version.
        let worktree_count = app.worktrees.len();
        let claude_session_count = app
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| s.kind == crate::pty_manager::SessionKind::ClaudeCode)
            .count();
        spans.push(Span::styled(
            format!("{} worktrees", worktree_count),
            Style::default().fg(theme.info).bg(bar_bg),
        ));
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!("{} sessions", claude_session_count),
            Style::default().fg(theme.success).bg(bar_bg),
        ));
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!("v{}", crate::update_checker::current_version()),
            Style::default().fg(theme.warning).bg(bar_bg),
        ));
        if let Some(ref info) = app.ccusage_info {
            if !spans.is_empty() {
                spans.push(sep.clone());
            }
            spans.push(Span::styled(
                format!("{} tokens", format_tokens(info.total_tokens)),
                Style::default().fg(theme.accent).bg(bar_bg),
            ));
            spans.push(sep.clone());
            spans.push(Span::styled(
                format!("${:.2}", info.total_cost),
                Style::default().fg(theme.success).bg(bar_bg),
            ));
        }
        // Track the update badge text for position calculation.
        let mut update_badge_text: Option<String> = None;
        if let Some(ref update) = app.update_info {
            if !spans.is_empty() {
                spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            }
            let badge = format!(" ↑ v{} available ", update.latest_version);
            update_badge_text = Some(badge.clone());
            spans.push(Span::styled(
                badge,
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        if !spans.is_empty() {
            // Add padding spaces
            spans.insert(0, Span::styled(" ", Style::default().bg(bar_bg)));
            spans.push(Span::styled(" ", Style::default().bg(bar_bg)));

            let stats_line = Line::from(spans);
            let stats_w = stats_line.width() as u16;
            if stats_w + 2 < area.width {
                let stats_x = area.x + area.width - stats_w;
                let stats_area = Rect::new(stats_x, area.y, stats_w, 1);
                frame.render_widget(Paragraph::new(stats_line), stats_area);

                // Compute absolute column range for the update badge.
                if let Some(ref badge) = update_badge_text {
                    let badge_w = UnicodeWidthStr::width(badge.as_str()) as u16;
                    // Badge is at end of stats (before trailing padding space).
                    let badge_end = stats_x + stats_w - 1; // -1 for trailing " "
                    let badge_start = badge_end - badge_w;
                    app.update_badge_cols = Some((badge_start, badge_end));
                } else {
                    app.update_badge_cols = None;
                }
            } else {
                app.update_badge_cols = None;
            }
        } else {
            app.update_badge_cols = None;
        }
    }
}
