//! Claude Code セッション再開ピッカーのオーバーレイ。

use super::input::{format_input_with_cursor, set_cursor_for_input};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

/// Claude Code セッション再開ピッカーのオーバーレイを描画する。
pub fn render_resume_session_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 80_u16.min(area.width.saturating_sub(4));
    let popup_height = 24_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // フィルタバーと一覧に分割する。
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(popup_area);

    // フィルタバー。
    let scope_label = if app.overlays.resume_session.all_projects {
        "all projects"
    } else {
        "this repo"
    };
    let title = format!(" Resume CC (Tab: {scope_label}, Enter: resume, Esc: cancel) ");
    let filter_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let filter_inner = filter_block.inner(chunks[0]);
    frame.render_widget(filter_block, chunks[0]);

    let filter_text = format_input_with_cursor(&app.overlays.resume_session.filter);
    let filter_para = Paragraph::new(Span::styled(filter_text, Style::default().fg(theme.fg)));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.overlays.resume_session.filter);

    // セッション一覧。
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = app.filtered_resume_sessions();
    if filtered.is_empty() {
        let paragraph =
            Paragraph::new("  No matching sessions.").style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, list_inner);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(vis_idx, (_orig_idx, session))| {
            let style = if vis_idx == app.overlays.resume_session.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            // ポップアップ内に収まるよう表示を切り詰める。
            let max_display = (popup_width as usize).saturating_sub(30);
            let display_text: String = session.display.chars().take(max_display).collect();

            let line = Line::from(vec![
                Span::styled(
                    format!(" {:>8} ", session.time_ago),
                    Style::default().fg(theme.muted),
                ),
                Span::styled(
                    format!("[{}] ", session.project_name),
                    Style::default().fg(theme.info),
                ),
                Span::styled(display_text, style),
            ]);

            let id_short = if session.session_id.len() > 12 {
                &session.session_id[..12]
            } else {
                &session.session_id
            };
            let detail_line = Line::from(vec![Span::styled(
                format!("          id: {id_short}"),
                Style::default().fg(theme.muted),
            )]);

            ListItem::new(vec![line, detail_line])
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.overlays.resume_session.selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}
