//! Panel number overlay — drawn when Alt key is held.
//!
//! Shows a large number centered on each panel to indicate the Alt+N shortcut.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::{App, Focus};

/// Panel descriptor: (area, label, associated Focus).
struct PanelInfo {
    area: Rect,
    number: &'static str,
    label: &'static str,
    focus: Focus,
}

/// Render the panel number overlay on all panels.
pub fn render_panel_overlay(frame: &mut Frame, app: &App) {
    let columns = app.layout_cache.columns;
    let terminal_split = app.layout_cache.terminal_split;

    let panels = [
        PanelInfo { area: columns[0], number: "1", label: "Worktree", focus: Focus::Worktree },
        PanelInfo { area: columns[1], number: "2", label: "Explorer", focus: Focus::Explorer },
        PanelInfo { area: columns[2], number: "3", label: "Viewer", focus: Focus::Viewer },
        PanelInfo { area: terminal_split[0], number: "4", label: "Claude", focus: Focus::TerminalClaude },
        PanelInfo { area: terminal_split[1], number: "5", label: "Shell", focus: Focus::TerminalShell },
    ];

    for panel in &panels {
        if panel.area.width < 3 || panel.area.height < 3 {
            continue;
        }
        render_single_panel_overlay(frame, panel, app.focus == panel.focus, &app.theme);
    }
}

fn render_single_panel_overlay(
    frame: &mut Frame,
    panel: &PanelInfo,
    is_focused: bool,
    theme: &crate::theme::Theme,
) {
    let area = panel.area;

    // Clear the underlying content to avoid bleed-through.
    frame.render_widget(Clear, area);

    // Background color: focused panels get the accent color (dimmed),
    // unfocused panels get a dark overlay.
    let bg = if is_focused {
        Color::Rgb(40, 60, 80)
    } else {
        Color::Rgb(25, 25, 35)
    };

    let border_color = if is_focused {
        theme.accent
    } else {
        theme.border_unfocused
    };

    let block = Block::bordered()
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(bg));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Build the number + label text, vertically centered.
    let number_style = Style::default()
        .fg(if is_focused { Color::White } else { Color::Rgb(180, 180, 200) })
        .add_modifier(Modifier::BOLD);

    let label_style = Style::default()
        .fg(if is_focused { Color::Rgb(200, 200, 220) } else { Color::Rgb(100, 100, 120) });

    let lines = vec![
        Line::from(Span::styled(panel.number, number_style)),
        Line::from(Span::styled(panel.label, label_style)),
    ];

    // Vertically center the 2-line content.
    let content_height = lines.len() as u16;
    let top_pad = inner.height.saturating_sub(content_height) / 2;
    let text_area = Rect::new(
        inner.x,
        inner.y + top_pad,
        inner.width,
        content_height.min(inner.height),
    );

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, text_area);
}
