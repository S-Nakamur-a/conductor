//! レビューを作る前の確認ダイアログ。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::overlay::RevidereArtifact;

pub fn render_revidere_confirm_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let confirm = &app.overlays.revidere_confirm;
    let popup_width = 64_u16.min(area.width.saturating_sub(4));
    let popup_height = 7_u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // 作り直しなのか初めてなのかで、押す前に知りたいことが変わる。
    let (title, situation, verb) = match confirm.artifact {
        RevidereArtifact::None => (" Review ", "No review for this worktree yet.", "analyse"),
        RevidereArtifact::Stale => (
            " Review ",
            "A review exists, but commits have landed since.",
            "analyse",
        ),
        RevidereArtifact::Current => (
            " Re-analyse ",
            "A review for this commit already exists.",
            "re-analyse",
        ),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let accent = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(Span::styled(
            format!(" {situation}"),
            Style::default().fg(theme.fg),
        )),
        Line::from(Span::styled(
            format!(" {} [{}]", confirm.branch, confirm.scope),
            Style::default().fg(theme.muted),
        )),
        Line::from(Span::styled(
            " It calls the AI and takes a few minutes.",
            Style::default().fg(theme.muted),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Enter", accent),
            Span::styled(format!(": {verb} / "), Style::default().fg(theme.muted)),
            Span::styled("Esc", accent),
            Span::styled(": cancel", Style::default().fg(theme.muted)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}
