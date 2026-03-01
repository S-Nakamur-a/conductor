//! Dashboard overlays — history viewer, worktree input, cherry-pick,
//! repo selector, and open-repo popups.
//!
//! These are rendered as overlays on top of the main 3-column layout.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::theme::Theme;

/// Set the terminal cursor position for IME at the end of a single-line input buffer.
fn set_cursor_for_input(frame: &mut Frame, area: Rect, buffer: &str) {
    let text_width = UnicodeWidthStr::width(buffer) as u16;
    let cursor_x = area.x + text_width;
    let cursor_y = area.y;
    if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

/// Render the session history viewer overlay.
pub fn render_history_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    frame.render_widget(ratatui::widgets::Clear, area);

    let (content_area, search_area) = if app.history_search_active {
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

    if app.history_records.is_empty() {
        let paragraph = Paragraph::new("  No history records.")
            .block(list_block)
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, panes[0]);
    } else {
        let items: Vec<ListItem> = app
            .history_records
            .iter()
            .enumerate()
            .map(|(i, record)| {
                let kind_badge = match record.kind.as_str() {
                    "claude_code" => "[CC]",
                    "shell" => "[SH]",
                    _ => "[??]",
                };

                let style = if i == app.history_selected {
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
        state.select(Some(app.history_selected));
        frame.render_stateful_widget(list, panes[0], &mut state);
    }

    // Right pane: output text.
    let detail_block = Block::default()
        .title(" Output ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let output_text = if let Some(record) = app.history_records.get(app.history_selected) {
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

        let input_text = format!("{}\u{2588}", app.history_search_query);
        let paragraph = Paragraph::new(Span::styled(
            input_text,
            Style::default().fg(theme.fg),
        ));
        frame.render_widget(paragraph, inner);
        set_cursor_for_input(frame, inner, &app.history_search_query);
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

    let input_text = format!("{}\u{2588}", app.worktree_input_buffer);
    let paragraph = Paragraph::new(Span::styled(
        input_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(paragraph, inner);
    set_cursor_for_input(frame, inner, &app.worktree_input_buffer);
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
        app.cherry_pick_source_branch
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.cherry_pick_commits.is_empty() {
        let paragraph = Paragraph::new("  No commits found on this branch.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .cherry_pick_commits
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            let style = if i == app.cherry_pick_selected {
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
    state.select(Some(app.cherry_pick_selected));

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

            let style = if i == app.repo_selector_selected {
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
    state.select(Some(app.repo_selector_selected));

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

    let input_text = format!("{}\u{2588}", app.open_repo_buffer);
    let paragraph = Paragraph::new(Span::styled(
        input_text,
        Style::default().fg(theme.fg),
    ))
    .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
    set_cursor_for_input(frame, inner, &app.open_repo_buffer);
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

    let filter_text = format!("{}\u{2588}", app.switch_branch_filter);
    let filter_para = Paragraph::new(Span::styled(
        filter_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.switch_branch_filter);

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
            let style = if vis_idx == app.switch_branch_selected {
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
    state.select(Some(app.switch_branch_selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

/// Render the grab branch picker overlay.
pub fn render_grab_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 50_u16.min(area.width.saturating_sub(4));
    let content_lines = app.grab_branches.len() as u16;
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

    if app.grab_branches.is_empty() {
        let paragraph = Paragraph::new("  No branches to grab.")
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .grab_branches
        .iter()
        .enumerate()
        .map(|(i, branch)| {
            let style = if i == app.grab_selected {
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
    state.select(Some(app.grab_selected));
    frame.render_stateful_widget(list, inner, &mut state);
}

/// Render the prune confirmation overlay.
pub fn render_prune_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let stale_count = app.prune_stale.len() as u16;
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
            format!("  Found {} stale worktree(s):", app.prune_stale.len()),
            Style::default().fg(theme.accent),
        )),
        Line::from(""),
    ];

    for name in &app.prune_stale {
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
        app.worktree_pending_branch,
    );
    let filter_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let filter_inner = filter_block.inner(chunks[0]);
    frame.render_widget(filter_block, chunks[0]);

    let filter_text = format!("{}\u{2588}", app.base_branch_filter);
    let filter_para = Paragraph::new(Span::styled(
        filter_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.base_branch_filter);

    // Branch list.
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = app.filtered_base_branches();
    if filtered.is_empty() {
        let hint = if app.base_branch_filter.is_empty() {
            "  No branches found.".to_string()
        } else {
            format!("  No matches. Enter will use '{}' as base ref.", app.base_branch_filter)
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
            let style = if vis_idx == app.base_branch_selected {
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
    state.select(Some(app.base_branch_selected));
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
    let scope_label = if app.resume_session_all_projects {
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

    let filter_text = if app.resume_session_filter.is_empty() {
        "\u{2588}".to_string()
    } else {
        format!("{}\u{2588}", app.resume_session_filter)
    };
    let filter_para = Paragraph::new(Span::styled(
        filter_text,
        Style::default().fg(theme.fg),
    ));
    frame.render_widget(filter_para, filter_inner);
    set_cursor_for_input(frame, filter_inner, &app.resume_session_filter);

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
            let style = if vis_idx == app.resume_session_selected {
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
    state.select(Some(app.resume_session_selected));
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

    let search_text = if app.command_palette_filter.is_empty() {
        "\u{2588}".to_string() // block cursor
    } else {
        format!("{}\u{2588}", app.command_palette_filter)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            search_text,
            Style::default().fg(theme.fg),
        )),
        search_inner,
    );
    set_cursor_for_input(frame, search_inner, &app.command_palette_filter);

    // Command list
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let filtered = command_palette::filter_commands(&app.command_palette_filter);
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
            let style = if i == app.command_palette_selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let kb = cmd.keybinding.unwrap_or("");
            let line = Line::from(vec![
                Span::styled(
                    if i == app.command_palette_selected {
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
    state.select(Some(app.command_palette_selected));
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
            let style = if *focus == app.help_context
                || (*focus == Focus::TerminalClaude
                    && app.help_context == Focus::TerminalShell)
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

    let lines = help_lines_for(app, app.help_context, theme);
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
    let display = format!("{}\u{2588}", app.smart_description_buffer);
    let paragraph = Paragraph::new(display)
        .style(Style::default().fg(theme.fg))
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, chunks[0]);
    {
        let last_line = app.smart_description_buffer.split('\n').next_back().unwrap_or("");
        let line_count = app.smart_description_buffer.split('\n').count().saturating_sub(1);
        let cursor_x = chunks[0].x + UnicodeWidthStr::width(last_line) as u16;
        let cursor_y = chunks[0].y + line_count as u16;
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
    let branch_display = format!(" {}\u{2588}", app.smart_branch_name);
    frame.render_widget(
        Paragraph::new(Span::styled(
            branch_display,
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        chunks[1],
    );
    {
        // +1 for the leading space in " {branch_name}"
        let cursor_x = chunks[1].x + 1 + UnicodeWidthStr::width(app.smart_branch_name.as_str()) as u16;
        let cursor_y = chunks[1].y;
        if cursor_x < chunks[1].x + chunks[1].width && cursor_y < chunks[1].y + chunks[1].height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }

    // Blank line
    // chunks[2] is blank separator

    // Prompt preview (truncated)
    let max_preview = (popup_width as usize).saturating_sub(4);
    let truncated: String = app.smart_prompt.chars().take(max_preview.saturating_sub(3)).collect();
    let preview = if truncated.len() < app.smart_prompt.len() {
        format!(" {truncated}...")
    } else {
        format!(" {}", &app.smart_prompt)
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
