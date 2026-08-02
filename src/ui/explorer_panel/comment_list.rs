//! レビューコメント一覧の描画。c で切り替えるエクスプローラ下部ペインと、
//! 中央全画面表示の C オーバーレイの両方で使う。

use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};

/// コメント一覧を中央全画面モーダル（C オーバーレイ）として描画する。
/// ブランチ上の全レビューコメントの一覧を表示し、該当箇所へジャンプできる。
/// エクスプローラ下部ペインと同じ描画ロジックを再利用する。
pub fn render_comment_list_overlay(frame: &mut Frame, area: Rect, app: &App) {
    // 下限を area にクランプする。そうしないと極小ターミナルで min > max になり
    // u16::clamp が panic する。
    let w = ((area.width as u32 * 70 / 100) as u16).clamp(24.min(area.width), area.width);
    let h = ((area.height as u32 * 80 / 100) as u16).clamp(6.min(area.height), area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);
    frame.render_widget(ratatui::widgets::Clear, popup);
    render_comment_list(frame, popup, app, true);
}

/// コメント一覧を描画する（c で切り替えたときの下半分）。
pub(super) fn render_comment_list(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
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
    // 一覧全体に対する一括送信ボタン。viewer_panel::thread_actions にある
    // コメント単位の「ask claude」アクションとは別物。
    const ASK_CLAUDE_ALL_LABEL: &str = " ✨ Ask Claude All ";

    // ボーダーの太さは Explorer カラム全体、タイトルの強調はその下半分に
    // フォーカスがあるかで決まる。
    let title_style = if list_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs_explorer.comment_list_scroll;
    let total_rows = app.review_state.comment_list_rows.len();

    let mut block = crate::ui::common::PanelChrome::new(theme, title, panel_focused, border_color)
        .with_title_style(title_style)
        .into_block()
        .title_bottom(
            Line::from(vec![Span::styled(
                ASK_CLAUDE_ALL_LABEL,
                Style::default().fg(Color::Rgb(180, 140, 255)),
            )])
            .alignment(Alignment::Right),
        );

    // スクロール位置インジケータ（一覧が溢れているときのみ）。
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
                    // 返信数は行末に置き、目が追う場所（位置情報と本文）の
                    // 邪魔にならないようにする。
                    let reply_suffix = if reply_count > 0 {
                        format!(" \u{21a9}{reply_count}")
                    } else {
                        String::new()
                    };

                    // 展開インジケータ（返信がある場合のみ意味を持つ）。
                    let expand_indicator = if reply_count > 0 {
                        if app.review_state.expanded_comments.contains(&comment.id) {
                            "\u{25bc} " // ▼
                        } else {
                            "\u{25b6} " // ▶
                        }
                    } else {
                        "  "
                    };

                    // 本文は最初の行のみ表示する。改行をスペースに潰すと
                    // コメントに構造があったことが分からなくなるため、
                    // +N で残りの行数を示す。
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
                            + 2, // ブロックの枠線分
                    );
                    let body: String = first_line.chars().take(max_body).collect();

                    let selected = row_idx == vs_explorer.comment_list_selected;
                    let item = if selected {
                        // 選択中の行は視認性のため一律のハイライトを維持する。
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
                        // 非選択行は意味色を使い、ステータスと位置情報を後退させて
                        // 本文がスキャンの主役になるようにする。解決済みの行は
                        // マーカーも含めて完全に後退させる。ミュートな本文の上に
                        // 明るい ✓ が乗ると、もう注意を払う必要のない行にこそ
                        // 目が引き寄せられてしまうため。
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

                    // "    ↳ " のインデント(6) + author + " "(1) + ブロック枠線(2)。
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
                        // 深いインデントと author の太字表示により、本文を
                        // 読まなくてもスレッド構造が分かるようにする。
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

    // 最後の項目より下の行（またはスクロールや高さ変更後の古い行）に
    // 前フレームの文字が残らないよう、先にクリアする。viewer と同じ
    // スクロール残像対策。
    frame.render_widget(ratatui::widgets::Clear, area);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}
