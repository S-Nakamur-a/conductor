//! レビューのオーバーレイ描画 — 入力ボックス、テンプレート選択、コメント詳細。
//!
//! これらはアクティブなときにメインレイアウトの上にオーバーレイとして描画される。

use crate::app::App;
use crate::review_state::{ReviewInputMode, ReviewState};
use crate::review_store::CommentKind;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub fn kind_icon(kind: CommentKind, set: crate::icons::IconSet) -> &'static str {
    match kind {
        CommentKind::Suggest => crate::icons::KIND_SUGGEST.get(set),
        CommentKind::Question => crate::icons::KIND_QUESTION.get(set),
    }
}

/// コメント種別バッジのスタイル付き Span。
pub fn kind_badge_span(
    kind: CommentKind,
    theme: &Theme,
    set: crate::icons::IconSet,
) -> Span<'static> {
    match kind {
        CommentKind::Suggest => Span::styled(
            format!("{} ", kind_icon(kind, set)),
            Style::default().fg(theme.success),
        ),
        CommentKind::Question => Span::styled(
            format!("{} ", kind_icon(kind, set)),
            Style::default().fg(theme.info),
        ),
    }
}

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
            let icon = kind_icon(app.review_state.input_kind, app.config.ui.icon_set());
            format!(" {icon} New {kind_label} (Tab: toggle | Shift+Enter: newline) ")
        }
        ReviewInputMode::EditingComment => " Edit Comment (Shift+Enter: newline) ".to_string(),
        ReviewInputMode::EditingReply => " Edit Reply (Shift+Enter: newline) ".to_string(),
        ReviewInputMode::ReplyingToComment => {
            " Reply to Comment (Shift+Enter: newline) ".to_string()
        }
        // ConfirmingDelete は専用の y/n オーバーレイを使い、この入力欄は使わない。
        ReviewInputMode::Normal | ReviewInputMode::ConfirmingDelete => unreachable!(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    // 返信時は、親コメントの最初の行のプレビューを表示する。
    if app.review_state.input_mode == ReviewInputMode::ReplyingToComment
        && let Some(parent) = app.review_state.comments.get(app.review_state.selected)
    {
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

    // カーソル位置にブロックカーソルを入れた複数行の表示を組み立てる。
    let buf = &app.review_state.input_buffer;
    let prefix_line_count = lines.len();
    let display = format!(
        "{}\u{2588}{}",
        buf.text_before_cursor(),
        buf.text_after_cursor()
    );
    let input_lines: Vec<Line> = display
        .split('\n')
        .map(|line| {
            Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme.fg),
            ))
        })
        .collect();

    lines.extend(input_lines);

    // 下部のヒント行。
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

    // IME 用にカーソル位置を設定する。
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

