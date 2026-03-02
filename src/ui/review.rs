//! Review overlay renderers — input box, template picker, and comment detail.
//!
//! These are rendered as overlays on top of the main layout when active.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use crate::app::App;
use crate::review_state::{ReviewInputMode, ReviewState};
use crate::review_store::CommentKind;
use crate::theme::Theme;

/// Emoji icon for a comment kind.
pub fn kind_icon(kind: CommentKind) -> &'static str {
    match kind {
        CommentKind::Suggest => "\u{1f4a1}", // 💡
        CommentKind::Question => "\u{2753}",  // ❓
    }
}

/// Styled span for a comment kind badge.
pub fn kind_badge_span(kind: CommentKind) -> Span<'static> {
    match kind {
        CommentKind::Suggest => {
            Span::styled(format!("{} ", kind_icon(kind)), Style::default().fg(Color::Green))
        }
        CommentKind::Question => {
            Span::styled(format!("{} ", kind_icon(kind)), Style::default().fg(Color::Magenta))
        }
    }
}

/// Render an input box overlay when adding or editing a comment.
pub fn render_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let popup_height = 12_u16.min(area.height.saturating_sub(4));
    let popup_width = area.width.saturating_sub(8).min(80);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let title = match app.review_state.input_mode {
        ReviewInputMode::AddingComment => {
            let kind_label = match app.review_state.input_kind {
                CommentKind::Suggest => "Suggest",
                CommentKind::Question => "Question",
            };
            let icon = kind_icon(app.review_state.input_kind);
            if cfg!(target_os = "macos") {
                format!(" {icon} New {kind_label} (Tab: toggle | Opt+Enter: newline) ")
            } else {
                format!(" {icon} New {kind_label} (Tab: toggle | Alt+Enter: newline) ")
            }
        }
        ReviewInputMode::EditingComment => {
            if cfg!(target_os = "macos") {
                " Edit Comment (Opt+Enter: newline) ".to_string()
            } else {
                " Edit Comment (Alt+Enter: newline) ".to_string()
            }
        }
        ReviewInputMode::ReplyingToComment => {
            if cfg!(target_os = "macos") {
                " Reply to Comment (Opt+Enter: newline) ".to_string()
            } else {
                " Reply to Comment (Alt+Enter: newline) ".to_string()
            }
        }
        ReviewInputMode::Normal => unreachable!(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    // When replying, show a preview of the parent comment's first line.
    if app.review_state.input_mode == ReviewInputMode::ReplyingToComment {
        if let Some(parent) = app.review_state.comments.get(app.review_state.selected) {
            let first_line = parent.body.lines().next().unwrap_or("");
            let max_len = inner.width.saturating_sub(4) as usize;
            let preview = if first_line.chars().count() > max_len {
                let truncated: String = first_line.chars().take(max_len).collect();
                format!("\u{258e} {truncated}\u{2026}")
            } else {
                format!("\u{258e} {first_line}")
            };
            lines.push(Line::from(Span::styled(
                preview,
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::ITALIC),
            )));
            lines.push(Line::from(""));
        }
    }

    // Build multi-line display with block cursor at the cursor position.
    let buf = &app.review_state.input_buffer;
    let prefix_line_count = lines.len();
    let display = format!(
        "{}\u{2588}{}",
        buf.text_before_cursor(),
        buf.text_after_cursor()
    );
    let input_lines: Vec<Line> = display
        .split('\n')
        .map(|line| Line::from(Span::styled(line.to_string(), Style::default().fg(theme.fg))))
        .collect();

    lines.extend(input_lines);

    // Hint line at the bottom.
    let hint = match app.review_state.input_mode {
        ReviewInputMode::AddingComment => "Enter: submit | Esc: cancel | Tab: toggle kind",
        _ => "Enter: submit | Esc: cancel",
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(theme.muted),
    )));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);

    // Set cursor position for IME.
    {
        let (cursor_row_in_buf, _) = buf.cursor_row_col();
        let cursor_row = prefix_line_count + cursor_row_in_buf;
        let cursor_x = inner.x + buf.display_width_before_cursor() as u16;
        let cursor_y = inner.y + cursor_row as u16;
        if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }
}

