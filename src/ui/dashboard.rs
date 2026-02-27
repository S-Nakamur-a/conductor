//! Dashboard overlays — history viewer, worktree input, cherry-pick,
//! repo selector, and open-repo popups.
//!
//! These are rendered as overlays on top of the main 3-column layout.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::theme::Theme;

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
        .title(" New Worktree Name ")
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

    let lines = help_lines_for(app.help_context, theme);
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

/// Add a key binding line.
fn help_key(lines: &mut Vec<Line<'static>>, keys: &'static str, desc: &'static str, theme: &Theme) {
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

/// Build help text lines for the given focus context.
fn help_lines_for(focus: crate::app::Focus, theme: &Theme) -> Vec<Line<'static>> {
    use crate::app::Focus;

    let mut lines = Vec::new();

    // Global section always shown.
    help_section(&mut lines, "Global", theme);
    help_key(&mut lines, "Ctrl+n", "New Claude Code session", theme);
    help_key(&mut lines, "Ctrl+t", "New Shell session", theme);
    help_key(&mut lines, "Ctrl+p", "Command palette", theme);
    help_key(&mut lines, "Ctrl+w", "Jump to Worktree panel", theme);
    help_key(&mut lines, "Ctrl+o", "Open repository by path", theme);
    help_key(&mut lines, "Ctrl+r", "Switch repository", theme);
    help_key(&mut lines, "Tab / Shift+Tab", "Cycle panel focus", theme);
    help_key(&mut lines, "q / Q", "Quit application", theme);
    help_key(&mut lines, "?", "Toggle this help", theme);

    match focus {
        Focus::Worktree => {
            help_section(&mut lines, "Worktree Panel", theme);
            help_key(&mut lines, "j / k", "Navigate up/down", theme);
            help_key(&mut lines, "Enter", "Select worktree -> Explorer", theme);
            help_key(&mut lines, "w", "Create new worktree", theme);
            help_key(&mut lines, "X", "Delete selected worktree", theme);
            help_key(&mut lines, "s", "Switch (checkout remote branch)", theme);
            help_key(&mut lines, "g", "Grab (checkout branch on main)", theme);
            help_key(&mut lines, "G", "Ungrab (restore main branch)", theme);
            help_key(&mut lines, "p", "Cherry-pick from other branch", theme);
            help_key(&mut lines, "P", "Prune stale worktrees", theme);
            help_key(&mut lines, "m", "Merge branch into main", theme);
            help_key(&mut lines, "r", "Refresh worktree list", theme);
            help_key(&mut lines, "R", "Reset main to origin/main", theme);
            help_key(&mut lines, "H", "Session history viewer", theme);
        }
        Focus::Explorer => {
            help_section(&mut lines, "Explorer Panel (File Tree)", theme);
            help_key(&mut lines, "j / k", "Navigate up/down", theme);
            help_key(&mut lines, "l / Right", "Expand directory", theme);
            help_key(&mut lines, "h / Left", "Collapse directory", theme);
            help_key(&mut lines, "Enter", "Open file -> Viewer", theme);
            help_key(&mut lines, "d", "Switch to Diff list", theme);

            help_key(&mut lines, "c", "Show review comments", theme);

            help_section(&mut lines, "Explorer Panel (Diff List)", theme);
            help_key(&mut lines, "j / k", "Navigate up/down", theme);
            help_key(&mut lines, "Enter", "Open diff file -> Viewer", theme);
            help_key(&mut lines, "u", "Toggle committed/all diff scope", theme);
            help_key(&mut lines, "Esc", "Back to file tree", theme);

            help_section(&mut lines, "Explorer Panel (Comments)", theme);
            help_key(&mut lines, "j / k", "Navigate up/down", theme);
            help_key(&mut lines, "g / G", "Jump to top/bottom", theme);
            help_key(&mut lines, "Enter / l", "Expand/collapse replies or jump", theme);
            help_key(&mut lines, "h", "Collapse thread", theme);
            help_key(&mut lines, "e", "Edit selected comment", theme);
            help_key(&mut lines, "x", "Delete selected comment", theme);
            help_key(&mut lines, "r", "Toggle resolve/pending", theme);
            help_key(&mut lines, "R", "Reply to comment", theme);
            help_key(&mut lines, "Esc", "Back to file tree", theme);
        }
        Focus::Viewer => {
            help_section(&mut lines, "Viewer Panel", theme);
            help_key(&mut lines, "j / k", "Scroll up/down", theme);
            help_key(&mut lines, "Ctrl+d / Ctrl+u", "Scroll half-page down/up", theme);
            help_key(&mut lines, "g / G", "Jump to top/bottom", theme);
            help_key(&mut lines, "/", "Search in file", theme);
            help_key(&mut lines, "n / N", "Next/prev search match", theme);
            help_key(&mut lines, "c", "Add review comment at line", theme);
            help_key(&mut lines, "Esc", "Back to Explorer", theme);
        }
        Focus::TerminalClaude | Focus::TerminalShell => {
            help_section(&mut lines, "Terminal Panel", theme);
            help_key(&mut lines, "Ctrl+Esc", "Leave terminal -> Explorer", theme);
            help_key(&mut lines, "(all other keys)", "Forwarded to PTY as-is", theme);

            help_section(&mut lines, "Note", theme);
            lines.push(Line::from(Span::styled(
                "  While in the terminal, all keys except Ctrl+Esc are",
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
