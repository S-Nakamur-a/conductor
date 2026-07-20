//! Inline review-comment thread rendering: the expanded thread box shown
//! under a commented line, replies, per-comment action row, and the
//! new-comment compose box.

use crate::app::App;
use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::thread_actions;

/// Build inline thread rows for a comment at the given line.
///
/// Returns a vec of `Line`s representing the thread box:
/// top border, each comment + replies + action icons, bottom border.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_inline_thread_lines<'a>(
    line_1: usize,
    gutter_width: usize,
    panel_width: usize,
    review_state: &crate::review_state::ReviewState,
    reply_comment_id: Option<&str>,
    reply_buffer: &crate::text_input::TextInput,
    theme: &Theme,
    syntax_set: &syntect::parsing::SyntaxSet,
    syntect_theme: &syntect::highlighting::Theme,
    md_cache: &crate::ui::markdown::MarkdownCache,
) -> Vec<(Line<'a>, crate::viewer::ScreenRow)> {
    // An expanded thread shows ALL its comments, resolved included. Resolved
    // ones are merely collapsed *by default* (see `expand_threads_for_file`),
    // so once the user clicks the badge open they must be visible (with a
    // "resolved" marker in the byline) — otherwise the box renders empty.
    let comments: Vec<&crate::review_store::ReviewComment> =
        match review_state.file_comments.get(&line_1) {
            Some(c) if !c.is_empty() => c.iter().collect(),
            _ => return Vec::new(),
        };

    use crate::viewer::ScreenRow;
    let mut out: Vec<(Line, ScreenRow)> = Vec::new();
    let left_pad = crate::viewer::COMMENT_MARKER_W as usize + gutter_width + 4 + 2; // marker + gutter + badge
    let gutter_pad: String = " ".repeat(left_pad);
    let border_style = Style::default().fg(theme.accent);
    // Per-author surface tint so "who wrote this" reads at a glance: Claude's
    // comments/replies on the neutral surface, the user's on a distinct one.
    let author_bg = |a: crate::review_store::Author| match a {
        crate::review_store::Author::Claude => theme.comment_preview_bg,
        crate::review_store::Author::User => theme.comment_user_bg,
    };
    // Box inner width (between │ and │).
    let box_inner = panel_width.saturating_sub(left_pad + 4 + 2); // "  │ " left + " │" right
    // Indent inside the box, but never wider than the box itself (a fixed
    // floor of 20 used to overflow the border on narrow panels).
    let wrap_width = box_inner.saturating_sub(6).max(10).min(box_inner.max(1));

    // Helper: bordered content line with left │, filled to full width in `bg`.
    let make_line = |spans: Vec<Span<'a>>, bg: Color| -> (Line<'a>, ScreenRow) {
        let bg_style = Style::default().bg(bg);
        let mut all = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled("  │ ", border_style),
        ];
        all.extend(spans);
        // Pad the line to panel_width so the background color fills the entire row.
        let used: usize = all
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let remaining = panel_width.saturating_sub(used + 2); // +2 for block borders
        if remaining > 0 {
            all.push(Span::styled(" ".repeat(remaining), bg_style));
        }
        (Line::from(all).style(bg_style), ScreenRow::ThreadContent)
    };

    // Helper: full-width border line, filled in `bg`.
    let make_border = |content: String, bg: Color| -> (Line<'a>, ScreenRow) {
        let bg_style = Style::default().bg(bg);
        let text = format!("{gutter_pad}{content}");
        let used = unicode_width::UnicodeWidthStr::width(text.as_str());
        let pad = panel_width.saturating_sub(used + 2);
        let mut spans = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled(content, border_style),
        ];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg_style));
        }
        (Line::from(spans).style(bg_style), ScreenRow::ThreadContent)
    };

    // Top border — tinted to the first comment's author.
    let top_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(
        format!("  ┌{top_fill}┐"),
        author_bg(comments[0].author),
    ));

    for (ci, comment) in comments.iter().enumerate() {
        // This comment's author surface; all of its rows use it.
        let cbg = author_bg(comment.author);
        let content_style = Style::default().fg(theme.fg).bg(cbg);
        let info_style = Style::default().fg(theme.info).bg(cbg);

        // Blank spacer line between comments in the same thread.
        if ci > 0 {
            out.push(make_line(vec![Span::styled("", content_style)], cbg));
        }

        let author_label = match comment.author {
            crate::review_store::Author::User => "you",
            crate::review_store::Author::Claude => "claude",
        };

        // Author byline: kind badge (💡/❓) + author, like a GitHub comment
        // header. A muted "✓ resolved" marker trails the byline for resolved
        // comments (these only appear when the thread is explicitly opened).
        let kind = crate::ui::review::kind_icon(comment.kind);
        let mut byline = vec![
            Span::styled(format!("{kind} "), content_style),
            Span::styled(
                author_label.to_string(),
                info_style.add_modifier(Modifier::BOLD),
            ),
        ];
        if comment.status == crate::review_store::CommentStatus::Resolved {
            byline.push(Span::styled(
                "  \u{2713} resolved".to_string(),
                Style::default().fg(theme.success).bg(cbg),
            ));
        }
        out.push(make_line(byline, cbg));

        // Comment body, rendered as GitHub-style Markdown onto the author
        // surface (headings, lists, fenced code cards, inline `code`, links, …).
        // Cached per comment id so it isn't re-parsed/highlighted every frame.
        let mut body_md =
            md_cache.render(&comment.id, &comment.body, wrap_width, theme, syntax_set, syntect_theme);
        crate::ui::markdown::apply_background(&mut body_md, cbg);
        for line in body_md {
            out.push(make_line(line.spans, cbg));
        }

        // Show replies if cached — each tinted to ITS OWN author, so a user
        // reply under a Claude comment (or vice-versa) is visibly distinct.
        if let Some(replies) = review_state.cached_replies.get(&comment.id) {
            for reply in replies {
                let rbg = author_bg(reply.author);
                let r_content = Style::default().fg(theme.fg).bg(rbg);
                let r_info = Style::default().fg(theme.info).bg(rbg);
                let reply_author = match reply.author {
                    crate::review_store::Author::User => "you",
                    crate::review_store::Author::Claude => "claude",
                };
                // Reply byline, indented under its parent with a ↳ marker.
                out.push(make_line(
                    vec![Span::styled(
                        format!("  \u{21b3} {reply_author}"),
                        r_info.add_modifier(Modifier::BOLD),
                    )],
                    rbg,
                ));
                // Reply body Markdown, indented two columns under the byline.
                // Cached per reply id.
                let mut reply_md = md_cache.render(
                    &reply.id,
                    &reply.body,
                    wrap_width.saturating_sub(2).max(1),
                    theme,
                    syntax_set,
                    syntect_theme,
                );
                crate::ui::markdown::apply_background(&mut reply_md, rbg);
                for line in reply_md {
                    let mut spans = vec![Span::styled("  ".to_string(), r_content)];
                    spans.extend(line.spans);
                    out.push(make_line(spans, rbg));
                }
            }
        }

        // Per-comment action icons row or active reply input.
        let is_replying_to_this = reply_comment_id == Some(comment.id.as_str());
        let action_row_type = ScreenRow::ThreadActions {
            comment_id: comment.id.clone(),
        };
        if is_replying_to_this {
            // GitHub-style multi-line reply form: a byline, the buffer rendered
            // line by line with a block cursor, then a key hint. The thread above
            // stays visible, so the parent comment is always in view while typing.
            let muted = Style::default().fg(theme.muted).bg(cbg);
            out.push(make_line(
                vec![Span::styled(
                    "\u{21b3} reply".to_string(),
                    info_style.add_modifier(Modifier::BOLD),
                )],
                cbg,
            ));
            if reply_buffer.is_empty() {
                out.push(make_line(
                    vec![
                        Span::styled("> ".to_string(), Style::default().fg(theme.accent).bg(cbg)),
                        Span::styled("Type reply\u{2026}".to_string(), muted),
                    ],
                    cbg,
                ));
            } else {
                // Block cursor sits between before/after text, like the modal.
                let display = format!(
                    "{}\u{2588}{}",
                    reply_buffer.text_before_cursor(),
                    reply_buffer.text_after_cursor()
                );
                for (li, seg) in display.split('\n').enumerate() {
                    let prefix = if li == 0 { "> " } else { "  " };
                    out.push(make_line(
                        vec![
                            Span::styled(
                                prefix.to_string(),
                                Style::default().fg(theme.accent).bg(cbg),
                            ),
                            Span::styled(seg.to_string(), content_style),
                        ],
                        cbg,
                    ));
                }
            }
            out.push(make_line(
                vec![Span::styled(
                    "Shift+Enter: newline  \u{b7}  Enter: send  \u{b7}  Esc: cancel".to_string(),
                    muted,
                )],
                cbg,
            ));
        } else {
            // Clickable action row. Labels and hit ranges both come from the
            // shared `thread_actions` module so the mouse handler stays in
            // sync with what is drawn here.
            let bg_style = Style::default().bg(cbg);
            let muted_style = Style::default().fg(theme.muted).bg(cbg);
            let reply_style = Style::default().fg(theme.info).bg(cbg);
            let resolve_style = Style::default().fg(theme.success).bg(cbg);
            let delete_style = Style::default().fg(theme.error).bg(cbg);
            let claude_style = Style::default().fg(Color::Rgb(180, 140, 255)).bg(cbg);
            let status_label = match comment.status {
                crate::review_store::CommentStatus::Pending => thread_actions::RESOLVE,
                crate::review_store::CommentStatus::Resolved => thread_actions::UNRESOLVE,
            };
            // Pad the status slot to a constant width so "delete" starts at a
            // stable column regardless of resolve/unresolve being shown.
            let status_pad = thread_actions::status_slot_width()
                .saturating_sub(unicode_width::UnicodeWidthStr::width(status_label));

            let gap = " ".repeat(thread_actions::GAP);
            let left_actions = vec![
                Span::styled(thread_actions::REPLY, reply_style),
                Span::styled(gap.clone(), muted_style),
                Span::styled(
                    format!("{status_label}{}", " ".repeat(status_pad)),
                    resolve_style,
                ),
                Span::styled(gap, muted_style),
                Span::styled(thread_actions::DELETE, delete_style),
            ];
            let right_label = thread_actions::ASK_CLAUDE;
            let right_label_w = thread_actions::ask_claude_width();

            let left_w: usize = left_actions
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let prefix_w = left_pad + 4; // gutter_pad + "  │ "
            let fill = panel_width.saturating_sub(prefix_w + left_w + right_label_w + 2 + 1);

            let mut spans = vec![
                Span::styled(gutter_pad.clone(), bg_style),
                Span::styled("  │ ", border_style),
            ];
            spans.extend(left_actions);
            if fill > 0 {
                spans.push(Span::styled(" ".repeat(fill), bg_style));
            }
            spans.push(Span::styled(right_label.to_string(), claude_style));
            spans.push(Span::styled(" ", bg_style)); // trailing pad

            let line = Line::from(spans).style(bg_style);
            out.push((line, action_row_type));
        }
    }

    // Bottom border — tinted to the last comment's author.
    let bot_fill = "─".repeat(box_inner.saturating_sub(1));
    out.push(make_border(
        format!("  └{bot_fill}┘"),
        author_bg(comments[comments.len() - 1].author),
    ));

    out
}

