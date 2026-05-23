//! Explorer panel — file tree browser in the middle column.
//!
//! Displays the file tree of the currently selected worktree in the top half,
//! and a list of changed (diff) files in the bottom half. Enter on a file
//! opens it in the Viewer panel.

use crate::app::{App, Focus};
use crate::viewer::file_icon;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

/// Render the explorer (file tree) panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Explorer;

    // Split into top (file tree) and bottom (diff list).
    let chunks =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    // Record actual panel heights for scroll calculations in event handling.
    let tree_inner_height = chunks[0].height.saturating_sub(2) as usize;
    let diff_inner_height = chunks[1].height.saturating_sub(2) as usize;
    app.viewer_state.explorer.explorer_tree_height = tree_inner_height.max(1);
    app.viewer_state.explorer.explorer_diff_list_height = diff_inner_height.max(1);

    render_file_tree(frame, chunks[0], app, focused);
    if app.viewer_state.explorer.explorer_show_comments {
        render_comment_list(frame, chunks[1], app, focused);
    } else {
        render_diff_list(frame, chunks[1], app, focused);
    }

    // Show search input overlay (skip cursor positioning when a global overlay covers us).
    let overlay_active = app.is_any_overlay_active();
    if app.viewer_state.search.search_active {
        render_search_box(
            frame,
            area,
            &app.viewer_state.search.search_query,
            &app.theme,
            overlay_active,
        );
    }

    // Show filename search overlay.
    if app.viewer_state.filename_search.filename_search_active {
        render_filename_search_overlay(frame, chunks[0], app, overlay_active);
    }
}

/// Cached indent strings by depth level to avoid repeated allocation.
const INDENT_CACHE: &[&str] = &[
    "",
    "  ",
    "    ",
    "      ",
    "        ",
    "          ",
    "            ",
    "              ",
    "                ",
    "                  ",
];

/// Get an indent string for a given depth, using cache for common depths.
fn indent_for_depth(depth: usize) -> std::borrow::Cow<'static, str> {
    if depth < INDENT_CACHE.len() {
        std::borrow::Cow::Borrowed(INDENT_CACHE[depth])
    } else {
        std::borrow::Cow::Owned("  ".repeat(depth))
    }
}

