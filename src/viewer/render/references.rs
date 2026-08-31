//! 参照オーバーレイポップアップ — 「Find References」の検索結果を表示する。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::app::App;
use crate::overlay::RefRow;

/// area の中央に参照オーバーレイポップアップを描画する。
pub fn render_references_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let overlay = &app.code_nav.references;
    let theme = &app.appearance.theme;

    // ポップアップの寸法を計算する: 幅70%、高さ60%、中央寄せ。
    let popup_width = (area.width as f32 * 0.7).clamp(40.0, 100.0) as u16;
    let popup_height = (area.height as f32 * 0.6).clamp(10.0, 40.0) as u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let title = format!(
        " References: {} ({} results) ",
        overlay.symbol_name,
        overlay.results.len()
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if overlay.results.is_empty() {
        let msg = Paragraph::new("No references found.").style(Style::default().fg(theme.muted));
        frame.render_widget(msg, inner);
        return;
    }

    // 内側の領域を分割する: リスト領域 + 下部のヒント行。
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    let list_area = chunks[0];
    let hint_area = chunks[1];

    let visible_height = list_area.height as usize;
    let scroll = overlay.scroll;

    let items: Vec<ListItem> = overlay
        .rows()
        .into_iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, row)| {
            let is_selected = i == overlay.selected;
            let fg = |normal| {
                if is_selected {
                    theme.selected_fg
                } else {
                    normal
                }
            };
            let spans = match row {
                RefRow::File {
                    path,
                    count,
                    collapsed,
                } => vec![
                    Span::styled(
                        format!("{} {path}", if collapsed { '▸' } else { '▾' }),
                        Style::default()
                            .fg(fg(theme.accent))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {count}"), Style::default().fg(fg(theme.info))),
                ],
                RefRow::Hit { index } => {
                    let reference = &overlay.results[index];
                    vec![
                        Span::styled(
                            format!("    {:>5}  ", reference.line),
                            Style::default().fg(fg(theme.info)),
                        ),
                        Span::styled(
                            reference.content.trim().to_string(),
                            Style::default().fg(fg(theme.fg)),
                        ),
                    ]
                }
            };
            let style = if is_selected {
                Style::default().bg(theme.selected_bg).fg(theme.selected_fg)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, list_area);

    // ヒント行。
    let hint = Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(theme.accent)),
        Span::styled(": navigate  ", Style::default().fg(theme.fg)),
        Span::styled("h/l", Style::default().fg(theme.accent)),
        Span::styled(": fold  ", Style::default().fg(theme.fg)),
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::styled(": jump  ", Style::default().fg(theme.fg)),
        Span::styled("Esc", Style::default().fg(theme.accent)),
        Span::styled(": close", Style::default().fg(theme.fg)),
    ]));
    frame.render_widget(hint, hint_area);
}
