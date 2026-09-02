//! 行の下に開くコメントスレッド。

use std::collections::HashSet;

use conductor_core::icons::{COMMENT, COMMENT_SPAN, IconSet};
use conductor_core::review_store::{Author, CommentStatus, ReviewComment};
use conductor_core::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::comment_list::kind_glyph;
use crate::review::{ReviewState, anchor_of};

/// スレッドの開閉。既定は「未解決なら開く」で、覚えるのは既定を裏返した行だけ。
///
/// 開いた行を覚える形にすると、コメントを足すたび・解決するたびに集合を継ぎ足す
/// 手当てが要る。既定の反転だけを持てば、新しいコメントは最初から開いている。
#[derive(Debug, Default)]
pub struct ThreadFolds {
    flipped: HashSet<usize>,
}

impl ThreadFolds {
    pub fn clear(&mut self) {
        self.flipped.clear();
    }

    pub fn flip(&mut self, anchor: usize) {
        if !self.flipped.remove(&anchor) {
            self.flipped.insert(anchor);
        }
    }

    pub fn is_open(&self, comments: &[&ReviewComment], anchor: usize) -> bool {
        let unresolved = comments
            .iter()
            .any(|c| anchor_of(c) == anchor && c.status == CommentStatus::Pending);
        unresolved != self.flipped.contains(&anchor)
    }
}

/// ガターの左端 2 桁。
pub fn marker(
    comments: &[&ReviewComment],
    line_1: usize,
    theme: &Theme,
    set: IconSet,
) -> Span<'static> {
    let covering = crate::review::covering(comments, line_1);
    let mut span = None;
    for comment in covering {
        if anchor_of(comment) == line_1 {
            span = Some(COMMENT.get(set));
            break;
        }
        span = Some(COMMENT_SPAN.get(set));
    }
    match span {
        Some(glyph) => Span::styled(format!("{glyph:<2}"), Style::default().fg(theme.accent)),
        None => Span::raw("  "),
    }
}

/// 終端行が anchor のコメントを、返信ごと組む。
pub fn lines(
    review: &ReviewState,
    comments: &[&ReviewComment],
    anchor: usize,
    theme: &Theme,
    set: IconSet,
    width: usize,
    indent: usize,
) -> Vec<Line<'static>> {
    let body_width = width.saturating_sub(indent + 4).max(8);
    let mut out = Vec::new();
    for comment in comments.iter().filter(|c| anchor_of(c) == anchor) {
        let bg = author_bg(comment.author, theme);
        let mut rows = vec![byline(comment, theme, set)];
        rows.extend(wrapped(
            &comment.body,
            body_width,
            Style::default().fg(theme.fg),
        ));
        for reply in review.replies(&comment.id) {
            rows.push(Line::from(Span::styled(
                format!("  \u{21b3} {}", reply.author),
                Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
            )));
            rows.extend(wrapped(
                &reply.body,
                body_width.saturating_sub(2).max(8),
                Style::default().fg(theme.reply_text),
            ));
        }
        out.extend(rows.into_iter().map(|row| frame(row, theme, bg, indent)));
    }
    out
}

/// 署名を読まなくても書き手が分かるように、面の色を変える。
fn author_bg(author: Author, theme: &Theme) -> ratatui::style::Color {
    match author {
        Author::Claude => theme.comment_preview_bg,
        Author::User => theme.comment_user_bg,
    }
}

fn byline(comment: &ReviewComment, theme: &Theme, set: IconSet) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{} {}", kind_glyph(comment.kind, set), comment.author),
        Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
    )];
    if comment.status == CommentStatus::Resolved {
        spans.push(Span::styled(
            "  \u{2713} resolved",
            Style::default().fg(theme.success),
        ));
    }
    Line::from(spans)
}

fn frame(
    row: Line<'static>,
    theme: &Theme,
    bg: ratatui::style::Color,
    indent: usize,
) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" ".repeat(indent)),
        Span::styled("\u{2502} ", Style::default().fg(theme.accent)),
    ];
    spans.extend(row.spans);
    Line::from(spans).style(Style::default().bg(bg))
}

/// 文字数で折り返す。空行も 1 行として残す。
fn wrapped(body: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    body.lines()
        .flat_map(|line| {
            let chars: Vec<char> = line.chars().collect();
            if chars.is_empty() {
                return vec![Line::from(Span::styled(String::new(), style))];
            }
            chars
                .chunks(width)
                .map(|chunk| Line::from(Span::styled(chunk.iter().collect::<String>(), style)))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::tests::comment as fixture;

    fn refs(comments: &[ReviewComment]) -> Vec<&ReviewComment> {
        comments.iter().collect()
    }

    #[test]
    fn 未解決は既定で開き解決済みは閉じspaceが両方を裏返す() {
        let mut pending = fixture("p", "a.rs", 3, None);
        let mut resolved = fixture("r", "a.rs", 9, None);
        resolved.status = CommentStatus::Resolved;
        pending.status = CommentStatus::Pending;
        let comments = vec![pending, resolved];
        let all = refs(&comments);

        let mut folds = ThreadFolds::default();
        assert!(folds.is_open(&all, 3));
        assert!(!folds.is_open(&all, 9));

        folds.flip(3);
        folds.flip(9);
        assert!(!folds.is_open(&all, 3));
        assert!(folds.is_open(&all, 9));

        folds.clear();
        assert!(folds.is_open(&all, 3), "ファイルを開き直すと既定へ戻る");
    }

    #[test]
    fn ガターの印は終端行と範囲の途中を見分ける() {
        let comments = vec![fixture("a", "a.rs", 4, Some(6))];
        let all = refs(&comments);
        let theme = Theme::default();
        let text = |line| {
            marker(&all, line, &theme, IconSet::Unicode)
                .content
                .to_string()
        };
        assert_eq!(text(3), "  ");
        assert_eq!(
            text(4),
            format!("{:<2}", COMMENT_SPAN.get(IconSet::Unicode))
        );
        assert_eq!(text(6), format!("{:<2}", COMMENT.get(IconSet::Unicode)));
    }

    #[test]
    fn スレッドは本文と返信を終端行の下にまとめる() {
        let comments = vec![fixture("a", "a.rs", 1, None), fixture("b", "a.rs", 5, None)];
        let mut review = ReviewState::default();
        review.install(Ok(crate::review::Snapshot {
            branch: "main".into(),
            comments: comments.clone(),
            replies: [(
                "a".to_string(),
                vec![conductor_core::review_store::ReviewReply {
                    id: "r1".into(),
                    body: "looks good".into(),
                    author: Author::Claude,
                    created_at: "now".into(),
                }],
            )]
            .into_iter()
            .collect(),
            ..crate::review::Snapshot::default()
        }));
        let all = review.for_file("a.rs");
        let rendered = |anchor| {
            lines(
                &review,
                &all,
                anchor,
                &Theme::default(),
                IconSet::Unicode,
                60,
                4,
            )
            .iter()
            .map(ratatui::text::Line::to_string)
            .collect::<Vec<_>>()
        };

        let first = rendered(1);
        assert!(first.iter().any(|l| l.contains("body of a")));
        assert!(first.iter().any(|l| l.contains("looks good")));
        assert!(
            !first.iter().any(|l| l.contains("body of b")),
            "他の行の分は出ない"
        );
        assert_eq!(rendered(9), Vec::<String>::new());
    }
}
