//! Dashboard overlays — history viewer, worktree input, cherry-pick,
//! repo selector, and open-repo popups.
//!
//! These are rendered as overlays on top of the main 3-column layout.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use crate::app::{App, UpdateState};
use crate::text_input::TextInput;
use crate::theme::Theme;

/// Set the terminal cursor position for IME at the cursor position within a
/// single-line `TextInput`.
fn set_cursor_for_input(frame: &mut Frame, area: Rect, buffer: &TextInput) {
    let text_width = buffer.display_width_before_cursor() as u16;
    let cursor_x = area.x + text_width;
    let cursor_y = area.y;
    if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

/// Format a single-line `TextInput` with a block cursor at the cursor position.
fn format_input_with_cursor(buffer: &TextInput) -> String {
    format!("{}\u{2588}{}", buffer.text_before_cursor(), buffer.text_after_cursor())
}

/// Render the session history viewer overlay.
pub fn render_history_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    frame.render_widget(ratatui::widgets::Clear, area);

    let (content_area, search_area) = if app.history.search_active {
        let chunks = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let panes = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(70),
    ])
    .split(content_area);

    // Left pane: history record list.
    let list_block = Block::default()
        .title(" Session History (j/k: navigate, /: search, s: save current, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    if app.history.records.is_empty() {
        let paragraph = Paragraph::new("  No history records.")
            .block(list_block)
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, panes[0]);
    } else {
        let items: Vec<ListItem> = app
            .history.records
            .iter()
            .enumerate()
            .map(|(i, record)| {
                let kind_badge = match record.kind.as_str() {
                    "claude_code" => "[CC]",
                    "shell" => "[SH]",
                    _ => "[??]",
                };

                let style = if i == app.history.selected {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg)
                };

                let line = Line::from(vec![
                    Span::styled(format!(" {kind_badge} "), Style::default().fg(theme.info)),
                    Span::styled(record.label.clone(), style),
                ]);

                let detail_line = Line::from(vec![
                    Span::styled(
                        format!("   {} ", record.worktree),
                        Style::default().fg(theme.success),
                    ),
                    Span::styled(
                        record.saved_at.clone(),
                        Style::default().fg(theme.muted),
                    ),
                ]);

                ListItem::new(vec![line, detail_line])
            })
            .collect();

        let list = List::new(items)
            .block(list_block)
            .highlight_style(
                Style::default()
                    .bg(theme.selected_bg_inactive)
                    .add_modifier(Modifier::BOLD),
            );

        let mut state = ListState::default();
        state.select(Some(app.history.selected));
        frame.render_stateful_widget(list, panes[0], &mut state);
    }

    // Right pane: output text.
    let detail_block = Block::default()
        .title(" Output ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let output_text = if let Some(record) = app.history.records.get(app.history.selected) {
        record.output_text.clone()
    } else {
        String::from("No record selected.")
    };

    let paragraph = Paragraph::new(output_text)
        .block(detail_block)
        .style(Style::default().fg(theme.fg))
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, panes[1]);

    // Search bar.
    if let Some(search_rect) = search_area {
        let search_block = Block::default()
            .title(" Search History ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border_focused));

        let inner = search_block.inner(search_rect);
        frame.render_widget(search_block, search_rect);

        let input_text = format_input_with_cursor(&app.history.search_query);
        let paragraph = Paragraph::new(Span::styled(
            input_text,
            Style::default().fg(theme.fg),
        ));
        frame.render_widget(paragraph, inner);
        set_cursor_for_input(frame, inner, &app.history.search_query);
    }
}