/// Render the file tree (top half).
fn render_file_tree(frame: &mut Frame, area: Rect, app: &mut App, panel_focused: bool) {
    let tree_focused = panel_focused && !app.viewer_state.explorer.explorer_focus_on_diff_list;
    let border_color = if tree_focused {
        app.theme.border_focused
    } else if panel_focused {
        app.theme.border_secondary
    } else {
        app.theme.border_unfocused
    };

    let visible = app.viewer_state.visible_indices();
    let inner_height = area.height.saturating_sub(2) as usize;

    let tree_selected = app.viewer_state.tree.tree_selected;
    let selected_vis_idx = visible
        .iter()
        .position(|&i| i == tree_selected)
        .unwrap_or(0);

    let title = if visible.len() > inner_height {
        format!(" Explorer ({}/{}) ", selected_vis_idx + 1, visible.len())
    } else {
        " Explorer ".to_string()
    };

    let is_expanded = app.expanded_panel == Some(Focus::Explorer);
    let theme = &app.theme;
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    let border_type = if panel_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let title_style = if tree_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(
            Line::from(Span::styled(
                expand_label,
                Style::default().fg(expand_color),
            ))
            .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let scroll = app.viewer_state.tree.tree_scroll;

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .filter_map(|(vis_idx, &tree_idx)| {
            let entry = app.viewer_state.tree.file_tree.get(tree_idx)?;
            let indent = indent_for_depth(entry.depth);

            let label = if entry.is_dir {
                let arrow = if entry.is_expanded {
                    "\u{25bc}" // ▼
                } else {
                    "\u{25b6}" // ▶
                };
                format!("{indent}{arrow} {} {}", entry.icon, entry.name)
            } else {
                format!("{indent}  {} {}", entry.icon, entry.name)
            };

            let style = if vis_idx == selected_vis_idx && tree_focused {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else if vis_idx == selected_vis_idx {
                Style::default()
                    .fg(theme.selected_fg_inactive)
                    .bg(theme.selected_bg_inactive)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_dir {
                Style::default().fg(theme.info)
            } else {
                Style::default().fg(theme.fg)
            };

            Some(ListItem::new(Span::styled(label, style)))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);

    // Render scrollbar when there are more items than fit in the panel.
    if visible.len() > inner_height {
        let inner_area = area.inner(ratatui::layout::Margin {
            horizontal: 0,
            vertical: 1,
        });
        let mut scrollbar_state =
            ScrollbarState::new(visible.len().saturating_sub(inner_height)).position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, inner_area, &mut scrollbar_state);
    }
}

/// Render the diff file list (bottom half) with Committed / Uncommitted sections.
fn render_diff_list(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
    use crate::diff_state::{DiffListEntry, DiffSection};

    let theme = &app.theme;
    let vs_explorer = &app.viewer_state.explorer;
    let diff_focused = panel_focused && vs_explorer.explorer_focus_on_diff_list;
    let border_color = if diff_focused {
        theme.border_focused
    } else if panel_focused {
        theme.border_secondary
    } else {
        theme.border_unfocused
    };

    let total = app.diff_state.committed_files.len() + app.diff_state.uncommitted_files.len();
    let title = format!(" Diff Files ({total}) ");

    let border_type = if panel_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let title_style = if diff_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs_explorer.diff_list_scroll;

    let items: Vec<ListItem> = app
        .diff_state
        .display_list
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .map(|(idx, entry)| match entry {
            DiffListEntry::SectionHeader {
                section,
                count,
                collapsed,
            } => {
                let arrow = if *collapsed { "\u{25b6}" } else { "\u{25bc}" };
                let label_text = match section {
                    DiffSection::Committed => "Committed",
                    DiffSection::Uncommitted => "Uncommitted",
                };
                let label = format!("{arrow} {label_text} ({count})");

                let style = if idx == vs_explorer.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs_explorer.diff_list_selected {
                    Style::default()
                        .fg(theme.selected_fg_inactive)
                        .bg(theme.selected_bg_inactive)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.info).add_modifier(Modifier::BOLD)
                };

                ListItem::new(Span::styled(label, style))
            }
            DiffListEntry::Directory {
                name,
                depth,
                collapsed,
                ..
            } => {
                let indent = "  ".repeat(*depth);
                let arrow = if *collapsed { "\u{25b6}" } else { "\u{25bc}" };
                let label = format!("  {indent}{arrow} \u{1f4c1} {name}");

                let style = if idx == vs_explorer.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs_explorer.diff_list_selected {
                    Style::default()
                        .fg(theme.selected_fg_inactive)
                        .bg(theme.selected_bg_inactive)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.info)
                };

                ListItem::new(Span::styled(label, style))
            }
            DiffListEntry::File {
                section,
                file_index,
                depth,
            } => {
                let files = match section {
                    DiffSection::Committed => &app.diff_state.committed_files,
                    DiffSection::Uncommitted => &app.diff_state.uncommitted_files,
                };
                let file_diff = &files[*file_index];

                let filename = file_diff.path.rsplit('/').next().unwrap_or(&file_diff.path);

                let indent = "  ".repeat(*depth);
                let icon = file_icon(filename);
                let label = format!(
                    "  {indent}{icon} {filename} +{} -{}",
                    file_diff.added_lines, file_diff.deleted_lines
                );

                let style = if idx == vs_explorer.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs_explorer.diff_list_selected {
                    Style::default()
                        .fg(theme.selected_fg_inactive)
                        .bg(theme.selected_bg_inactive)
                        .add_modifier(Modifier::BOLD)
                } else if file_diff.is_new {
                    Style::default().fg(theme.success)
                } else if file_diff.is_deleted {
                    Style::default().fg(theme.error)
                } else {
                    Style::default().fg(theme.fg)
                };

                ListItem::new(Span::styled(label, style))
            }
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Render the comment list (bottom half, when toggled via `c`).
fn render_comment_list(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
    use crate::review_state::CommentListRow;

    let theme = &app.theme;
    let vs_explorer = &app.viewer_state.explorer;
    let list_focused = panel_focused && vs_explorer.explorer_focus_on_diff_list;
    let border_color = if list_focused {
        theme.border_focused
    } else if panel_focused {
        theme.border_secondary
    } else {
        theme.border_unfocused
    };

    let total = app.review_state.comments.len();
    let pending = app
        .review_state
        .comments
        .iter()
        .filter(|c| c.status == crate::review_store::CommentStatus::Pending)
        .count();
    let title = format!(" Comments ({pending}/{total}) ");
    let ask_claude_label = " ✨ Ask Claude All ";

    let border_type = if panel_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let title_style = if list_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_bottom(
            Line::from(vec![Span::styled(
                ask_claude_label,
                Style::default().fg(Color::Rgb(180, 140, 255)),
            )])
            .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs_explorer.comment_list_scroll;

    let items: Vec<ListItem> = app
        .review_state
        .comment_list_rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .filter_map(|(row_idx, row)| {
            match row {
                CommentListRow::Comment { comment_idx } => {
                    let comment = app.review_state.comments.get(*comment_idx)?;

                    let kind_badge = crate::ui::review::kind_icon(comment.kind);
                    let status_marker = match comment.status {
                        crate::review_store::CommentStatus::Pending => "\u{25cb}", // ○
                        crate::review_store::CommentStatus::Resolved => "\u{2713}", // ✓
                    };

                    let filename = comment
                        .file_path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&comment.file_path);

                    let line_range = if let Some(end) = comment.line_end {
                        format!("L{}-{}", comment.line_start, end)
                    } else {
                        format!("L{}", comment.line_start)
                    };

                    let reply_count = app
                        .review_state
                        .reply_counts
                        .get(&comment.id)
                        .copied()
                        .unwrap_or(0);
                    let reply_badge = if reply_count > 0 {
                        format!("({reply_count}\u{21a9}) ")
                    } else {
                        String::new()
                    };

                    // Expansion indicator.
                    let expand_indicator = if reply_count > 0 {
                        if app.review_state.expanded_comments.contains(&comment.id) {
                            "\u{25bc} " // ▼
                        } else {
                            "\u{25b6} " // ▶
                        }
                    } else {
                        "  "
                    };

                    let prefix = format!(
                        "{expand_indicator}{status_marker} {kind_badge} {reply_badge}{filename}:{line_range} "
                    );
                    let max_body = (area.width as usize).saturating_sub(prefix.len() + 2);
                    let body: String = comment
                        .body
                        .replace('\n', " ")
                        .chars()
                        .take(max_body)
                        .collect();
                    let label = format!("{prefix}{body}");

                    let style = if row_idx == vs_explorer.comment_list_selected && list_focused {
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                            .add_modifier(Modifier::BOLD)
                    } else if row_idx == vs_explorer.comment_list_selected {
                        Style::default()
                            .fg(theme.selected_fg_inactive)
                            .bg(theme.selected_bg_inactive)
                            .add_modifier(Modifier::BOLD)
                    } else if comment.status == crate::review_store::CommentStatus::Resolved {
                        Style::default().fg(theme.muted)
                    } else {
                        Style::default().fg(theme.fg)
                    };

                    Some(ListItem::new(Span::styled(label, style)))
                }
                CommentListRow::Reply {
                    comment_idx,
                    reply_idx,
                } => {
                    let comment = app.review_state.comments.get(*comment_idx)?;
                    let replies = app.review_state.cached_replies.get(&comment.id)?;
                    let reply = replies.get(*reply_idx)?;

                    let author_label = match reply.author {
                        crate::review_store::Author::User => "You",
                        crate::review_store::Author::Claude => "Claude",
                    };

                    let max_body =
                        (area.width as usize).saturating_sub(author_label.len() + 10);
                    let body: String = reply
                        .body
                        .replace('\n', " ")
                        .chars()
                        .take(max_body)
                        .collect();
                    let label = format!("  \u{21b3} [{author_label}] {body}");

                    let style = if row_idx == vs_explorer.comment_list_selected && list_focused {
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                            .add_modifier(Modifier::BOLD)
                    } else if row_idx == vs_explorer.comment_list_selected {
                        Style::default()
                            .fg(theme.selected_fg_inactive)
                            .bg(theme.selected_bg_inactive)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.reply_text)
                    };

                    Some(ListItem::new(Span::styled(label, style)))
                }
            }
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Render a search input box at the bottom of the given area.
fn render_search_box(
    frame: &mut Frame,
    area: Rect,
    query: &crate::text_input::TextInput,
    theme: &crate::theme::Theme,
    suppress_cursor: bool,
) {
    let height = 1_u16;
    let y = area.y + area.height.saturating_sub(height + 1);
    let search_area = Rect::new(area.x + 1, y, area.width.saturating_sub(2), height);

    frame.render_widget(ratatui::widgets::Clear, search_area);

    let text = format!(
        "/{}\u{2588}{}",
        query.text_before_cursor(),
        query.text_after_cursor()
    );
    let paragraph = ratatui::widgets::Paragraph::new(Span::styled(
        text,
        Style::default().fg(theme.search_match_fg),
    ));
    frame.render_widget(paragraph, search_area);
    if !suppress_cursor {
        // +1 for the leading '/' character
        let cursor_x = search_area.x + 1 + query.display_width_before_cursor() as u16;
        if cursor_x < search_area.x + search_area.width {
            frame.set_cursor_position(Position::new(cursor_x, search_area.y));
        }
    }
}

/// Render the filename search overlay on top of the file tree area.
fn render_filename_search_overlay(frame: &mut Frame, area: Rect, app: &App, suppress_cursor: bool) {
    let theme = &app.theme;
    let vs = &app.viewer_state.filename_search;
    let inner_width = area.width.saturating_sub(2);
    let inner_height = area.height.saturating_sub(2) as usize;

    if inner_width == 0 || inner_height == 0 {
        return;
    }

    // Input box at the top of the panel (inside the border).
    let input_y = area.y + 1;
    let input_area = Rect::new(area.x + 1, input_y, inner_width, 1);
    frame.render_widget(ratatui::widgets::Clear, input_area);

    let total_files = vs.filename_search_all_files.len();
    let match_count = vs.filename_search_results.len();
    let counter = format!(" {match_count}/{total_files}");
    let query_width = inner_width.saturating_sub(counter.len() as u16 + 1) as usize;

    let query_input = &vs.filename_search_query;
    let query_text = format!(
        "/{}\u{2588}{}",
        query_input.text_before_cursor(),
        query_input.text_after_cursor()
    );
    // Truncate display if needed.
    let query_truncated: String = query_text.chars().take(query_width).collect();

    let input_line = Line::from(vec![
        Span::styled(query_truncated, Style::default().fg(theme.search_match_fg)),
        Span::styled(counter, Style::default().fg(theme.muted)),
    ]);
    frame.render_widget(ratatui::widgets::Paragraph::new(input_line), input_area);
    if !suppress_cursor {
        // +1 for the leading '/' character
        let cursor_x = input_area.x + 1 + query_input.display_width_before_cursor() as u16;
        if cursor_x < input_area.x + input_area.width {
            frame.set_cursor_position(Position::new(cursor_x, input_area.y));
        }
    }

    // Results list below the input.
    let results_start_y = input_y + 1;
    let results_height = (area.y + area.height).saturating_sub(results_start_y + 1) as usize;

    if results_height == 0 {
        return;
    }

    // Scroll the results if selected is beyond visible range.
    let scroll = if vs.filename_search_selected >= results_height {
        vs.filename_search_selected - results_height + 1
    } else {
        0
    };

    if vs.filename_search_results.is_empty() && !vs.filename_search_query.is_empty() {
        let no_match_area = Rect::new(area.x + 1, results_start_y, inner_width, 1);
        frame.render_widget(ratatui::widgets::Clear, no_match_area);
        frame.render_widget(
            ratatui::widgets::Paragraph::new(Span::styled(
                "No matches",
                Style::default().fg(theme.muted),
            )),
            no_match_area,
        );
        return;
    }

    for (vi, result) in vs
        .filename_search_results
        .iter()
        .skip(scroll)
        .take(results_height)
        .enumerate()
    {
        let y = results_start_y + vi as u16;
        let row_area = Rect::new(area.x + 1, y, inner_width, 1);
        frame.render_widget(ratatui::widgets::Clear, row_area);

        let is_selected = scroll + vi == vs.filename_search_selected;
        let style = if is_selected {
            Style::default()
                .fg(theme.search_current_fg)
                .bg(theme.search_match_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        // Truncate path to fit.
        let display: String = result.path.chars().take(inner_width as usize).collect();
        frame.render_widget(
            ratatui::widgets::Paragraph::new(Span::styled(display, style)),
            row_area,
        );
    }
}
