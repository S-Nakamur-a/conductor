//! Self-update overlays (confirm, in-progress/error) and the review-comment
//! publish confirmation dialog.

use crate::app::{App, UpdateState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Render the update confirmation overlay.
pub fn render_update_confirm_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 55_u16.min(area.width.saturating_sub(4));
    let popup_height = 5_u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Update Conductor ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let version = app
        .update_info
        .as_ref()
        .map(|u| u.latest_version.as_str())
        .unwrap_or("?");

    let lines = vec![
        Line::from(Span::styled(
            format!(" v{version} をダウンロードして再起動しますか？"),
            Style::default().fg(theme.fg),
        )),
        Line::from(vec![
            Span::styled(
                " y",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": はい / ", Style::default().fg(theme.muted)),
            Span::styled(
                "n",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": いいえ", Style::default().fg(theme.muted)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the confirm dialog for publishing review comments to GitHub —
/// shown before the irreversible external POST, listing how many comments
/// will be posted and how many were skipped for not being on a diff line.
pub fn render_publish_confirm_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let Some(confirm) = app.publish_confirm.as_ref() else {
        return;
    };
    let popup_width = 60_u16.min(area.width.saturating_sub(4));
    let popup_height = if confirm.skipped > 0 { 6 } else { 5 };
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Publish Comments to GitHub ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines = vec![Line::from(Span::styled(
        format!(
            " Publish {} comment(s) to PR #{}?",
            confirm.comments.len(),
            confirm.pr_number
        ),
        Style::default().fg(theme.fg),
    ))];
    if confirm.skipped > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                " {} comment(s) skipped — outside the current diff",
                confirm.skipped
            ),
            Style::default().fg(theme.muted),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            " y",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": publish / ", Style::default().fg(theme.muted)),
        Span::styled(
            "n",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": cancel", Style::default().fg(theme.muted)),
    ]));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the update progress/error overlay.
pub fn render_update_progress_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 60_u16.min(area.width.saturating_sub(4));
    let popup_height = 6_u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let (title, border_color) = match app.update_state {
        UpdateState::InProgress => (" Updating Conductor ", theme.info),
        UpdateState::Restarting => (" Restarting... ", theme.success),
        UpdateState::Failed => (" Update Failed ", theme.error),
        _ => (" Update ", theme.info),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let braille = [
        '\u{2801}', '\u{2802}', '\u{2804}', '\u{2840}', '\u{2880}', '\u{2820}', '\u{2810}',
        '\u{2808}',
    ];
    let idx = (app.ui_tick / 4) as usize % braille.len();

    let mut lines = Vec::new();

    if app.update_state == UpdateState::Failed {
        lines.push(Line::from(Span::styled(
            format!(" {}", app.update_progress_message),
            Style::default().fg(theme.error),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Press any key to dismiss",
            Style::default().fg(theme.muted),
        )));
    } else {
        let spinner = braille[idx];
        lines.push(Line::from(vec![
            Span::styled(format!(" {spinner} "), Style::default().fg(theme.accent)),
            Span::styled(&app.update_progress_message, Style::default().fg(theme.fg)),
        ]));
        if app.update_state == UpdateState::InProgress {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Press Esc to cancel",
                Style::default().fg(theme.muted),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
