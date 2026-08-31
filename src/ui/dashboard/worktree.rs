//! worktree 作成のオーバーレイ群: 名前入力、ベースブランチのピッカー、削除確認、
//! Smart Worktree の複数行の説明入力。

use super::input::{format_input_with_cursor, set_cursor_for_input, wrap_with_cursor};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

pub fn render_worktree_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.appearance.theme;
    let popup_height = 3_u16;
    let popup_width = area.width.saturating_sub(8).min(60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" New Worktree Name (Tab: Smart Mode) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let input_text = format_input_with_cursor(&app.worktree_mgr.input_buffer);
    let paragraph = Paragraph::new(Span::styled(input_text, Style::default().fg(theme.fg)));
    frame.render_widget(paragraph, inner);
    set_cursor_for_input(frame, inner, &app.worktree_mgr.input_buffer);
}

/// ベースブランチの入力オーバーレイ（worktree 作成のステップ2）を描画する。
pub fn render_worktree_base_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
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
    let title = format!(
        " Base Branch for '{}' (type to filter, Enter: select, Esc: cancel) ",
        app.worktree_mgr.pending_branch,
    );
    let filter_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let filter_inner = filter_block.inner(chunks[0]);
    frame.render_widget(filter_block, chunks[0]);

    let filter_text = format_input_with_cursor(&app.worktree_mgr.base_branch_filter);
    let filter_para = Paragraph::new(Span::styled(filter_text, Style::default().fg(theme.fg)));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.worktree_mgr.base_branch_filter);

    // ブランチ一覧。
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = app.filtered_base_branches();
    if filtered.is_empty() {
        let hint = if app.worktree_mgr.base_branch_filter.is_empty() {
            "  No branches found.".to_string()
        } else {
            format!(
                "  No matches. Enter will use '{}' as base ref.",
                app.worktree_mgr.base_branch_filter
            )
        };
        let paragraph = Paragraph::new(hint).style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, list_inner);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(vis_idx, (_orig_idx, branch))| {
            let style = if vis_idx == app.worktree_mgr.base_branch_selected {
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
    state.select(Some(app.worktree_mgr.base_branch_selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

// Smart Worktree のオーバーレイ

/// Smart Worktree の説明入力オーバーレイ（複数行）を描画する。
pub fn render_smart_description_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.appearance.theme;
    let popup_width = 80_u16.min(area.width.saturating_sub(4));
    let text_width = popup_width.saturating_sub(2).max(1); // 左右の枠線の内側

    // ブロックカーソルの記号を埋め込み、その折り返し後の位置が周囲のテキストと
    // 完全に同じ計算で求まるようにする。
    let display = format!(
        "{}\u{2588}{}",
        app.worktree_mgr
            .smart_description_buffer
            .text_before_cursor(),
        app.worktree_mgr
            .smart_description_buffer
            .text_after_cursor()
    );
    let (rows, cur_row, cur_col) = wrap_with_cursor(&display, text_width as usize, '\u{2588}');

    // ポップアップは内容に合わせて拡張する: 枠線(2) + テキスト行 + ヒント(1)、
    // ただし画面に収まる範囲にクランプする。テキストが表示可能な高さを超えたら、
    // スクロールオフセットでカーソルを見える位置に保つ。
    let max_height = area.height.saturating_sub(4).max(4);
    let desired_height = (rows.len() as u16).saturating_add(3); // 枠線2 + ヒント1
    let popup_height = desired_height.clamp(6, max_height);
    let text_area_height = popup_height.saturating_sub(3).max(1);

    let scroll = (cur_row as u16).saturating_sub(text_area_height.saturating_sub(1));

    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Smart Worktree — Describe your task ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // 分割: テキストエリア + ヘルプヒント
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    // 事前に折り返し済みの行のうち、見えている（スクロール後の）部分だけを描画する。
    let visible: Vec<Line> = rows
        .iter()
        .skip(scroll as usize)
        .take(text_area_height as usize)
        .map(|r| Line::from(r.clone()))
        .collect();
    let paragraph = Paragraph::new(visible).style(Style::default().fg(theme.fg));
    frame.render_widget(paragraph, chunks[0]);

    // ハードウェアカーソルを、記号の見た目上の位置（表示範囲内）に配置する。
    let cursor_screen_row = cur_row as u16;
    if cursor_screen_row >= scroll {
        let cursor_x = chunks[0].x + cur_col as u16;
        let cursor_y = chunks[0].y + (cursor_screen_row - scroll);
        if cursor_x < chunks[0].x + chunks[0].width && cursor_y < chunks[0].y + chunks[0].height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }

    // ヘルプヒント。
    let hint = Line::from(vec![
        Span::styled(
            "Shift+Enter",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": newline  ", Style::default().fg(theme.muted)),
        Span::styled(
            "Enter",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": generate  ", Style::default().fg(theme.muted)),
        Span::styled(
            "Tab",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": manual  ", Style::default().fg(theme.muted)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": cancel", Style::default().fg(theme.muted)),
    ]);
    frame.render_widget(Paragraph::new(hint), chunks[1]);
}
