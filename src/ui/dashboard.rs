//! Dashboard overlays — history viewer, worktree input, cherry-pick,
//! repo selector, and open-repo popups.
//!
//! These are rendered as overlays on top of the main 3-column layout.

use crate::app::{App, UpdateState};
use crate::text_input::TextInput;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

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
    format!(
        "{}\u{2588}{}",
        buffer.text_before_cursor(),
        buffer.text_after_cursor()
    )
}

/// Render the session history viewer overlay.
pub fn render_history_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    frame.render_widget(ratatui::widgets::Clear, area);

    let (content_area, search_area) = if app.overlays.history.search_active {
        let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let panes = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(content_area);

    // Left pane: history record list.
    let list_block = Block::default()
        .title(" Session History (j/k: navigate, /: search, s: save current, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    if app.overlays.history.records.is_empty() {
        let paragraph = Paragraph::new("  No history records.")
            .block(list_block)
            .style(Style::default().fg(theme.muted));
        frame.render_widget(paragraph, panes[0]);
    } else {
        let items: Vec<ListItem> = app
            .overlays
            .history
            .records
            .iter()
            .enumerate()
            .map(|(i, record)| {
                let kind_badge = match record.kind.as_str() {
                    "claude_code" => "[CC]",
                    "shell" => "[SH]",
                    _ => "[??]",
                };

                let style = if i == app.overlays.history.selected {
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
                    Span::styled(record.saved_at.clone(), Style::default().fg(theme.muted)),
                ]);

                ListItem::new(vec![line, detail_line])
            })
            .collect();

        let list = List::new(items).block(list_block).highlight_style(
            Style::default()
                .bg(theme.selected_bg_inactive)
                .add_modifier(Modifier::BOLD),
        );

        let mut state = ListState::default();
        state.select(Some(app.overlays.history.selected));
        frame.render_stateful_widget(list, panes[0], &mut state);
    }

    // Right pane: output text.
    let detail_block = Block::default()
        .title(" Output ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let output_text = if let Some(record) = app
        .overlays
        .history
        .records
        .get(app.overlays.history.selected)
    {
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

        let input_text = format_input_with_cursor(&app.overlays.history.search_query);
        let paragraph = Paragraph::new(Span::styled(input_text, Style::default().fg(theme.fg)));
        frame.render_widget(paragraph, inner);
        set_cursor_for_input(frame, inner, &app.overlays.history.search_query);
    }
}

/// Render the worktree name input overlay.
pub fn render_worktree_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
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

/// Render the repo selector overlay.
pub fn render_repo_selector_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 50_u16.min(area.width.saturating_sub(4));
    let content_lines = app.repo_list.len() as u16;
    let popup_height = (content_lines + 2)
        .min(12)
        .min(area.height.saturating_sub(4));
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
        let paragraph =
            Paragraph::new("  No repositories configured.").style(Style::default().fg(theme.muted));
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

            let style = if i == app.overlays.repo_selector.selected {
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
                Span::styled(format!("  {full_path}"), Style::default().fg(theme.muted)),
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
    state.select(Some(app.overlays.repo_selector.selected));

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

    let input_text = format_input_with_cursor(&app.overlays.open_repo.buffer);
    let paragraph = Paragraph::new(Span::styled(input_text, Style::default().fg(theme.fg)))
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
    set_cursor_for_input(frame, inner, &app.overlays.open_repo.buffer);
}

