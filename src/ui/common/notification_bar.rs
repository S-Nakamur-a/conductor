//! The Claude-Code-waiting notification bar (currently unwired — superseded
//! by the worktree monitor strip, kept for a possible future revival).

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// Render the notification bar showing CC waiting badges.
/// Returns the height consumed (0 if no notifications, 1 if shown).
/// Records badge positions in `app.notification_bar_badges` for click handling.
///
/// Retained but no longer wired: the CC-waiting notification bar was replaced
/// by the worktree monitor strip (which highlights the waiting worktree). Kept
/// for now to keep the diff focused; safe to delete in a follow-up cleanup.
#[allow(dead_code)]
pub fn render_notification_bar(frame: &mut Frame, area: Rect, app: &mut crate::app::App) -> u16 {
    app.notification_bar_badges.clear();

    if app.terminal.cc_waiting_worktrees.is_empty() {
        return 0;
    }

    let theme = &app.theme;

    // Determine the worktree path shown in the focused CC panel (if any).
    let focused_cc_wt: Option<std::path::PathBuf> =
        if app.focus == crate::app::Focus::TerminalClaude {
            Some(app.selected_worktree_path())
        } else {
            None
        };

    // Suppress the entire bar pulse when the only waiting session(s) are all focused.
    let all_suppressed = focused_cc_wt.is_some()
        && app.terminal.cc_waiting_worktrees.len() == 1
        && focused_cc_wt.as_deref()
            == app
                .terminal
                .cc_waiting_worktrees
                .iter()
                .next()
                .map(|p| p.as_path());

    // Orange-tinted background for the notification bar.
    // Breathing pulse: a smooth triangle wave rather than a hard on/off blink,
    // so the bar gently rises and falls instead of flickering in the periphery.
    let cycle = 56u64;
    let phase = (app.ui_tick % cycle) as f64 / cycle as f64;
    let breath = 1.0 - (2.0 * phase - 1.0).abs(); // 0.0 → 1.0 → 0.0
    let bar_bg = if all_suppressed {
        Theme::darken(theme.waiting_primary, 0.17)
    } else {
        Theme::darken(theme.waiting_primary, 0.14 + 0.06 * breath)
    };

    // Fill background.
    let bg_line = Line::from(Span::styled(
        " ".repeat(area.width as usize),
        Style::default().bg(bar_bg),
    ));
    frame.render_widget(Paragraph::new(bg_line), area);

    // Leading indicator.
    let prefix = " ⏳ ";
    let prefix_style = Style::default()
        .fg(theme.waiting_primary)
        .bg(bar_bg)
        .add_modifier(Modifier::BOLD);
    let prefix_area = Rect::new(area.x, area.y, prefix.len() as u16, 1);
    frame.render_widget(
        Paragraph::new(Span::styled(prefix, prefix_style)),
        prefix_area,
    );

    // Collect waiting worktrees sorted by branch name.
    let mut waiting: Vec<(&PathBuf, String)> = app
        .terminal
        .cc_waiting_worktrees
        .iter()
        .map(|p| {
            let name = app
                .worktrees
                .iter()
                .find(|w| &w.path == p)
                .map(|w| w.branch.clone())
                .unwrap_or_else(|| {
                    p.file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or("?")
                        .to_string()
                });
            (p, name)
        })
        .collect();
    waiting.sort_by(|a, b| a.1.cmp(&b.1));

    // Badge colors: breathing vs static (for focused session).
    let badge_bg_pulse = Theme::darken(theme.waiting_secondary, 0.85 + 0.15 * breath);
    let badge_bg_static = theme.waiting_secondary;

    let sep_style = Style::default()
        .fg(Theme::darken(theme.waiting_primary, 0.70))
        .bg(bar_bg);

    let mut x = area.x + UnicodeWidthStr::width(prefix) as u16;

    for (i, (path, name)) in waiting.iter().enumerate() {
        if i > 0 {
            // Separator between badges.
            let sep_area = Rect::new(x, area.y, 1, 1);
            frame.render_widget(Paragraph::new(Span::styled(" ", sep_style)), sep_area);
            x += 1;
        }

        let badge_str = format!(" {name} ⏳ ");
        let w = UnicodeWidthStr::width(badge_str.as_str()) as u16;

        if x + w > area.x + area.width {
            break; // not enough room
        }

        // Suppress blinking for the badge matching the focused CC session.
        let suppress = focused_cc_wt.as_deref() == Some(path.as_path());
        let bg = if suppress {
            badge_bg_static
        } else {
            badge_bg_pulse
        };
        let badge_style = Style::default()
            .fg(Color::Black)
            .bg(bg)
            .add_modifier(Modifier::BOLD);

        let badge_area = Rect::new(x, area.y, w, 1);
        frame.render_widget(
            Paragraph::new(Span::styled(&badge_str, badge_style)),
            badge_area,
        );

        // Record position for click handling.
        app.notification_bar_badges.push((x, x + w, name.clone()));

        x += w;
    }

    // Trailing hint text.
    let hint = " (click to jump)";
    let hint_w = UnicodeWidthStr::width(hint) as u16;
    if x + hint_w < area.x + area.width {
        let hint_area = Rect::new(x + 1, area.y, hint_w, 1);
        let hint_style = Style::default()
            .fg(Theme::darken(theme.waiting_primary, 0.47))
            .bg(bar_bg);
        frame.render_widget(Paragraph::new(Span::styled(hint, hint_style)), hint_area);
    }

    1
}