/// Render a centered popup for the comment template picker.
pub fn render_template_picker_overlay(frame: &mut Frame, area: Rect, state: &ReviewState, theme: &Theme) {
    let popup_width = 60_u16.min(area.width.saturating_sub(4));
    let popup_height = 15_u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Templates (Enter: use, Del: delete, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if state.templates.is_empty() {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  No templates saved. Use T to save a comment as template.",
            Style::default().fg(theme.muted),
        )]));
        frame.render_widget(empty, inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, tmpl) in state.templates.iter().enumerate() {
        let is_selected = i == state.template_selected;

        let badge = kind_badge_span(tmpl.kind);

        let max_body_len =
            (popup_width as usize).saturating_sub(tmpl.name.chars().count() + 10);
        let body_preview: String = tmpl.body.chars().take(max_body_len).collect();
        let body_preview = body_preview.replace('\n', " ");

        let style = if is_selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        let prefix = if is_selected { "> " } else { "  " };

        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            badge,
            Span::styled(&tmpl.name, style),
        ]));
        lines.push(Line::from(vec![Span::styled(
            format!("    {body_preview}"),
            Style::default().fg(theme.muted),
        )]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Render a centered detail modal for viewing a full comment and its replies.
pub fn render_comment_detail_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = &app.theme;
    let popup_width = 72_u16.min(area.width.saturating_sub(4));
    let popup_height = area.height.saturating_sub(4).max(10);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let comment = match app.review_state.comments.get(app.review_state.comment_detail_idx) {
        Some(c) => c,
        None => return,
    };

    let icon = kind_icon(comment.kind);
    let kind_label = match comment.kind {
        CommentKind::Suggest => "Suggest",
        CommentKind::Question => "Question",
    };
    let status_label = match comment.status {
        crate::review_store::CommentStatus::Pending => "\u{25cb} Pending",
        crate::review_store::CommentStatus::Resolved => "\u{2713} Resolved",
    };

    let title = format!(" {icon} {kind_label} \u{2502} {status_label} (Esc/q: close, e: edit, R: reply, r: resolve, Del: delete) ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let inner_width = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();

    // Location header.
    let line_range = if let Some(end) = comment.line_end {
        format!("{}:{}-{}", comment.file_path, comment.line_start, end)
    } else {
        format!("{}:{}", comment.file_path, comment.line_start)
    };
    lines.push(Line::from(vec![
        Span::styled(" \u{1f4cd} ", Style::default().fg(theme.accent)), // 📍
        Span::styled(line_range, Style::default().fg(theme.accent)),
    ]));

    let author_label = match comment.author {
        crate::review_store::Author::User => "You",
        crate::review_store::Author::Claude => "Claude",
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!(" by {author_label}"),
            Style::default().fg(theme.info),
        ),
    ]));

    // Separator.
    let sep: String = "\u{2500}".repeat(inner_width.saturating_sub(2));
    lines.push(Line::from(Span::styled(
        format!(" {sep}"),
        Style::default().fg(theme.muted),
    )));

    // Comment body (full, multi-line).
    for body_line in comment.body.split('\n') {
        lines.push(Line::from(Span::styled(
            format!(" {body_line}"),
            Style::default().fg(theme.fg),
        )));
    }

    // Replies section.
    let replies = app.review_state.cached_replies.get(&comment.id);
    if let Some(replies) = replies {
        if !replies.is_empty() {
            lines.push(Line::from(Span::raw("")));
            let reply_sep: String = "\u{2500}".repeat(inner_width.saturating_sub(2));
            lines.push(Line::from(Span::styled(
                format!(" {reply_sep}"),
                Style::default().fg(theme.muted),
            )));
            lines.push(Line::from(Span::styled(
                format!(" \u{1f4ac} Replies ({})", replies.len()), // 💬
                Style::default()
                    .fg(theme.info)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::raw("")));

            for reply in replies {
                let r_author = match reply.author {
                    crate::review_store::Author::User => "You",
                    crate::review_store::Author::Claude => "Claude",
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  \u{21b3} [{r_author}] "),
                        Style::default().fg(theme.info),
                    ),
                ]));
                for reply_line in reply.body.split('\n') {
                    lines.push(Line::from(Span::styled(
                        format!("    {reply_line}"),
                        Style::default().fg(theme.reply_text),
                    )));
                }
                lines.push(Line::from(Span::raw("")));
            }
        }
    }

    // Compute total content height accounting for word-wrap.
    let content_width = inner.width as usize;
    let total_lines: usize = lines
        .iter()
        .map(|line| {
            let line_len: usize = line.spans.iter().map(|s| s.content.len()).sum();
            if content_width > 0 && line_len > content_width {
                line_len.div_ceil(content_width)
            } else {
                1
            }
        })
        .sum();
    let visible_height = inner.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);

    // Store max_scroll and clamp scroll offset.
    app.review_state.comment_detail_max_scroll = max_scroll;
    if app.review_state.comment_detail_scroll > max_scroll {
        app.review_state.comment_detail_scroll = max_scroll;
    }
    let scroll = app.review_state.comment_detail_scroll as u16;

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);

    // Scroll indicator on the bottom border.
    if total_lines > visible_height {
        let current = app.review_state.comment_detail_scroll;
        let indicator = format!(" [{}/{} j/k:scroll] ", current + visible_height.min(total_lines), total_lines);
        let indicator_span = Span::styled(indicator, Style::default().fg(theme.muted));
        let indicator_x = popup_area.x + popup_area.width.saturating_sub(indicator_span.width() as u16 + 2);
        let indicator_y = popup_area.y + popup_area.height - 1;
        if indicator_x > popup_area.x && indicator_y < area.y + area.height {
            frame.render_widget(
                indicator_span,
                Rect::new(indicator_x, indicator_y, popup_area.width.saturating_sub(2), 1),
            );
        }
    }
}