/// Render the PR-number/URL input overlay ("Review: Review Pull Request…").
///
/// Below the input line, shows either a loading notice while the background
/// gh/git intake runs, or the last failure's message — the overlay stays
/// open on failure (input preserved) so the user can correct and retry.
pub fn render_pr_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let overlay = &app.overlays.pr_input;
    let popup_width = 70_u16.min(area.width.saturating_sub(4));
    let popup_height = 6_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let title = if overlay.loading {
        " Review Pull Request (fetching...) "
    } else {
        " Review Pull Request (Enter: fetch & review, Esc: cancel) "
    };
    let border_color = if overlay.error.is_some() {
        theme.error
    } else {
        theme.border_focused
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);

    let input_text = format_input_with_cursor(&overlay.buffer);
    let input_style = if overlay.loading {
        Style::default().fg(theme.muted)
    } else {
        Style::default().fg(theme.fg)
    };
    frame.render_widget(Paragraph::new(Span::styled(input_text, input_style)), chunks[0]);
    if !overlay.loading {
        set_cursor_for_input(frame, chunks[0], &overlay.buffer);
    }

    let message = if overlay.loading {
        Some(("Fetching PR metadata via gh...".to_string(), theme.muted))
    } else {
        overlay
            .error
            .as_ref()
            .map(|e| (e.clone(), theme.error))
    };
    if let Some((text, color)) = message {
        let paragraph =
            Paragraph::new(Span::styled(text, Style::default().fg(color)))
                .wrap(ratatui::widgets::Wrap { trim: false });
        frame.render_widget(paragraph, chunks[1]);
    }
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
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(popup_area);

    // Filter bar.
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

    // Branch list.
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

/// Render the grab branch picker overlay.
pub fn render_grab_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 50_u16.min(area.width.saturating_sub(4));
    let popup_height = 22_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    // Split into filter bar + list.
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(popup_area);

    // Filter bar.
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

    // Branch list.
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

