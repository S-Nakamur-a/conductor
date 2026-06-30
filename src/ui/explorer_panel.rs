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

    // Split into top (file tree) and bottom (diff list), using the configured,
    // runtime-resizable ratio (Ctrl+Alt+↑/↓). Must match `LayoutCache`'s
    // `explorer_mid_y` so mouse routing lines up with what's drawn.
    let tree_pct = app.config.layout.explorer_split_pct;
    let changed_pct = 100u16.saturating_sub(tree_pct);
    let chunks = Layout::vertical([
        Constraint::Percentage(tree_pct),
        Constraint::Percentage(changed_pct),
    ])
    .split(area);

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

    // The fuzzy filename-search modal is rendered at the top level
    // (see `layout::render_ui`) so it stays visible even when this panel is
    // collapsed to zero width (e.g. while the viewer is maximized).
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
    let on_diff = app.viewer_state.explorer.explorer_focus_on_diff_list;
    let tree_focused = panel_focused && !on_diff;
    // Glide the column-level focus color; the tree is the "active" element when
    // not focused on the diff list, so it eases both when the column gains and
    // when it loses focus. The inactive sub-panel keeps the static secondary tint.
    let border_color = if tree_focused {
        app.animated_border_color(Focus::Explorer)
    } else if panel_focused {
        app.theme.border_secondary
    } else if !on_diff {
        app.animated_border_color(Focus::Explorer)
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

    // Clear first so rows below the last item (or stale rows after scrolling /
    // a height change) don't show the previous frame's glyphs — the same
    // scroll-bleed guard the viewer uses.
    frame.render_widget(ratatui::widgets::Clear, area);
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
    let on_diff = vs_explorer.explorer_focus_on_diff_list;
    let diff_focused = panel_focused && on_diff;
    let border_color = if diff_focused {
        app.animated_border_color(Focus::Explorer)
    } else if panel_focused {
        theme.border_secondary
    } else if on_diff {
        app.animated_border_color(Focus::Explorer)
    } else {
        theme.border_unfocused
    };

    let total = app.diff_state.committed_files.len() + app.diff_state.uncommitted_files.len();
    let title = format!(" Changed files ({total}) ");

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
                // Origin marker: C = committed (in HEAD), U = uncommitted
                // (working tree). A file changed both ways appears twice.
                let marker = match section {
                    DiffSection::Committed => "C",
                    DiffSection::Uncommitted => "U",
                };
                let label = format!(
                    "  {indent}{marker} {icon} {filename} +{} -{}",
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

                // GitHub-style comment badge: 💬N for files with review comments,
                // coloured by whether any are still unresolved.
                let mut spans = vec![Span::styled(label, style)];
                if let Some(badge) = comment_badge(app, &file_diff.path, theme) {
                    spans.push(badge);
                }
                ListItem::new(Line::from(spans))
            }
            DiffListEntry::Summary {} => {
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
                    Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
                };
                ListItem::new(Span::styled("  \u{25A3} SUMMARY", style))
            }
        })
        .collect();

    // Clear first so rows below the last item (or stale rows after scrolling /
    // a height change) don't show the previous frame's glyphs — the same
    // scroll-bleed guard the viewer uses.
    frame.render_widget(ratatui::widgets::Clear, area);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Build a GitHub-style comment-count badge (e.g. ` 💬3`) for a file path, or
/// `None` when the file has no review comments. Unresolved comments colour the
/// badge with the accent; an all-resolved file uses muted.
fn comment_badge<'a>(app: &App, file_path: &str, theme: &crate::theme::Theme) -> Option<Span<'a>> {
    use crate::review_store::CommentStatus;
    let mut total = 0usize;
    let mut unresolved = 0usize;
    for c in app.review_state.comments.iter().filter(|c| c.file_path == file_path) {
        total += 1;
        if c.status == CommentStatus::Pending {
            unresolved += 1;
        }
    }
    if total == 0 {
        return None;
    }
    let color = if unresolved > 0 {
        theme.accent
    } else {
        theme.muted
    };
    Some(Span::styled(
        format!("  \u{1f4ac}{total}"),
        Style::default().fg(color),
    ))
}

