//! ブランチ一覧のピッカーオーバーレイ群: cherry-pick の元コミット、switch-branch、
//! grab-from-branch、古い worktree の削除確認。

use super::input::{format_input_with_cursor, set_cursor_for_input};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

pub fn render_cherry_pick_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.appearance.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 18_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let title = format!(
        " Cherry-pick from {} (Tab: switch, Enter: pick, Esc: close) ",
        app.overlays.cherry_pick.source_branch
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.overlays.cherry_pick.commits.is_empty() {
        let paragraph = Paragraph::new("  No commits found on this branch.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .overlays
        .cherry_pick
        .commits
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            let style = if i == app.overlays.cherry_pick.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let line = Line::from(vec![
                Span::styled(
                    format!(" [{}] ", commit.short_oid),
                    Style::default().fg(theme.info),
                ),
                Span::styled(commit.message.clone(), style),
                Span::styled(
                    format!(" ({}, {})", commit.author, commit.time_ago),
                    Style::default().fg(theme.muted),
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.overlays.cherry_pick.selected));

    frame.render_stateful_widget(list, inner, &mut state);
}

/// switch-branch（リモートブランチのチェックアウト）オーバーレイを描画する。
pub fn render_switch_branch_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.appearance.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 22_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // フィルタバーと一覧に分割する。
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(popup_area);

    // フィルタバー。
    let filter_block = Block::default()
        .title(" Switch Branch (type to filter, Enter: checkout, Esc: cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let filter_inner = filter_block.inner(chunks[0]);
    frame.render_widget(filter_block, chunks[0]);

    let filter_text = format_input_with_cursor(&app.overlays.switch_branch.filter);
    let filter_para = Paragraph::new(Span::styled(filter_text, Style::default().fg(theme.fg)));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.overlays.switch_branch.filter);

    // ブランチ一覧。
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = app.filtered_switch_branches();
    if filtered.is_empty() {
        let paragraph =
            Paragraph::new("  No matching branches.").style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, list_inner);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(vis_idx, (_orig_idx, branch))| {
            let style = if vis_idx == app.overlays.switch_branch.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(format!("  {branch}"), style)))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.overlays.switch_branch.selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

pub fn render_grab_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.appearance.theme;
    let popup_width = 50_u16.min(area.width.saturating_sub(4));
    let popup_height = 22_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // フィルタバーと一覧に分割する。
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(popup_area);

    // フィルタバー。
    let filter_block = Block::default()
        .title(" Grab \u{2192} main (type to filter, Enter: grab, Esc: cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.success));

    let filter_inner = filter_block.inner(chunks[0]);
    frame.render_widget(filter_block, chunks[0]);

    let filter_text = format_input_with_cursor(&app.overlays.grab.filter);
    let filter_para = Paragraph::new(Span::styled(filter_text, Style::default().fg(theme.fg)));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.overlays.grab.filter);

    // ブランチ一覧。
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.success));

    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = app.filtered_grab_branches();
    if filtered.is_empty() {
        let paragraph =
            Paragraph::new("  No matching branches.").style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, list_inner);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(vis_idx, (_orig_idx, branch))| {
            let style = if vis_idx == app.overlays.grab.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(format!("  {branch}"), style)))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.overlays.grab.selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

pub fn render_prune_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.appearance.theme;
    let stale_count = app.overlays.prune.stale.len() as u16;
    let popup_width = 60_u16.min(area.width.saturating_sub(4));
    let popup_height = (stale_count + 4).min(16).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Prune Stale Worktrees (y: prune all, Esc/n: cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(
                "  Found {} stale worktree(s):",
                app.overlays.prune.stale.len()
            ),
            Style::default().fg(theme.accent),
        )),
        Line::from(""),
    ];

    for name in &app.overlays.prune.stale {
        lines.push(Line::from(Span::styled(
            format!("    - {name}"),
            Style::default().fg(theme.fg),
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
