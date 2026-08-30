//! レビューコメント一覧の描画。
//!
//! 選択行の flatten は [Row::into_line] が「選択行は全 segment を decoration
//! スタイルに落とす」という一般規則としてすでに持っているので、ここでは
//! 分岐せずに任せる。

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem};

use crate::explorer::ctx::{Ctx, Paint};
use crate::explorer::state::{Explorer, Pane};
use crate::review_state::CommentListRow;
use crate::review_store::{Author, CommentKind, CommentStatus};
use crate::widget::list::Viewport;
use crate::widget::row::{Row, Segment};

/// 一覧全体に対する一括送信ボタンのラベル。
const ASK_CLAUDE_ALL_LABEL: &str = " \u{2728} Ask Claude All ";

/// [ASK_CLAUDE_ALL_LABEL] が右下枠に占める列。幅は [crate::explorer::pointer::ASK_CLAUDE_W]
/// をクリック側と共有する — 描画とクリックがそれぞれ幅を仮定すると、
/// ラベルを変えたときに片方だけ直し忘れる。
pub(crate) fn ask_claude_all_cols(x: u16, width: u16) -> Range<u16> {
    let end = x + width.saturating_sub(1);
    let start = end.saturating_sub(crate::explorer::pointer::ASK_CLAUDE_W);
    start..end
}

/// コメント一覧を描画する（下部ペインの Comments ビュー、または C オーバーレイ）。
pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    view: Viewport,
    ex: &Explorer,
    ctx: &Ctx,
    paint: &Paint,
) {
    let theme = ctx.theme;
    let icon_set = ctx.config.ui.icon_set();
    let list_focused = ctx.focused && ex.focus() == Pane::Bottom;

    let total = ctx.review.comments.len();
    let pending = ctx
        .review
        .comments
        .iter()
        .filter(|c| c.status == CommentStatus::Pending)
        .count();
    let title = format!(
        " {}Comments ({pending}/{total}) ",
        crate::icons::PANEL_COMMENTS.labeled(icon_set)
    );
    let title_style = if list_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };

    let inner_height = view.height;
    let total_rows = ctx.review.comment_list_rows.len();
    let scroll = ex.comments_cursor.scroll();

    let mut block = crate::ui::common::PanelChrome::new(theme, title, ctx.focused, paint.border)
        .with_title_style(title_style)
        .into_block()
        .title_bottom(
            Line::from(vec![Span::styled(
                ASK_CLAUDE_ALL_LABEL,
                Style::default().fg(Color::Rgb(180, 140, 255)),
            )])
            .alignment(Alignment::Right),
        );
    if total_rows > inner_height {
        let first = scroll + 1;
        let last = (scroll + inner_height).min(total_rows);
        block = block.title_bottom(Line::from(Span::styled(
            format!(" {first}-{last}/{total_rows} "),
            Style::default().fg(theme.muted),
        )));
    }

    let selected = ex.comments_cursor.selected();
    let range = ex.comments_cursor.visible(total_rows, view);
    let rows = ctx
        .review
        .comment_list_rows
        .get(range.clone())
        .unwrap_or(&[]);

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .filter_map(|(offset, row)| {
            let row_idx = range.start + offset;
            let selected = row_idx == selected;
            match *row {
                CommentListRow::Comment { comment_idx } => comment_row(
                    comment_idx,
                    selected,
                    list_focused,
                    ctx,
                    theme,
                    icon_set,
                    area.width,
                ),
                CommentListRow::Reply {
                    comment_idx,
                    reply_idx,
                } => reply_row(
                    comment_idx,
                    reply_idx,
                    selected,
                    list_focused,
                    ctx,
                    theme,
                    area.width,
                ),
            }
        })
        .collect();

    frame.render_widget(Clear, area);
    frame.render_widget(List::new(items).block(block), area);
}

