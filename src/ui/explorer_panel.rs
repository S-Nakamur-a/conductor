//! Explorer panel — file tree browser in the middle column.
//!
//! Displays the file tree of the currently selected worktree in the top half,
//! and a list of changed (diff) files in the bottom half. Enter on a file
//! opens it in the Viewer panel.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use crate::app::{App, Focus};

/// Return an emoji icon for a file based on its extension or name.
fn file_icon(name: &str) -> &'static str {
    // Special filenames first.
    let lower = name.to_ascii_lowercase();
    let special = match lower.as_str() {
        "cargo.toml" | "cargo.lock" => Some("🦀"),
        "package.json" | "package-lock.json" => Some("📦"),
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" => Some("🐳"),
        "makefile" | "cmake" | "cmakelists.txt" => Some("🔧"),
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some("🔀"),
        "license" | "license.md" | "license.txt" => Some("📜"),
        "readme.md" | "readme" | "readme.txt" => Some("📖"),
        _ => None,
    };
    if let Some(icon) = special {
        return icon;
    }

    // By extension.
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("rs") => "🦀",
        Some("py") => "🐍",
        Some("js") | Some("mjs") | Some("cjs") => "🟨",
        Some("ts") | Some("mts") | Some("cts") => "🔷",
        Some("jsx") | Some("tsx") => "⚛\u{fe0f}",
        Some("go") => "🐹",
        Some("rb") => "💎",
        Some("java" | "class" | "jar") => "☕",
        Some("c" | "h") => "🇨",
        Some("cpp" | "cc" | "cxx" | "hpp") => "⚙\u{fe0f}",
        Some("cs") => "🟪",
        Some("swift") => "🐦",
        Some("kt" | "kts") => "🟣",
        Some("php") => "🐘",
        Some("lua") => "🌙",
        Some("sh" | "bash" | "zsh" | "fish") => "🐚",
        Some("html" | "htm") => "🌐",
        Some("css" | "scss" | "sass" | "less") => "🎨",
        Some("json" | "jsonc" | "json5") => "📋",
        Some("yaml" | "yml") => "📄",
        Some("toml") => "⚙\u{fe0f}",
        Some("xml" | "xsl") => "📰",
        Some("md" | "mdx") => "📝",
        Some("txt" | "text") => "📃",
        Some("sql") => "🗄\u{fe0f}",
        Some("graphql" | "gql") => "🔮",
        Some("proto") => "📡",
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" | "bmp") => "🖼\u{fe0f}",
        Some("mp4" | "mov" | "avi" | "webm") => "🎬",
        Some("mp3" | "wav" | "ogg" | "flac") => "🎵",
        Some("zip" | "tar" | "gz" | "bz2" | "xz" | "rar" | "7z") => "📦",
        Some("pdf") => "📕",
        Some("lock") => "🔒",
        Some("env") => "🔐",
        Some("log") => "📜",
        Some("wasm") => "🟦",
        Some("d.ts") => "🔷",
        Some("test" | "spec") => "🧪",
        _ => "📄",
    }
}

/// Render the explorer (file tree) panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Explorer;

    // Split into top (file tree) and bottom (diff list).
    let chunks = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Record actual panel heights for scroll calculations in event handling.
    let tree_inner_height = chunks[0].height.saturating_sub(2) as usize;
    let diff_inner_height = chunks[1].height.saturating_sub(2) as usize;
    app.viewer_state.explorer_tree_height = tree_inner_height.max(1);
    app.viewer_state.explorer_diff_list_height = diff_inner_height.max(1);

    render_file_tree(frame, chunks[0], app, focused);
    if app.viewer_state.explorer_show_comments {
        render_comment_list(frame, chunks[1], app, focused);
    } else {
        render_diff_list(frame, chunks[1], app, focused);
    }

    // Show search input overlay.
    if app.viewer_state.search_active {
        render_search_box(frame, area, &app.viewer_state.search_query);
    }
}

