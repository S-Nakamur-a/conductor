//! Grep (full-text search) overlay renderer — tree view.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::search_result_tree::SearchTreeRow;
use crate::text_input::TextInput;

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

/// Find the largest byte index `<= pos` that is a valid UTF-8 character
/// boundary in `s`.
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Render the grep search overlay.
pub fn render_grep_search_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = &app.theme;

    // 60% width, 70% height, centered.
    let popup_width = ((area.width as u32 * 60 / 100) as u16).max(40).min(area.width.saturating_sub(4));
    let popup_height = ((area.height as u32 * 70 / 100) as u16).max(10).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let chunks = Layout::vertical([
        Constraint::Length(3), // Search bar (with mode indicators)
        Constraint::Length(1), // Status line
        Constraint::Min(3),   // Results list
    ])
    .split(popup_area);

    // ── Search bar ──────────────────────────────────────────────
    let input_focused = app.overlays.grep_search.input_focused;
    let title = if input_focused {
        " Full-text Search (Tab: results, ↓: results, Esc: close) "
    } else {
        " Full-text Search (Tab: input, Enter: jump, h/l: fold, Esc: input) "
    };
    let search_border_color = if input_focused { theme.border_focused } else { theme.border_unfocused };
    let search_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(search_border_color));
    let search_inner = search_block.inner(chunks[0]);
    frame.render_widget(search_block, chunks[0]);

    // Mode indicators: [.*] or [ab] for regex, [Aa] or [aa] for case
    let regex_indicator = if app.overlays.grep_search.regex_mode { "[.*]" } else { "[ab]" };
    let case_indicator = if app.overlays.grep_search.case_sensitive { "[Aa]" } else { "[aa]" };

    let query_text = format!(
        "{}\u{2588}{}",
        app.overlays.grep_search.query.text_before_cursor(),
        app.overlays.grep_search.query.text_after_cursor(),
    );

    let mode_width = regex_indicator.len() + 1 + case_indicator.len() + 1;
    let available_for_query = search_inner.width as usize;

    if available_for_query > mode_width + 3 {
        let spans = vec![
            Span::styled(
                format!("{regex_indicator} "),
                Style::default().fg(if app.overlays.grep_search.regex_mode { theme.accent } else { theme.muted }),
            ),
            Span::styled(
                format!("{case_indicator} "),
                Style::default().fg(if app.overlays.grep_search.case_sensitive { theme.accent } else { theme.muted }),
            ),
            Span::styled(query_text, Style::default().fg(theme.fg)),
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), search_inner);

        if input_focused {
            let prefix_width = mode_width;
            let cursor_offset = app.overlays.grep_search.query.display_width_before_cursor();
            let cursor_x = search_inner.x + prefix_width as u16 + cursor_offset as u16;
            let cursor_y = search_inner.y;
            if cursor_x < search_inner.x + search_inner.width && cursor_y < search_inner.y + search_inner.height {
                frame.set_cursor_position(Position::new(cursor_x, cursor_y));
            }
        }
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled(query_text, Style::default().fg(theme.fg))),
            search_inner,
        );
        if input_focused {
            set_cursor_for_input(frame, search_inner, &app.overlays.grep_search.query);
        }
    }

    // ── Status line ─────────────────────────────────────────────
    let total_matches = app.overlays.grep_search.result_tree.match_count();
    let status_text = if app.overlays.grep_search.running {
        format!("  Searching... ({total_matches} matches so far)")
    } else if total_matches == 0 {
        if app.overlays.grep_search.query.is_empty() {
            "  Start typing to search".to_string()
        } else if app.overlays.grep_search.debounce_deadline.is_some() {
            String::new()
        } else {
            "  No matches found".to_string()
        }
    } else {
        let rows_count = app.overlays.grep_search.result_tree.visible_rows().len();
        let pos = if rows_count > 0 { app.overlays.grep_search.selected + 1 } else { 0 };
        format!("  {pos}/{rows_count} rows ({total_matches} matches)  |  Ctrl+R: regex  Ctrl+I: case")
    };
    frame.render_widget(
        Paragraph::new(Span::styled(status_text, Style::default().fg(theme.muted))),
        chunks[1],
    );

    // ── Results tree ────────────────────────────────────────────
    let list_border_color = if !input_focused { theme.border_focused } else { theme.border_unfocused };
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(list_border_color));
    let list_inner = list_block.inner(chunks[2]);
    frame.render_widget(list_block, chunks[2]);

    let rows = app.overlays.grep_search.result_tree.visible_rows().to_vec();
    if rows.is_empty() {
        return;
    }

    let visible_height = list_inner.height as usize;
    let selected = app.overlays.grep_search.selected.min(rows.len().saturating_sub(1));

    // Compute scroll offset.
    let scroll = {
        let mut s = app.overlays.grep_search.scroll;
        if selected < s {
            s = selected;
        }
        if selected >= s + visible_height {
            s = selected + 1 - visible_height;
        }
        s
    };
    app.overlays.grep_search.scroll = scroll;

    let inner_width = list_inner.width as usize;
    let matches = app.overlays.grep_search.result_tree.matches();

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, row)| {
            let is_selected = i == selected;
            let indent_str = "  ".repeat(row_depth(row));

            let line = match row {
                SearchTreeRow::Dir { name, expanded, match_count, .. } => {
                    let arrow = if *expanded { "▼" } else { "▶" };
                    let prefix = if is_selected { ">" } else { " " };
                    Line::from(vec![
                        Span::styled(format!("{prefix} {indent_str}"), Style::default().fg(theme.accent)),
                        Span::styled(format!("{arrow} "), Style::default().fg(theme.muted)),
                        Span::styled(
                            format!("{name}/"),
                            Style::default().fg(theme.warning).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" ({match_count} matches)"),
                            Style::default().fg(theme.muted),
                        ),
                    ])
                }
                SearchTreeRow::File { name, match_count, expanded, .. } => {
                    let arrow = if *expanded { "▼" } else { "▶" };
                    let prefix = if is_selected { ">" } else { " " };
                    let match_label = if *match_count == 1 { "match" } else { "matches" };
                    Line::from(vec![
                        Span::styled(format!("{prefix} {indent_str}"), Style::default().fg(theme.accent)),
                        Span::styled(format!("{arrow} "), Style::default().fg(theme.muted)),
                        Span::styled(
                            name.clone(),
                            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" ({match_count} {match_label})"),
                            Style::default().fg(theme.muted),
                        ),
                    ])
                }
                SearchTreeRow::Match { match_index, .. } => {
                    let prefix = if is_selected { ">" } else { " " };
                    if let Some(m) = matches.get(*match_index) {
                        let content = m.line_content.trim();
                        let trim_offset = m.line_content.len() - m.line_content.trim_start().len();
                        let location = format!("L{}", m.line_number);

                        let max_content = inner_width
                            .saturating_sub(indent_str.len() + 2 + location.len() + 3);

                        let ms = m.match_start.saturating_sub(trim_offset);
                        let me = m.match_end.saturating_sub(trim_offset).min(content.len());
                        let safe_max = floor_char_boundary(content, max_content);

                        let mut spans = vec![
                            Span::styled(format!("{prefix} {indent_str}"), Style::default().fg(theme.accent)),
                            Span::styled(
                                format!("{location}: "),
                                Style::default().fg(theme.muted),
                            ),
                        ];

                        let content_style = if is_selected {
                            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme.fg)
                        };

                        if ms < me && me <= content.len() && ms < safe_max {
                            let before = &content[..ms];
                            let me_clamped = me.min(safe_max);
                            let matched = &content[ms..me_clamped];
                            let after = if me_clamped < safe_max {
                                &content[me_clamped..safe_max]
                            } else {
                                ""
                            };
                            spans.push(Span::styled(before.to_string(), content_style));
                            spans.push(Span::styled(
                                matched.to_string(),
                                Style::default().fg(theme.search_current_fg).bg(theme.search_match_bg).add_modifier(Modifier::BOLD),
                            ));
                            spans.push(Span::styled(after.to_string(), content_style));
                            if content.len() > safe_max {
                                spans.push(Span::styled("...", Style::default().fg(theme.muted)));
                            }
                        } else {
                            let safe_trunc = floor_char_boundary(content, max_content.saturating_sub(3));
                            let display = if content.len() > max_content && max_content > 3 {
                                format!("{}...", &content[..safe_trunc])
                            } else {
                                content.to_string()
                            };
                            spans.push(Span::styled(display, content_style));
                        }

                        Line::from(spans)
                    } else {
                        Line::from(format!("{prefix} {indent_str}<invalid match>"))
                    }
                }
            };

            let item = ListItem::new(line);
            if is_selected {
                item.style(Style::default().bg(theme.selected_bg))
            } else {
                item
            }
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default();
    if selected >= scroll {
        state.select(Some(selected - scroll));
    }
    frame.render_stateful_widget(list, list_inner, &mut state);
}

fn row_depth(row: &SearchTreeRow) -> usize {
    match row {
        SearchTreeRow::Dir { depth, .. } => *depth,
        SearchTreeRow::File { depth, .. } => *depth,
        SearchTreeRow::Match { depth, .. } => *depth,
    }
}