/// Render the worktree name input overlay.
pub fn render_worktree_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_height = 3_u16;
    let popup_width = area.width.saturating_sub(8).min(60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + area.height.saturating_sub(popup_height + 2);
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" New Worktree Name (Tab: Smart Mode) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let input_text = format_input_with_cursor(&app.worktree_mgr.input_buffer);
    let paragraph = Paragraph::new(Span::styled(
        input_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(paragraph, inner);
    set_cursor_for_input(frame, inner, &app.worktree_mgr.input_buffer);
}

/// Render the cherry-pick commit picker overlay.
pub fn render_cherry_pick_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 18_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let title = format!(
        " Cherry-pick from {} (Tab: switch, Enter: pick, Esc: close) ",
        app.cherry_pick.source_branch
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.cherry_pick.commits.is_empty() {
        let paragraph = Paragraph::new("  No commits found on this branch.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .cherry_pick.commits
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            let style = if i == app.cherry_pick.selected {
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
                Span::styled(
                    commit.message.clone(),
                    style,
                ),
                Span::styled(
                    format!(" ({}, {})", commit.author, commit.time_ago),
                    Style::default().fg(theme.muted),
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme.selected_bg_inactive)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    state.select(Some(app.cherry_pick.selected));

    frame.render_stateful_widget(list, inner, &mut state);
}

/// Render the repo selector overlay.
pub fn render_repo_selector_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 50_u16.min(area.width.saturating_sub(4));
    let content_lines = app.repo_list.len() as u16;
    let popup_height = (content_lines + 2).min(12).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Switch Repository (Enter: select, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.repo_list.is_empty() {
        let paragraph = Paragraph::new("  No repositories configured.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .repo_list
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            let full_path = path.display().to_string();

            let active_marker = if i == app.repo_list_index {
                "\u{25cf} "
            } else {
                "  "
            };

            let style = if i == app.repo_selector.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let line = Line::from(vec![
                Span::styled(
                    format!(" {active_marker}"),
                    if i == app.repo_list_index {
                        Style::default().fg(theme.success)
                    } else {
                        Style::default().fg(theme.muted)
                    },
                ),
                Span::styled(name, style),
                Span::styled(
                    format!("  {full_path}"),
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
    state.select(Some(app.repo_selector.selected));

    frame.render_stateful_widget(list, inner, &mut state);
}

/// Render the "open repository" path input overlay.
pub fn render_open_repo_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 5_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Open Repository (Enter: open, Esc: cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let input_text = format_input_with_cursor(&app.open_repo.buffer);
    let paragraph = Paragraph::new(Span::styled(
        input_text,
        Style::default().fg(theme.fg),
    ))
    .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
    set_cursor_for_input(frame, inner, &app.open_repo.buffer);
}

/// Render the switch-branch (remote branch checkout) overlay.
pub fn render_switch_branch_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 22_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // Split into filter bar + list.
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .split(popup_area);

    // Filter bar.
    let filter_block = Block::default()
        .title(" Switch Branch (type to filter, Enter: checkout, Esc: cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let filter_inner = filter_block.inner(chunks[0]);
    frame.render_widget(filter_block, chunks[0]);

    let filter_text = format_input_with_cursor(&app.switch_branch.filter);
    let filter_para = Paragraph::new(Span::styled(
        filter_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.switch_branch.filter);

    // Branch list.
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = app.filtered_switch_branches();
    if filtered.is_empty() {
        let paragraph = Paragraph::new("  No matching branches.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, list_inner);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(vis_idx, (_orig_idx, branch))| {
            let style = if vis_idx == app.switch_branch.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(
                format!("  {branch}"),
                style,
            )))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.switch_branch.selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

/// Render the grab branch picker overlay.
pub fn render_grab_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 50_u16.min(area.width.saturating_sub(4));
    let content_lines = app.grab.branches.len() as u16;
    let popup_height = (content_lines + 2).min(14).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Grab \u{2192} main (Enter: grab, Esc: cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.success));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.grab.branches.is_empty() {
        let paragraph = Paragraph::new("  No branches to grab.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .grab.branches
        .iter()
        .enumerate()
        .map(|(i, branch)| {
            let style = if i == app.grab.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(
                format!("  {branch}"),
                style,
            )))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg_inactive)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.grab.selected));
    frame.render_stateful_widget(list, inner, &mut state);
}

/// Render the prune confirmation overlay.
pub fn render_prune_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let stale_count = app.prune.stale.len() as u16;
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
            format!("  Found {} stale worktree(s):", app.prune.stale.len()),
            Style::default().fg(theme.accent),
        )),
        Line::from(""),
    ];

    for name in &app.prune.stale {
        lines.push(Line::from(Span::styled(
            format!("    - {name}"),
            Style::default().fg(theme.fg),
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Render the base branch input overlay (step 2 of worktree creation).
pub fn render_worktree_base_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 22_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // Split into filter bar + list.
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .split(popup_area);

    // Filter bar.
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
    let filter_para = Paragraph::new(Span::styled(
        filter_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.worktree_mgr.base_branch_filter);

    // Branch list.
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
            format!("  No matches. Enter will use '{}' as base ref.", app.worktree_mgr.base_branch_filter)
        };
        let paragraph = Paragraph::new(hint)
            .style(Style::default().fg(theme.muted));
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
            ListItem::new(Line::from(Span::styled(
                format!("  {branch}"),
                style,
            )))
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

/// Render the branch deletion confirmation overlay.
pub fn render_delete_branch_confirm_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_height = 3_u16;
    let popup_width = area.width.saturating_sub(8).min(65);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + area.height.saturating_sub(popup_height + 2);
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Delete Branch? ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if let Some(ref msg) = app.status_message {
        let paragraph = Paragraph::new(Span::styled(
            msg.text.as_str(),
            Style::default().fg(theme.accent),
        ));
        frame.render_widget(paragraph, inner);
    }
}

// ── Resume Claude session picker overlay ────────────────────────────────

/// Render the resume Claude Code session picker overlay.
pub fn render_resume_session_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 80_u16.min(area.width.saturating_sub(4));
    let popup_height = 24_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // Split into filter bar + list.
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .split(popup_area);

    // Filter bar.
    let scope_label = if app.resume_session.all_projects {
        "all projects"
    } else {
        "this repo"
    };
    let title = format!(
        " Resume CC (Tab: {scope_label}, Enter: resume, Esc: cancel) "
    );
    let filter_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let filter_inner = filter_block.inner(chunks[0]);
    frame.render_widget(filter_block, chunks[0]);

    let filter_text = format_input_with_cursor(&app.resume_session.filter);
    let filter_para = Paragraph::new(Span::styled(
        filter_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.resume_session.filter);

    // Session list.
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = app.filtered_resume_sessions();
    if filtered.is_empty() {
        let paragraph = Paragraph::new("  No matching sessions.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, list_inner);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(vis_idx, (_orig_idx, session))| {
            let style = if vis_idx == app.resume_session.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            // Truncate display to fit within the popup.
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
    state.select(Some(app.resume_session.selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

// ── Command palette overlay ──────────────────────────────────────────────

/// Render the command palette overlay with search bar and command list.
pub fn render_command_palette_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use crate::command_palette;

    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 24_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let chunks = Layout::vertical([
        Constraint::Length(3), // Search bar
        Constraint::Min(3),   // Command list
    ])
    .split(popup_area);

    // Search bar
    let search_block = Block::default()
        .title(" Command Palette (Enter: run, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let search_inner = search_block.inner(chunks[0]);
    frame.render_widget(search_block, chunks[0]);

    let search_text = format_input_with_cursor(&app.command_palette.filter);
    frame.render_widget(
        Paragraph::new(Span::styled(
            search_text,
            Style::default().fg(theme.fg),
        )),
        search_inner,
    );
    set_cursor_for_input(frame, search_inner, &app.command_palette.filter);

    // Command list
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = command_palette::filter_commands(&app.command_palette.filter);
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("  No matching commands.")
                .style(Style::default().fg(theme.muted)),
            list_inner,
        );
        return;
    }

    // Build list items with keybinding hints
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, scored)| {
            let cmd = &command_palette::COMMANDS[scored.index];
            let style = if i == app.command_palette.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let kb = cmd.keybinding.unwrap_or("");
            let line = Line::from(vec![
                Span::styled(
                    if i == app.command_palette.selected {
                        " > "
                    } else {
                        "   "
                    },
                    Style::default().fg(theme.accent),
                ),
                Span::styled(cmd.label, style),
                Span::styled(
                    format!("  {kb:>12}"),
                    Style::default().fg(theme.muted),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(Some(app.command_palette.selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

// ── Help overlay ────────────────────────────────────────────────────────

/// Render the help overlay showing keybindings for the current context.
pub fn render_help_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use crate::app::Focus;

    let theme = &app.theme;
    let popup_width = 72_u16.min(area.width.saturating_sub(4));
    let popup_height = 30_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // Tab bar showing which panel's help is displayed.
    let tabs = Layout::vertical([Constraint::Length(1), Constraint::Min(3)])
        .split(popup_area);

    let tab_labels = [
        ("1:Worktree", Focus::Worktree),
        ("2:Explorer", Focus::Explorer),
        ("3:Viewer", Focus::Viewer),
        ("4:Terminal", Focus::TerminalClaude),
    ];

    let tab_spans: Vec<Span> = tab_labels
        .iter()
        .flat_map(|(label, focus)| {
            let style = if *focus == app.help.context
                || (*focus == Focus::TerminalClaude
                    && app.help.context == Focus::TerminalShell)
            {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            };
            vec![
                Span::styled(format!(" {label} "), style),
                Span::styled(" ", Style::default()),
            ]
        })
        .collect();

    let tab_line = Paragraph::new(Line::from(tab_spans))
        .style(Style::default().bg(theme.titlebar_bg));
    frame.render_widget(tab_line, tabs[0]);

    // Main content block.
    let block = Block::default()
        .title(" Help (?/Esc: close, 1-4: switch panel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(tabs[1]);
    frame.render_widget(block, tabs[1]);

    let lines = help_lines_for(app, app.help.context, theme);
    let paragraph = Paragraph::new(lines)
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Add a section header line.
fn help_section(lines: &mut Vec<Line<'static>>, title: &'static str, theme: &Theme) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        title,
        Style::default()
            .fg(theme.info)
            .add_modifier(Modifier::BOLD),
    )));
}

/// Add a key binding line (dynamic: keys from KeyMap).
fn help_key_dyn(lines: &mut Vec<Line<'static>>, keys: String, desc: &'static str, theme: &Theme) {
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {keys:<18}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc, Style::default().fg(theme.fg)),
    ]));
}

/// Format keys for an action in a given context (e.g. "j / Down").
fn fmt_keys(app: &App, ctx: crate::keymap::KeyContext, action: crate::keymap::Action) -> String {
    let keys = app.keymap.keys_for_action(ctx, action);
    if keys.is_empty() {
        "(unbound)".to_string()
    } else {
        keys.join(" / ")
    }
}

/// Build help text lines for the given focus context.
fn help_lines_for(app: &App, focus: crate::app::Focus, theme: &Theme) -> Vec<Line<'static>> {
    use crate::app::Focus;
    use crate::keymap::{Action, KeyContext};

    let mut lines = Vec::new();

    // Global section always shown.
    help_section(&mut lines, "Global", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::NewClaudeCode), "New Claude Code session", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::NewShell), "New Shell session", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::CommandPalette), "Command palette", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::FocusWorktree), "Jump to Worktree panel", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::OpenRepo), "Open repository by path", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::SwitchRepo), "Switch repository", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::SearchFullText), "Full-text search (grep)", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::CycleFocusForward), "Cycle panel focus forward", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::CycleFocusBackward), "Cycle panel focus backward", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::Quit), "Quit application", theme);
    help_key_dyn(&mut lines, fmt_keys(app, KeyContext::Global, Action::ShowHelp), "Toggle this help", theme);

    match focus {
        Focus::Worktree => {
            let ctx = KeyContext::Worktree;
            help_section(&mut lines, "Worktree Panel", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::NavigateDown), "Navigate down", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::NavigateUp), "Navigate up", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::Select), "Select worktree -> Explorer", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::CreateWorktree), "Create new worktree", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::DeleteWorktree), "Delete selected worktree", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::SwitchBranch), "Switch (checkout remote branch)", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::GrabBranch), "Grab (checkout branch on main)", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::UngrabBranch), "Ungrab (restore main branch)", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::CherryPick), "Cherry-pick from other branch", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::PruneWorktrees), "Prune stale worktrees", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::MergeToMain), "Merge branch into main", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::RefreshWorktrees), "Refresh worktree list", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ResetMainToOrigin), "Reset main to origin/main", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::PullWorktree), "Pull worktree", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::SessionHistory), "Session history viewer", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::OpenPullRequest), "Open pull request in browser", theme);
        }
        Focus::Explorer => {
            let ctx = KeyContext::Explorer;
            help_section(&mut lines, "Explorer Panel (File Tree)", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::NavigateDown), "Navigate down", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::NavigateUp), "Navigate up", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ExpandOrRight), "Expand directory", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::CollapseOrLeft), "Collapse directory", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::Select), "Open file -> Viewer", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ShowDiffList), "Switch to Diff list", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ShowCommentList), "Show review comments", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::SearchFilename), "Search filename", theme);

            let ctx2 = KeyContext::ExplorerDiffList;
            help_section(&mut lines, "Explorer Panel (Diff List)", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx2, Action::NavigateDown), "Navigate down", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx2, Action::NavigateUp), "Navigate up", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx2, Action::Select), "Open diff file -> Viewer", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx2, Action::ExitSubPanel), "Back to file tree", theme);

            let ctx3 = KeyContext::ExplorerCommentList;
            help_section(&mut lines, "Explorer Panel (Comments)", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::NavigateDown), "Navigate down", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::NavigateUp), "Navigate up", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::GoToTop), "Jump to top", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::GoToBottom), "Jump to bottom", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::Select), "Expand/collapse or jump", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::CollapseOrLeft), "Collapse thread", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::EditComment), "Edit selected comment", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::DeleteComment), "Delete selected comment", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::ToggleResolve), "Toggle resolve/pending", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::ReplyToComment), "Reply to comment", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx3, Action::ExitSubPanel), "Back to file tree", theme);
        }
        Focus::Viewer => {
            let ctx = KeyContext::Viewer;
            help_section(&mut lines, "Viewer Panel", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::NavigateDown), "Scroll down", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::NavigateUp), "Scroll up", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ScrollHalfPageDown), "Scroll half-page down", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ScrollHalfPageUp), "Scroll half-page up", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::GoToTop), "Jump to top", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::GoToBottom), "Jump to bottom", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::SearchInFile), "Search in file", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::NextSearchMatch), "Next search match", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::PrevSearchMatch), "Previous search match", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::AddComment), "Add review comment at line", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ExitToExplorer), "Back to Explorer", theme);
        }
        Focus::TerminalClaude | Focus::TerminalShell => {
            let ctx = KeyContext::Terminal;
            help_section(&mut lines, "Terminal Panel", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::LeaveTerminal), "Leave terminal -> Explorer", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ScrollbackUp), "Scroll up", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ScrollbackDown), "Scroll down", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::ScrollbackTop), "Scroll to top", theme);
            help_key_dyn(&mut lines, fmt_keys(app, ctx, Action::SnapToLive), "Snap to live", theme);

            help_section(&mut lines, "Note", theme);
            lines.push(Line::from(Span::styled(
                "  While in the terminal, all keys except the above are",
                Style::default().fg(theme.muted),
            )));
            lines.push(Line::from(Span::styled(
                "  sent directly to the running process (Claude Code / Shell).",
                Style::default().fg(theme.muted),
            )));
            lines.push(Line::from(Span::styled(
                "  Use mouse click to switch panels without leaving.",
                Style::default().fg(theme.muted),
            )));
        }
    }

    lines
}