pub fn render_delete_confirm_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use crate::review_state::PendingDelete;
    let theme = &app.theme;
    let what = match &app.review_state.pending_delete {
        Some(PendingDelete::Reply { .. }) => "this reply",
        Some(PendingDelete::Comment { .. }) => "this comment and all its replies",
        None => "this item",
    };
    let msg = format!("Delete {what}?");
    let popup_width = (msg.len() as u16 + 8).clamp(28, area.width.saturating_sub(4));
    let popup_height = 5_u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" Confirm delete ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let lines = vec![
        Line::from(Span::styled(msg, Style::default().fg(theme.fg))),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": delete   ", Style::default().fg(theme.muted)),
            Span::styled(
                "n / Esc",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": cancel", Style::default().fg(theme.muted)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn render_template_picker_overlay(
    frame: &mut Frame,
    area: Rect,
    state: &ReviewState,
    theme: &Theme,
    icon_set: crate::icons::IconSet,
) {
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

        let badge = kind_badge_span(tmpl.kind, theme, icon_set);

        let max_body_len = (popup_width as usize).saturating_sub(tmpl.name.chars().count() + 10);
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

pub fn render_comment_detail_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = &app.theme;
    let popup_width = 72_u16.min(area.width.saturating_sub(4));
    let popup_height = area.height.saturating_sub(4).max(10);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let comment = match app
        .review_state
        .comments
        .get(app.review_state.comment_detail_idx)
    {
        Some(c) => c,
        None => return,
    };

    let icon = kind_icon(comment.kind, app.config.ui.icon_set());
    let kind_label = match comment.kind {
        CommentKind::Suggest => "Suggest",
        CommentKind::Question => "Question",
    };
    let status_label = match comment.status {
        crate::review_store::CommentStatus::Pending => "\u{25cb} Pending",
        crate::review_store::CommentStatus::Resolved => "\u{2713} Resolved",
    };

    let title = format!(
        " {icon} {kind_label} \u{2502} {status_label} (Esc/q: close, e: edit, R: reply, r: resolve, Del: delete) "
    );

    // border_focused（info ではない）: レビュー系のモーダル — 入力、テンプレート
    // 選択、詳細 — はすべてフォーカス時のボーダー色を共有し、同じ一族として
    // 見えるようにしている。
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let inner_width = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();

    // 位置情報のヘッダ。
    let line_range = if let Some(end) = comment.line_end {
        format!("{}:{}-{}", comment.file_path, comment.line_start, end)
    } else {
        format!("{}:{}", comment.file_path, comment.line_start)
    };
    // 位置情報と著者は1つのヘッダ行を共有する: 著者は本文ほど重要ではないので、
    // 独立した行を持たせず muted 色に乗せるだけにしている。
    let author_label = match comment.author {
        crate::review_store::Author::User => "You",
        crate::review_store::Author::Claude => "Claude",
    };
    lines.push(Line::from(vec![
        Span::styled(" \u{1f4cd} ", Style::default().fg(theme.accent)), // 📍
        Span::styled(line_range, Style::default().fg(theme.accent)),
        Span::styled(
            format!(" \u{b7} {author_label}"),
            Style::default().fg(theme.muted),
        ),
    ]));

    // 区切り線。
    let sep: String = "\u{2500}".repeat(inner_width.saturating_sub(2));
    lines.push(Line::from(Span::styled(
        format!(" {sep}"),
        Style::default().fg(theme.muted),
    )));

    // コメント本文は GitHub 風の Markdown として描画する（SUMMARY ビューや
    // インラインスレッドボックスと同じレンダラー）: 見出し、リスト、fenced
    // code カード、インラインコード、リンク、テーブル。ポップアップの
    // ボーダーを避けるため1カラム分インデントする。
    let body_md = crate::ui::markdown::render_markdown(
        &comment.body,
        inner_width.saturating_sub(1),
        theme,
        &app.highlight.syntax_set,
        &app.highlight.theme,
    );
    for line in body_md {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }

    // 返信のセクション。
    let replies = app.review_state.cached_replies.get(&comment.id);
    if let Some(replies) = replies
        && !replies.is_empty()
    {
        lines.push(Line::from(Span::raw("")));
        let reply_sep: String = "\u{2500}".repeat(inner_width.saturating_sub(2));
        lines.push(Line::from(Span::styled(
            format!(" {reply_sep}"),
            Style::default().fg(theme.muted),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                " {} Replies ({})",
                crate::icons::COMMENT.get(app.config.ui.icon_set()),
                replies.len()
            ),
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::raw("")));

        for reply in replies {
            let r_author = match reply.author {
                crate::review_store::Author::User => "You",
                crate::review_store::Author::Claude => "Claude",
            };
            lines.push(Line::from(vec![Span::styled(
                format!("  \u{21b3} {r_author}"),
                Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
            )]));
            // 返信本文の Markdown。署名行の下にインデントする。
            let reply_md = crate::ui::markdown::render_markdown(
                &reply.body,
                inner_width.saturating_sub(4),
                theme,
                &app.highlight.syntax_set,
                &app.highlight.theme,
            );
            for line in reply_md {
                let mut spans = vec![Span::raw("    ")];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
            lines.push(Line::from(Span::raw("")));
        }
    }

    // ワードラップを考慮したコンテンツ全体の高さを計算する。
    let content_width = inner.width as usize;
    let total_lines: usize = lines
        .iter()
        .map(|line| {
            // 表示幅を使う（バイト長ではない）— そうしないとマルチバイトの
            // 本文でラップ量を過大評価し、無駄なスクロール範囲が残ってしまう。
            let line_len: usize = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            if content_width > 0 && line_len > content_width {
                line_len.div_ceil(content_width)
            } else {
                1
            }
        })
        .sum();
    let visible_height = inner.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);

    // max_scroll を保存し、スクロールオフセットをクランプする。
    app.review_state.comment_detail_max_scroll = max_scroll;
    if app.review_state.comment_detail_scroll > max_scroll {
        app.review_state.comment_detail_scroll = max_scroll;
    }
    let scroll = app.review_state.comment_detail_scroll as u16;

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);

    // 下部ボーダーのスクロールインジケーター。
    if total_lines > visible_height {
        let current = app.review_state.comment_detail_scroll;
        let indicator = format!(
            " [{}/{} j/k:scroll] ",
            current + visible_height.min(total_lines),
            total_lines
        );
        let indicator_span = Span::styled(indicator, Style::default().fg(theme.muted));
        let indicator_x = popup_area.x
            + popup_area
                .width
                .saturating_sub(indicator_span.width() as u16 + 2);
        let indicator_y = popup_area.y + popup_area.height - 1;
        if indicator_x > popup_area.x && indicator_y < area.y + area.height {
            frame.render_widget(
                indicator_span,
                Rect::new(
                    indicator_x,
                    indicator_y,
                    popup_area.width.saturating_sub(2),
                    1,
                ),
            );
        }
    }
}