/// トップレベルのコメント行を組み立てる。存在しない comment_idx（ティックのずれ）
/// では None を返し、その行を静かに省く — 上のファイルツリー/Changed files と同じ、
/// インデックスの古さに対する扱い。
#[allow(clippy::too_many_arguments)]
fn comment_row(
    comment_idx: usize,
    selected: bool,
    list_focused: bool,
    ctx: &Ctx,
    theme: &crate::theme::Theme,
    icon_set: crate::icons::IconSet,
    area_width: u16,
) -> Option<ListItem<'static>> {
    let comment = ctx.review.comments.get(comment_idx)?;
    let resolved = comment.status == CommentStatus::Resolved;
    let status_marker = if resolved { "\u{2713}" } else { "\u{25cb}" };

    let filename = comment
        .file_path
        .rsplit('/')
        .next()
        .unwrap_or(&comment.file_path);
    let line_range = match comment.line_end {
        Some(end) => format!("L{}-{}", comment.line_start, end),
        None => format!("L{}", comment.line_start),
    };
    let location = format!("{filename}:{line_range}");

    let reply_count = ctx
        .review
        .reply_counts
        .get(&comment.id)
        .copied()
        .unwrap_or(0);
    // 返信数は行末に置き、目が追う場所（位置情報と本文）の邪魔にならないようにする。
    let reply_suffix = if reply_count > 0 {
        format!(" \u{21a9}{reply_count}")
    } else {
        String::new()
    };
    // 展開インジケータ（返信がある場合のみ意味を持つ）。
    let expand_indicator = if reply_count > 0 {
        let expanded = ctx.review.expanded_comments.contains(&comment.id);
        format!("{} ", crate::icons::expand_arrow(expanded, icon_set))
    } else {
        "  ".to_string()
    };

    // 本文は最初の行のみ表示する。改行をスペースに潰すとコメントに構造が
    // あったことが分からなくなるため、+N で残りの行数を示す。
    let kind_glyph = crate::ui::review::kind_icon(comment.kind, icon_set);
    let first_line = comment.body.lines().next().unwrap_or("");
    let extra_lines = comment.body.lines().count().saturating_sub(1);
    let more_suffix = if extra_lines > 0 {
        format!(" +{extra_lines}")
    } else {
        String::new()
    };
    let fixed = format!("{expand_indicator}{status_marker} {kind_glyph} {location} ");
    let max_body = (area_width as usize).saturating_sub(
        unicode_width::UnicodeWidthStr::width(fixed.as_str())
            + unicode_width::UnicodeWidthStr::width(more_suffix.as_str())
            + unicode_width::UnicodeWidthStr::width(reply_suffix.as_str())
            + 2, // ブロックの枠線分
    );
    let body: String = first_line.chars().take(max_body).collect();

    // 解決済みはマーカーも本文も後退させる。ミュートな本文の上に明るい ✓ が
    // 乗ると、もう注意の要らない行にこそ目が引き寄せられるため。
    let marker_color = if resolved { theme.muted } else { theme.warning };
    let body_color = if resolved { theme.muted } else { theme.fg };
    let kind_color = match comment.kind {
        CommentKind::Suggest => theme.success,
        CommentKind::Question => theme.info,
    };

    let line = Row::new(body, body_color)
        .lead([
            Segment::plain(expand_indicator),
            Segment::colored(status_marker, marker_color),
            Segment::plain(" "),
            Segment::colored(kind_glyph, kind_color),
            Segment::plain(" "),
            Segment::colored(location, theme.muted),
            Segment::plain(" "),
        ])
        .trail([Segment::plain(more_suffix), Segment::plain(reply_suffix)])
        .into_line(theme, selected, list_focused, None);
    Some(ListItem::new(line))
}

/// 返信行を組み立てる。
fn reply_row(
    comment_idx: usize,
    reply_idx: usize,
    selected: bool,
    list_focused: bool,
    ctx: &Ctx,
    theme: &crate::theme::Theme,
    area_width: u16,
) -> Option<ListItem<'static>> {
    let comment = ctx.review.comments.get(comment_idx)?;
    let replies = ctx.review.cached_replies.get(&comment.id)?;
    let reply = replies.get(reply_idx)?;

    let author_label = match reply.author {
        Author::User => "You",
        Author::Claude => "Claude",
    };
    let prefix = format!("    \u{21b3} {author_label} ");
    let max_body = (area_width as usize)
        .saturating_sub(unicode_width::UnicodeWidthStr::width(prefix.as_str()) + 2);
    let first_line = reply.body.lines().next().unwrap_or("");
    let extra_lines = reply.body.lines().count().saturating_sub(1);
    let more_suffix = if extra_lines > 0 {
        format!(" +{extra_lines}")
    } else {
        String::new()
    };
    let body: String = first_line.chars().take(max_body).collect();

    let line = Row::new(body, theme.reply_text)
        .lead([Segment::colored(prefix, theme.info).bold()])
        .trail([Segment::colored(more_suffix, theme.muted)])
        .into_line(theme, selected, list_focused, None);
    Some(ListItem::new(line))
}