// ── Smart Worktree overlays ──────────────────────────────────────────

/// Render the Smart Worktree description input overlay (multi-line).
pub fn render_smart_description_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 80_u16.min(area.width.saturating_sub(4));
    let popup_height = 14_u16.min(area.height.saturating_sub(4));
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

    // Split: text area + help hint
    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    // Render multi-line text with block cursor.
    let display = format!(
        "{}\u{2588}{}",
        app.worktree_mgr.smart_description_buffer.text_before_cursor(),
        app.worktree_mgr.smart_description_buffer.text_after_cursor()
    );
    let paragraph = Paragraph::new(display)
        .style(Style::default().fg(theme.fg))
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, chunks[0]);
    {
        let (row, _col) = app.worktree_mgr.smart_description_buffer.cursor_row_col();
        let cursor_x = chunks[0].x + app.worktree_mgr.smart_description_buffer.display_width_before_cursor() as u16;
        let cursor_y = chunks[0].y + row as u16;
        if cursor_x < chunks[0].x + chunks[0].width && cursor_y < chunks[0].y + chunks[0].height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }

    // Help hint.
    let hint = Line::from(vec![
        Span::styled("Alt+Enter", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled(": newline  ", Style::default().fg(theme.muted)),
        Span::styled("Enter", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled(": generate  ", Style::default().fg(theme.muted)),
        Span::styled("Tab", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled(": manual  ", Style::default().fg(theme.muted)),
        Span::styled("Esc", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled(": cancel", Style::default().fg(theme.muted)),
    ]);
    frame.render_widget(Paragraph::new(hint), chunks[1]);
}

/// Render the Smart Worktree generating/loading overlay.
pub fn render_smart_generating_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 60_u16.min(area.width.saturating_sub(4));
    let popup_height = 5_u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Smart Worktree ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Braille spinner animation using ui_tick.
    let braille = ['\u{2801}', '\u{2802}', '\u{2804}', '\u{2840}', '\u{2880}', '\u{2820}', '\u{2810}', '\u{2808}'];
    let idx = (app.ui_tick / 4) as usize % braille.len();
    let spinner = braille[idx];

    let lines = vec![
        Line::from(vec![
            Span::styled(format!(" {spinner} "), Style::default().fg(theme.accent)),
            Span::styled(
                "Generating branch name and prompt...",
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(Span::styled(
            " Press Esc to cancel",
            Style::default().fg(theme.muted),
        )),
    ];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Render the Smart Worktree branch confirmation/edit overlay.
pub fn render_smart_confirm_branch_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 9_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Smart Worktree — Confirm Branch Name ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    // Label
    frame.render_widget(
        Paragraph::new(Span::styled(" Branch name:", Style::default().fg(theme.muted))),
        chunks[0],
    );

    // Editable branch name with cursor
    let branch_display = format!(
        " {}\u{2588}{}",
        app.worktree_mgr.smart_branch_name.text_before_cursor(),
        app.worktree_mgr.smart_branch_name.text_after_cursor()
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            branch_display,
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        chunks[1],
    );
    {
        // +1 for the leading space in " {branch_name}"
        let cursor_x = chunks[1].x + 1 + app.worktree_mgr.smart_branch_name.display_width_before_cursor() as u16;
        let cursor_y = chunks[1].y;
        if cursor_x < chunks[1].x + chunks[1].width && cursor_y < chunks[1].y + chunks[1].height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }

    // Blank line
    // chunks[2] is blank separator

    // Prompt preview (truncated)
    let max_preview = (popup_width as usize).saturating_sub(4);
    let truncated: String = app.worktree_mgr.smart_prompt.chars().take(max_preview.saturating_sub(3)).collect();
    let preview = if truncated.len() < app.worktree_mgr.smart_prompt.len() {
        format!(" {truncated}...")
    } else {
        format!(" {}", &app.worktree_mgr.smart_prompt)
    };
    // Replace newlines with spaces for single-line preview.
    let preview = preview.replace('\n', " ");
    frame.render_widget(
        Paragraph::new(Span::styled(preview, Style::default().fg(theme.muted))),
        chunks[3],
    );

    // Help hint
    let hint = Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled(": continue  ", Style::default().fg(theme.muted)),
        Span::styled("Esc", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled(": cancel", Style::default().fg(theme.muted)),
    ]);
    frame.render_widget(Paragraph::new(hint), chunks[4]);
}

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
            Span::styled(" y", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
            Span::styled(": はい / ", Style::default().fg(theme.muted)),
            Span::styled("n", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
            Span::styled(": いいえ", Style::default().fg(theme.muted)),
        ]),
    ];
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

    let braille = ['\u{2801}', '\u{2802}', '\u{2804}', '\u{2840}', '\u{2880}', '\u{2820}', '\u{2810}', '\u{2808}'];
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
            Span::styled(
                &app.update_progress_message,
                Style::default().fg(theme.fg),
            ),
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
