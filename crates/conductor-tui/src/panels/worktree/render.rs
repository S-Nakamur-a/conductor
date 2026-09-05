//! 全幅のストリップと、フォーカス中に開く一覧。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::strip;
use crate::workspace::Workspace;

pub fn strip(ws: &Workspace, area: Rect) -> Line<'static> {
    let theme = &ws.theme;
    let panel = &ws.panels.worktree;
    let selected = panel.selected_index();

    let spans = strip::slots(ws, area.width)
        .into_iter()
        .map(|slot| {
            let style = match slot.kind {
                strip::SlotKind::Lead if panel.is_busy() => Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
                strip::SlotKind::Lead => Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD),
                strip::SlotKind::Select(i) if i == selected => Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD),
                strip::SlotKind::Select(i)
                    if ws.panels.terminal.is_waiting(&panel.list()[i].path) =>
                {
                    Style::default().fg(theme.warning)
                }
                strip::SlotKind::Select(_) => Style::default().fg(theme.muted),
                strip::SlotKind::Delete(_) => Style::default().fg(theme.error),
                strip::SlotKind::Add => Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
                strip::SlotKind::Sep => Style::default().fg(theme.border_secondary),
                strip::SlotKind::Muted => Style::default().fg(theme.muted),
            };
            Span::styled(slot.label, style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// フォーカス中だけ画面の中央に開く一覧。ストリップは幅の都合で削るので、
/// 全部を並べて見る場所を別に持つ。
pub fn list(frame: &mut Frame, area: Rect, ws: &Workspace) {
    let theme = &ws.theme;
    let panel = &ws.panels.worktree;
    let selected = panel.selected_index();

    let rows: Vec<Line> = panel
        .list()
        .iter()
        .enumerate()
        .map(|(i, worktree)| {
            let marker = if i == selected { "\u{25b8} " } else { "  " };
            let style = if i == selected {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let mut spans = vec![Span::styled(format!("{marker}{}", worktree.branch), style)];
            if !worktree.is_clean {
                spans.push(Span::styled(
                    format!(
                        " ~{}",
                        worktree.added + worktree.modified + worktree.deleted
                    ),
                    Style::default().fg(theme.warning),
                ));
            }
            spans.push(Span::styled(
                format!("  {}", worktree.path.display()),
                Style::default().fg(theme.dir_fg),
            ));
            Line::from(spans)
        })
        .collect();

    let rows = if rows.is_empty() {
        vec![Line::styled(
            "  no worktrees",
            Style::default().fg(theme.muted),
        )]
    } else {
        rows
    };

    let height = (rows.len() as u16 + 2).min(area.height);
    let width = (area.width * 70 / 100).max(1).min(area.width);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            " Worktrees ",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Reset));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}