/// The line under which the new-comment compose box should render, if a new
/// comment is being composed and its anchor is in the current file. Returns the
/// end line of the anchored range (where the box is injected).
pub(super) fn new_comment_anchor_end(app: &App) -> Option<usize> {
    if app.review_state.input_mode != crate::review_state::ReviewInputMode::AddingComment {
        return None;
    }
    let (file, start, end) = app.review_state.input_anchor.as_ref()?;
    if Some(file.as_str()) != app.viewer_state.content.current_file.as_deref() {
        return None;
    }
    Some(end.unwrap_or(*start) as usize)
}

/// Build the inline **new-comment** compose box, injected under the anchored
/// line when `ReviewInputMode::AddingComment` is active. A GitHub-style form:
/// a kind header, the body buffer with a block cursor, and a key hint — drawn
/// on the user surface (`comment_user_bg`) like a user-authored comment.
pub(super) fn build_inline_compose_lines<'a>(
    kind: crate::review_store::CommentKind,
    input: &crate::text_input::TextInput,
    gutter_width: usize,
    panel_width: usize,
    theme: &Theme,
) -> Vec<(Line<'a>, crate::viewer::ScreenRow)> {
    use crate::viewer::ScreenRow;
    let bg = theme.comment_user_bg;
    let left_pad = crate::viewer::COMMENT_MARKER_W as usize + gutter_width + 4 + 2;
    let gutter_pad: String = " ".repeat(left_pad);
    let border_style = Style::default().fg(theme.accent);
    let bg_style = Style::default().bg(bg);
    let content_style = Style::default().fg(theme.fg).bg(bg);
    let muted = Style::default().fg(theme.muted).bg(bg);
    let accent_bg = Style::default().fg(theme.accent).bg(bg);
    let box_inner = panel_width.saturating_sub(left_pad + 4 + 2);

    let make_line = |spans: Vec<Span<'a>>| -> (Line<'a>, ScreenRow) {
        let mut all = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled("  │ ", border_style),
        ];
        all.extend(spans);
        let used: usize = all
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let rem = panel_width.saturating_sub(used + 2);
        if rem > 0 {
            all.push(Span::styled(" ".repeat(rem), bg_style));
        }
        (Line::from(all).style(bg_style), ScreenRow::ThreadContent)
    };
    let make_border = |content: String| -> (Line<'a>, ScreenRow) {
        let used = unicode_width::UnicodeWidthStr::width(format!("{gutter_pad}{content}").as_str());
        let pad = panel_width.saturating_sub(used + 2);
        let mut spans = vec![
            Span::styled(gutter_pad.clone(), bg_style),
            Span::styled(content, border_style),
        ];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg_style));
        }
        (Line::from(spans).style(bg_style), ScreenRow::ThreadContent)
    };

    let mut out = Vec::new();
    let top_fill = "\u{2500}".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  \u{250c}{top_fill}\u{2510}")));

    // Kind header (toggle with Tab).
    let (icon, label) = match kind {
        crate::review_store::CommentKind::Suggest => ("\u{1f4a1}", "New Suggest"),
        crate::review_store::CommentKind::Question => ("\u{2753}", "New Question"),
    };
    out.push(make_line(vec![
        Span::styled(format!("{icon} "), content_style),
        Span::styled(
            label.to_string(),
            Style::default().fg(theme.info).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (Tab: toggle kind)".to_string(), muted),
    ]));

    // Body with a block cursor between before/after text.
    if input.is_empty() {
        out.push(make_line(vec![
            Span::styled("> ".to_string(), accent_bg),
            Span::styled("Write a comment\u{2026}".to_string(), muted),
        ]));
    } else {
        let display = format!(
            "{}\u{2588}{}",
            input.text_before_cursor(),
            input.text_after_cursor()
        );
        for (li, seg) in display.split('\n').enumerate() {
            let prefix = if li == 0 { "> " } else { "  " };
            out.push(make_line(vec![
                Span::styled(prefix.to_string(), accent_bg),
                Span::styled(seg.to_string(), content_style),
            ]));
        }
    }

    out.push(make_line(vec![Span::styled(
        "Shift+Enter: newline  \u{b7}  Enter: submit  \u{b7}  Esc: cancel".to_string(),
        muted,
    )]));
    let bot_fill = "\u{2500}".repeat(box_inner.saturating_sub(1));
    out.push(make_border(format!("  \u{2514}{bot_fill}\u{2518}")));
    out
}