/// Render the prune confirmation overlay.
pub fn render_prune_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
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
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(popup_area);

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
    let filter_para = Paragraph::new(Span::styled(filter_text, Style::default().fg(theme.fg)));
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
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(popup_area);

    // Filter bar.
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

    // Session list.
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
    state.select(Some(app.overlays.resume_session.selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}

// ── Filename fuzzy-search overlay ─────────────────────────────────────────

/// Render the fuzzy filename-search ("jump to file") modal as a centered popup.
///
/// Rendered at the top level so it stays visible regardless of which panel is
/// focused or maximized — in particular, when the viewer is maximized and the
/// file tree column is collapsed to zero width.
pub fn render_filename_search_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let vs = &app.viewer_state.filename_search;

    let popup_width = 80_u16.min(area.width.saturating_sub(4));
    let popup_height = 24_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let chunks = Layout::vertical([
        Constraint::Length(3), // Search input
        Constraint::Min(1),    // Results
    ])
    .split(popup_area);

    // Search input.
    let total_files = vs.filename_search_all_files.len();
    let match_count = vs.filename_search_results.len();
    let input_block = Block::default()
        .title(format!(
            " Jump to file ({match_count}/{total_files}) — Enter: open, Esc: cancel "
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let input_inner = input_block.inner(chunks[0]);
    frame.render_widget(input_block, chunks[0]);

    let query_text = format_input_with_cursor(&vs.filename_search_query);
    frame.render_widget(
        Paragraph::new(Span::styled(query_text, Style::default().fg(theme.fg))),
        input_inner,
    );
    set_cursor_for_input(frame, input_inner, &vs.filename_search_query);

    // Results list.
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    if vs.filename_search_results.is_empty() {
        let msg = if vs.filename_search_query.is_empty() {
            "  Type to search files…"
        } else {
            "  No matches."
        };
        frame.render_widget(
            Paragraph::new(msg).style(Style::default().fg(theme.muted)),
            list_inner,
        );
        return;
    }

    let items: Vec<ListItem> = vs
        .filename_search_results
        .iter()
        .enumerate()
        .map(|(i, result)| {
            let selected = i == vs.filename_search_selected;
            let style = if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let line = Line::from(vec![
                Span::styled(
                    if selected { " > " } else { "   " },
                    Style::default().fg(theme.accent),
                ),
                Span::styled(result.path.clone(), style),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(Some(vs.filename_search_selected));
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
        Constraint::Min(3),    // Command list
    ])
    .split(popup_area);

    // Search bar
    let search_block = Block::default()
        .title(" Command Palette (Enter: run, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let search_inner = search_block.inner(chunks[0]);
    frame.render_widget(search_block, chunks[0]);

    let search_text = format_input_with_cursor(&app.overlays.command_palette.filter);
    frame.render_widget(
        Paragraph::new(Span::styled(search_text, Style::default().fg(theme.fg))),
        search_inner,
    );
    set_cursor_for_input(frame, search_inner, &app.overlays.command_palette.filter);

    // Command list
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let list_inner = list_block.inner(chunks[1]);
    frame.render_widget(list_block, chunks[1]);

    let context = app.focus.key_context();
    let filtered = command_palette::filter_commands(
        &app.overlays.command_palette.filter,
        &app.keymap,
        context,
    );
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("  No matching commands.").style(Style::default().fg(theme.muted)),
            list_inner,
        );
        return;
    }

    let current_label = match app.focus {
        crate::app::Focus::Worktree => "Worktree",
        crate::app::Focus::Explorer => "Explorer",
        crate::app::Focus::Viewer => "Viewer",
        crate::app::Focus::TerminalClaude => "Claude Code",
        crate::app::Focus::TerminalShell => "Shell",
        crate::app::Focus::Editor => "Editor",
    };
    let scope_header = |scope: command_palette::CommandScope| match scope {
        command_palette::CommandScope::Current => current_label,
        command_palette::CommandScope::Global => "Global",
        command_palette::CommandScope::Other => "Other",
    };

    // Interleave non-selectable scope headers between the (selectable) command
    // rows. `selected` indexes the command rows only, so track the visual row
    // index of the selected command to drive the highlight.
    let selected = app.overlays.command_palette.selected;
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_row: Option<usize> = None;
    let mut last_scope: Option<command_palette::CommandScope> = None;

    for (cmd_idx, scored) in filtered.iter().enumerate() {
        if last_scope != Some(scored.scope) {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {}", scope_header(scored.scope)),
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD | Modifier::DIM),
            ))));
            last_scope = Some(scored.scope);
        }

        let cmd = &command_palette::COMMANDS[scored.index];
        let is_selected = cmd_idx == selected;
        if is_selected {
            selected_row = Some(items.len());
        }
        let style = if is_selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        // Live keybinding from the keymap for the focused context (blank for
        // palette-only commands and commands not bound in this context).
        let kb = cmd
            .action
            .and_then(|a| crate::ui::common::representative_chord(&app.keymap, context, a))
            .unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(
                if is_selected { " > " } else { "   " },
                Style::default().fg(theme.accent),
            ),
            Span::styled(cmd.label, style),
            Span::styled(format!("  {kb:>12}"), Style::default().fg(theme.muted)),
        ]);
        items.push(ListItem::new(line));
    }

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(selected_row);
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
    let tabs = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).split(popup_area);

    let tab_labels = [
        ("1:Worktree", Focus::Worktree),
        ("2:Explorer", Focus::Explorer),
        ("3:Viewer", Focus::Viewer),
        ("4:Terminal", Focus::TerminalClaude),
    ];

    let tab_spans: Vec<Span> = tab_labels
        .iter()
        .flat_map(|(label, focus)| {
            let style = if *focus == app.overlays.help.context
                || (*focus == Focus::TerminalClaude
                    && app.overlays.help.context == Focus::TerminalShell)
            {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme.fg)
            };
            vec![
                Span::styled(format!(" {label} "), style),
                Span::styled(" ", Style::default()),
            ]
        })
        .collect();

    let tab_line =
        Paragraph::new(Line::from(tab_spans)).style(Style::default().bg(theme.titlebar_bg));
    frame.render_widget(tab_line, tabs[0]);

    // Main content block.
    let block = Block::default()
        .title(" Help (?/Esc: close, 1-4: switch panel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(tabs[1]);
    frame.render_widget(block, tabs[1]);

    let lines = help_lines_for(app, app.overlays.help.context, theme);
    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Add a section header line.
fn help_section(lines: &mut Vec<Line<'static>>, title: &'static str, theme: &Theme) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        title,
        Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
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

/// Build the cheatsheet lines for a help tab, **auto-generated** from the
/// keymap so it always lists every binding that fires in that panel — nothing
/// is hand-curated, so no action can be silently missing (the old curated list
/// showed only a fraction). One section per layer, listing that layer's own
/// bindings (global chords are shown once, under "Global").
fn help_lines_for(app: &App, focus: crate::app::Focus, theme: &Theme) -> Vec<Line<'static>> {
    use crate::app::Focus;
    use crate::keymap::{Action, KeyContext};

    let mut lines = Vec::new();

    let section = |lines: &mut Vec<Line<'static>>, title: &'static str, ctx: KeyContext| {
        let mut entries: Vec<(String, &'static str)> = Vec::new();
        for &action in Action::ALL {
            let keys = app.keymap.keys_in_layer(ctx, action);
            if !keys.is_empty() {
                entries.push((keys.join(" / "), action.label()));
            }
        }
        if entries.is_empty() {
            return;
        }
        help_section(lines, title, theme);
        for (keys, desc) in entries {
            help_key_dyn(lines, keys, desc, theme);
        }
    };

    // Panel-specific layers first (most relevant to where you are), then the
    // always-available global chords.
    let panel_ctxs: &[(&'static str, KeyContext)] = match focus {
        Focus::Worktree => &[("Worktree panel", KeyContext::Worktree)],
        Focus::Explorer => &[
            ("Explorer — file tree", KeyContext::Explorer),
            ("Explorer — changed files", KeyContext::ExplorerDiffList),
            ("Explorer — comment list", KeyContext::ExplorerCommentList),
            ("Explorer — walkthrough", KeyContext::ExplorerWalkthrough),
        ],
        Focus::Viewer => &[
            ("Viewer", KeyContext::Viewer),
            ("Viewer — diff mode", KeyContext::ViewerDiffMode),
        ],
        Focus::TerminalClaude | Focus::TerminalShell => &[("Terminal panel", KeyContext::Terminal)],
        Focus::Editor => &[("Editor panel", KeyContext::Editor)],
    };
    for (title, ctx) in panel_ctxs {
        section(&mut lines, title, *ctx);
    }
    help_review_commands_section(&mut lines, theme);
    section(&mut lines, "Global — works anywhere", KeyContext::Global);

    lines
}

/// The PR-intake, walkthrough-generation, and publish commands have no
/// default keybinding (see `default_keybinds.toml`) — they're reached only
/// through the command palette, so `section()` above (which walks
/// `app.keymap`) never finds them. Listed here instead so the help screen
/// still surfaces them.
fn help_review_commands_section(lines: &mut Vec<Line<'static>>, theme: &Theme) {
    help_section(lines, "Review (via command palette)", theme);
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Review Pull Request…",
        theme,
    );
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Generate Walkthrough",
        theme,
    );
    help_key_dyn(
        lines,
        "palette".to_string(),
        "Review: Publish Comments to GitHub",
        theme,
    );
}