/// Render the comment list as a centered full-screen modal (the `C` overlay) —
/// an overview of every review comment on the branch with jump-to-location.
/// Reuses the same comment-list rendering as the explorer bottom pane.
pub fn render_comment_list_overlay(frame: &mut Frame, area: Rect, app: &App) {
    // Clamp lower bounds to `area` so a tiny terminal can't make min > max
    // (which would panic in `u16::clamp`).
    let w = ((area.width as u32 * 70 / 100) as u16).clamp(24.min(area.width), area.width);
    let h = ((area.height as u32 * 80 / 100) as u16).clamp(6.min(area.height), area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);
    frame.render_widget(ratatui::widgets::Clear, popup);
    render_comment_list(frame, popup, app, true);
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
    // Bulk-send button for the whole list — distinct from the per-comment
    // "ask claude" action defined in `viewer_panel::thread_actions`.
    const ASK_CLAUDE_ALL_LABEL: &str = " ✨ Ask Claude All ";

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
    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs_explorer.comment_list_scroll;
    let total_rows = app.review_state.comment_list_rows.len();

    let mut block = Block::default()
        .title(Span::styled(title, title_style))
        .title_bottom(
            Line::from(vec![Span::styled(
                ASK_CLAUDE_ALL_LABEL,
                Style::default().fg(Color::Rgb(180, 140, 255)),
            )])
            .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    // Scroll position indicator (only when the list overflows).
    if total_rows > inner_height {
        let first = scroll + 1;
        let last = (scroll + inner_height).min(total_rows);
        block = block.title_bottom(Line::from(Span::styled(
            format!(" {first}-{last}/{total_rows} "),
            Style::default().fg(theme.muted),
        )));
    }

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
                    let resolved =
                        comment.status == crate::review_store::CommentStatus::Resolved;

                    let kind_badge = crate::ui::review::kind_icon(comment.kind);
                    let status_marker = if resolved { "\u{2713}" } else { "\u{25cb}" };

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
                    let location = format!("{filename}:{line_range}");

                    let reply_count = app
                        .review_state
                        .reply_counts
                        .get(&comment.id)
                        .copied()
                        .unwrap_or(0);
                    // Reply count rides at the end of the row, out of the way
                    // of the location + body the eye scans for.
                    let reply_suffix = if reply_count > 0 {
                        format!(" \u{21a9}{reply_count}")
                    } else {
                        String::new()
                    };

                    // Expansion indicator (only meaningful when replies exist).
                    let expand_indicator = if reply_count > 0 {
                        if app.review_state.expanded_comments.contains(&comment.id) {
                            "\u{25bc} " // ▼
                        } else {
                            "\u{25b6} " // ▶
                        }
                    } else {
                        "  "
                    };

                    // First body line only; collapsing newlines to spaces hid
                    // the fact that a comment had structure. `+N` marks it.
                    let first_line = comment.body.lines().next().unwrap_or("");
                    let extra_lines = comment.body.lines().count().saturating_sub(1);
                    let more_suffix = if extra_lines > 0 {
                        format!(" +{extra_lines}")
                    } else {
                        String::new()
                    };

                    let fixed = format!("{expand_indicator}{status_marker} {kind_badge} {location} ");
                    let max_body = (area.width as usize).saturating_sub(
                        unicode_width::UnicodeWidthStr::width(fixed.as_str())
                            + unicode_width::UnicodeWidthStr::width(more_suffix.as_str())
                            + unicode_width::UnicodeWidthStr::width(reply_suffix.as_str())
                            + 2, // block borders
                    );
                    let body: String = first_line.chars().take(max_body).collect();

                    let selected = row_idx == vs_explorer.comment_list_selected;
                    let item = if selected {
                        // Selected rows keep a uniform highlight for legibility.
                        let style = if list_focused {
                            Style::default()
                                .fg(theme.selected_fg)
                                .bg(theme.selected_bg)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(theme.selected_fg_inactive)
                                .bg(theme.selected_bg_inactive)
                                .add_modifier(Modifier::BOLD)
                        };
                        ListItem::new(Span::styled(
                            format!("{fixed}{body}{more_suffix}{reply_suffix}"),
                            style,
                        ))
                    } else {
                        // Unselected rows use semantic colours so status and
                        // location recede and the body dominates the scan.
                        // Resolved rows recede *entirely* (marker included):
                        // a bright ✓ over a muted body would pull the eye to
                        // exactly the rows that no longer need attention.
                        let marker_style = if resolved {
                            Style::default().fg(theme.muted)
                        } else {
                            Style::default().fg(theme.warning)
                        };
                        let dim = Style::default().fg(theme.muted);
                        let body_style = if resolved {
                            Style::default().fg(theme.muted)
                        } else {
                            Style::default().fg(theme.fg)
                        };
                        ListItem::new(Line::from(vec![
                            Span::styled(expand_indicator.to_string(), dim),
                            Span::styled(status_marker.to_string(), marker_style),
                            Span::raw(" "),
                            crate::ui::review::kind_badge_span(comment.kind, theme),
                            Span::styled(location, dim),
                            Span::raw(" "),
                            Span::styled(body, body_style),
                            Span::styled(more_suffix, dim),
                            Span::styled(reply_suffix, dim),
                        ]))
                    };
                    Some(item)
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

                    // "    ↳ " indent (6) + author + " " (1) + block borders (2).
                    let reply_prefix_w =
                        6 + unicode_width::UnicodeWidthStr::width(author_label) + 1;
                    let max_body = (area.width as usize).saturating_sub(reply_prefix_w + 2);
                    let first_line = reply.body.lines().next().unwrap_or("");
                    let extra_lines = reply.body.lines().count().saturating_sub(1);
                    let more_suffix = if extra_lines > 0 {
                        format!(" +{extra_lines}")
                    } else {
                        String::new()
                    };
                    let body: String = first_line.chars().take(max_body).collect();

                    let selected = row_idx == vs_explorer.comment_list_selected;
                    let item = if selected {
                        let style = if list_focused {
                            Style::default()
                                .fg(theme.selected_fg)
                                .bg(theme.selected_bg)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(theme.selected_fg_inactive)
                                .bg(theme.selected_bg_inactive)
                                .add_modifier(Modifier::BOLD)
                        };
                        ListItem::new(Span::styled(
                            format!("    \u{21b3} {author_label} {body}{more_suffix}"),
                            style,
                        ))
                    } else {
                        // Deeper indent + bold author make the thread
                        // structure visible without reading the text.
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                format!("    \u{21b3} {author_label} "),
                                Style::default()
                                    .fg(theme.info)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(body, Style::default().fg(theme.reply_text)),
                            Span::styled(more_suffix, Style::default().fg(theme.muted)),
                        ]))
                    };
                    Some(item)
                }
            }
        })
        .collect();

    // Clear first so rows below the last item (or stale rows after scrolling /
    // a height change) don't show the previous frame's glyphs — the same
    // scroll-bleed guard the viewer uses.
    frame.render_widget(ratatui::widgets::Clear, area);
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

