//! Grep (full-text search) overlay renderer.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
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
/// boundary in `s`.  Equivalent to the nightly `str::floor_char_boundary`.
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
pub fn render_grep_search_overlay(frame: &mut Frame, area: Rect, app: &App) {
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
    let title = " Full-text Search (Enter: jump, Esc: close) ";
    let search_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let search_inner = search_block.inner(chunks[0]);
    frame.render_widget(search_block, chunks[0]);

    // Mode indicators: [.*] or [ab] for regex, [Aa] or [aa] for case
    let regex_indicator = if app.grep_search.regex_mode { "[.*]" } else { "[ab]" };
    let case_indicator = if app.grep_search.case_sensitive { "[Aa]" } else { "[aa]" };

    let query_text = format!(
        "{}\u{2588}{}",
        app.grep_search.query.text_before_cursor(),
        app.grep_search.query.text_after_cursor(),
    );

    let mode_width = regex_indicator.len() + 1 + case_indicator.len() + 1; // +spaces
    let available_for_query = search_inner.width as usize;

    if available_for_query > mode_width + 3 {
        let spans = vec![
            Span::styled(
                format!("{regex_indicator} "),
                Style::default().fg(if app.grep_search.regex_mode { theme.accent } else { theme.muted }),
            ),
            Span::styled(
                format!("{case_indicator} "),
                Style::default().fg(if app.grep_search.case_sensitive { theme.accent } else { theme.muted }),
            ),
            Span::styled(query_text, Style::default().fg(theme.fg)),
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), search_inner);

        // Set cursor position (after mode indicators + query text before cursor).
        let prefix_width = mode_width;
        let cursor_offset = app.grep_search.query.display_width_before_cursor();
        let cursor_x = search_inner.x + prefix_width as u16 + cursor_offset as u16;
        let cursor_y = search_inner.y;
        if cursor_x < search_inner.x + search_inner.width && cursor_y < search_inner.y + search_inner.height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled(query_text, Style::default().fg(theme.fg))),
            search_inner,
        );
        set_cursor_for_input(frame, search_inner, &app.grep_search.query);
    }

    // ── Status line ─────────────────────────────────────────────
    let status_text = if app.grep_search.running {
        format!("  Searching... ({} matches so far)", app.grep_search.results.len())
    } else if app.grep_search.results.is_empty() {
        if app.grep_search.query.is_empty() {
            "  Start typing to search".to_string()
        } else if app.grep_search.debounce_deadline.is_some() {
            // Debounce waiting — keep previous status or show nothing.
            String::new()
        } else {
            "  No matches found".to_string()
        }
    } else {
        let total = app.grep_search.results.len();
        let pos = if total > 0 { app.grep_search.selected + 1 } else { 0 };
        format!("  {pos}/{total} matches  |  Ctrl+R: regex  Ctrl+I: case")
    };
    frame.render_widget(
        Paragraph::new(Span::styled(status_text, Style::default().fg(theme.muted))),
        chunks[1],
    );

    // ── Results list ────────────────────────────────────────────
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let list_inner = list_block.inner(chunks[2]);
    frame.render_widget(list_block, chunks[2]);

    if app.grep_search.results.is_empty() {
        return;
    }

    let visible_height = list_inner.height as usize;
    let selected = app.grep_search.selected;

    // Compute scroll offset to keep selected item visible.
    let scroll = {
        let mut s = app.grep_search.scroll;
        if selected < s {
            s = selected;
        }
        if selected >= s + visible_height {
            s = selected + 1 - visible_height;
        }
        s
    };
    // Note: we can't mutate app here, but the scroll tracking is done
    // via the selected index which is sufficient for rendering.

    let inner_width = list_inner.width as usize;

    let items: Vec<ListItem> = app
        .grep_search.results
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, m)| {
            let is_selected = i == selected;

            let location = format!("{}:{}", m.file_path, m.line_number);
            let content = m.line_content.trim();
            let sep = "  ";

            let content_style = if is_selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let prefix = if is_selected { " > " } else { "   " };

            // Calculate the trim offset so match positions stay correct.
            let trim_offset = m.line_content.len() - m.line_content.trim_start().len();

            // Build content spans with match highlighting.
            let max_content = inner_width.saturating_sub(location.len() + sep.len() + 3);
            let mut spans = vec![
                Span::styled(prefix, Style::default().fg(theme.accent)),
                Span::styled(location, Style::default().fg(theme.warning).add_modifier(Modifier::BOLD)),
                Span::styled(sep.to_string(), Style::default()),
            ];

            // Highlight the matched portion within the trimmed content.
            let ms = m.match_start.saturating_sub(trim_offset);
            let me = m.match_end.saturating_sub(trim_offset).min(content.len());

            // Ensure max_content is at a valid UTF-8 character boundary
            // to prevent panics when slicing multi-byte content.
            let safe_max = floor_char_boundary(content, max_content);

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
                    Style::default().bg(theme.accent).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(after.to_string(), content_style));
                if content.len() > safe_max {
                    spans.push(Span::styled("...", Style::default().fg(theme.muted)));
                }
            } else {
                // Fallback: no highlight, just truncate.
                let safe_trunc = floor_char_boundary(content, max_content.saturating_sub(3));
                let display = if content.len() > max_content && max_content > 3 {
                    format!("{}...", &content[..safe_trunc])
                } else {
                    content.to_string()
                };
                spans.push(Span::styled(display, content_style));
            }

            let item = ListItem::new(Line::from(spans));
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