/// Render the file tree (top half).
fn render_file_tree(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
    let vs = &app.viewer_state;
    let tree_focused = panel_focused && !vs.explorer_focus_on_diff_list;
    let border_color = if tree_focused {
        Color::Yellow
    } else if panel_focused {
        Color::White
    } else {
        Color::DarkGray
    };

    let visible = vs.visible_indices();
    let inner_height = area.height.saturating_sub(2) as usize;

    let selected_vis_idx = visible
        .iter()
        .position(|&i| i == vs.tree_selected)
        .unwrap_or(0);

    let title = if visible.len() > inner_height {
        format!(" Explorer ({}/{}) ", selected_vis_idx + 1, visible.len())
    } else {
        " Explorer ".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let scroll = vs.tree_scroll;

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .filter_map(|(vis_idx, &tree_idx)| {
            let entry = vs.file_tree.get(tree_idx)?;
            let indent = "  ".repeat(entry.depth);

            let label = if entry.is_dir {
                let arrow = if entry.is_expanded {
                    "\u{25bc}" // ▼
                } else {
                    "\u{25b6}" // ▶
                };
                format!("{indent}{arrow} \u{1f4c1} {}", entry.name) // 📁
            } else {
                let icon = file_icon(&entry.name);
                format!("{indent}  {icon} {}", entry.name)
            };

            let style = if vis_idx == selected_vis_idx && tree_focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if vis_idx == selected_vis_idx {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };

            Some(ListItem::new(Span::styled(label, style)))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);

    // Render scrollbar when there are more items than fit in the panel.
    if visible.len() > inner_height {
        let inner_area = area.inner(ratatui::layout::Margin { horizontal: 0, vertical: 1 });
        let mut scrollbar_state = ScrollbarState::new(visible.len().saturating_sub(inner_height))
            .position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, inner_area, &mut scrollbar_state);
    }
}

/// Render the diff file list (bottom half) with Committed / Uncommitted sections.
fn render_diff_list(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
    use crate::diff_state::{DiffListEntry, DiffSection};

    let vs = &app.viewer_state;
    let diff_focused = panel_focused && vs.explorer_focus_on_diff_list;
    let border_color = if diff_focused {
        Color::Yellow
    } else if panel_focused {
        Color::White
    } else {
        Color::DarkGray
    };

    let total = app.diff_state.committed_files.len() + app.diff_state.uncommitted_files.len();
    let title = format!(" Diff Files ({total}) ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs.diff_list_scroll;

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

                let style = if idx == vs.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs.diff_list_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                };

                ListItem::new(Span::styled(label, style))
            }
            DiffListEntry::File {
                section,
                file_index,
            } => {
                let files = match section {
                    DiffSection::Committed => &app.diff_state.committed_files,
                    DiffSection::Uncommitted => &app.diff_state.uncommitted_files,
                };
                let file_diff = &files[*file_index];

                let filename = file_diff
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&file_diff.path);

                let icon = file_icon(filename);
                let label = format!(
                    "  {icon} {} +{} -{}",
                    file_diff.path, file_diff.added_lines, file_diff.deleted_lines
                );

                let style = if idx == vs.diff_list_selected && diff_focused {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if idx == vs.diff_list_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else if file_diff.is_new {
                    Style::default().fg(Color::Green)
                } else if file_diff.is_deleted {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::White)
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

    let vs = &app.viewer_state;
    let list_focused = panel_focused && vs.explorer_focus_on_diff_list;
    let border_color = if list_focused {
        Color::Yellow
    } else if panel_focused {
        Color::White
    } else {
        Color::DarkGray
    };

    let total = app.review_state.comments.len();
    let pending = app
        .review_state
        .comments
        .iter()
        .filter(|c| c.status == crate::review_store::CommentStatus::Pending)
        .count();
    let title = format!(" Comments ({pending}/{total}) [Enter:expand e:edit x:del r:resolve R:reply] ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs.comment_list_scroll;

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

                    let kind_badge = match comment.kind {
                        crate::review_store::CommentKind::Suggest => "[S]",
                        crate::review_store::CommentKind::Question => "[Q]",
                    };
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

                    let style = if row_idx == vs.comment_list_selected && list_focused {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else if row_idx == vs.comment_list_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else if comment.status == crate::review_store::CommentStatus::Resolved {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default().fg(Color::White)
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

                    let style = if row_idx == vs.comment_list_selected && list_focused {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else if row_idx == vs.comment_list_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Rgb(120, 120, 140))
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
fn render_search_box(frame: &mut Frame, area: Rect, query: &str) {
    let height = 1_u16;
    let y = area.y + area.height.saturating_sub(height + 1);
    let search_area = Rect::new(area.x + 1, y, area.width.saturating_sub(2), height);

    frame.render_widget(ratatui::widgets::Clear, search_area);

    let text = format!("/{query}\u{2588}");
    let paragraph = ratatui::widgets::Paragraph::new(Span::styled(
        text,
        Style::default().fg(Color::Yellow),
    ));
    frame.render_widget(paragraph, search_area);
}