// ── Smart Worktree overlays ──────────────────────────────────────────

/// Wrap `text` into visual rows that are at most `width` display-columns wide,
/// hard-breaking long lines (and honouring explicit `\n`). Returns the wrapped
/// rows plus the (row, col) of `cursor_char` within them — so the caller can
/// place the cursor and scroll to keep it visible. This mirrors exactly what is
/// rendered (we draw these rows without ratatui's own `Wrap`), so the cursor
/// never drifts from the text the way it did when `Paragraph` re-wrapped behind
/// our back.
fn wrap_with_cursor(text: &str, width: usize, cursor_char: char) -> (Vec<String>, usize, usize) {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut rows: Vec<String> = vec![String::new()];
    let mut cur_w = 0usize;
    let mut cursor_pos = (0usize, 0usize);
    for ch in text.chars() {
        if ch == '\n' {
            rows.push(String::new());
            cur_w = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !rows.last().unwrap().is_empty() {
            rows.push(String::new());
            cur_w = 0;
        }
        if ch == cursor_char {
            cursor_pos = (rows.len() - 1, cur_w);
        }
        rows.last_mut().unwrap().push(ch);
        cur_w += cw;
    }
    (rows, cursor_pos.0, cursor_pos.1)
}

/// Render the Smart Worktree description input overlay (multi-line).
pub fn render_smart_description_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_width = 80_u16.min(area.width.saturating_sub(4));
    let text_width = popup_width.saturating_sub(2).max(1); // inside L/R borders

    // Embed a block-cursor glyph so its wrapped position is computed exactly
    // like the surrounding text.
    let display = format!(
        "{}\u{2588}{}",
        app.worktree_mgr
            .smart_description_buffer
            .text_before_cursor(),
        app.worktree_mgr
            .smart_description_buffer
            .text_after_cursor()
    );
    let (rows, cur_row, cur_col) =
        wrap_with_cursor(&display, text_width as usize, '\u{2588}');

    // Grow the popup with the content: borders (2) + text rows + hint (1),
    // clamped to what fits on screen. A scroll offset keeps the cursor visible
    // once the text outgrows the available height.
    let max_height = area.height.saturating_sub(4).max(4);
    let desired_height = (rows.len() as u16).saturating_add(3); // 2 borders + 1 hint
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

    // Split: text area + help hint
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    // Render only the visible (scrolled) slice of pre-wrapped rows.
    let visible: Vec<Line> = rows
        .iter()
        .skip(scroll as usize)
        .take(text_area_height as usize)
        .map(|r| Line::from(r.clone()))
        .collect();
    let paragraph = Paragraph::new(visible).style(Style::default().fg(theme.fg));
    frame.render_widget(paragraph, chunks[0]);

    // Place the hardware cursor at the glyph's visual position (within view).
    let cursor_screen_row = cur_row as u16;
    if cursor_screen_row >= scroll {
        let cursor_x = chunks[0].x + cur_col as u16;
        let cursor_y = chunks[0].y + (cursor_screen_row - scroll);
        if cursor_x < chunks[0].x + chunks[0].width && cursor_y < chunks[0].y + chunks[0].height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }

    // Help hint.
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

#[cfg(test)]
mod tests {
    use super::wrap_with_cursor;

    #[test]
    fn explicit_newlines_become_rows() {
        let (rows, r, c) = wrap_with_cursor("ab\ncd\u{2588}", 80, '\u{2588}');
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], "ab");
        // Cursor glyph sits at row 1, after "cd".
        assert_eq!((r, c), (1, 2));
    }

    #[test]
    fn long_line_hard_wraps_at_width_and_tracks_cursor() {
        // 10 chars, width 4 → rows of 4,4,2. Cursor glyph at the very end.
        let (rows, r, c) = wrap_with_cursor("0123456789\u{2588}", 4, '\u{2588}');
        assert_eq!(rows, vec!["0123", "4567", "89\u{2588}"]);
        assert_eq!((r, c), (2, 2));
    }

    #[test]
    fn wide_chars_do_not_split_across_the_boundary() {
        // Each CJK char is 2 cols wide; width 3 fits one per row.
        let (rows, _r, _c) = wrap_with_cursor("あい", 3, '\u{2588}');
        assert_eq!(rows, vec!["あ", "い"]);
    }

    #[test]
    fn empty_text_yields_one_row_and_origin_cursor() {
        let (rows, r, c) = wrap_with_cursor("\u{2588}", 80, '\u{2588}');
        assert_eq!(rows.len(), 1);
        assert_eq!((r, c), (0, 0));
    }
}
