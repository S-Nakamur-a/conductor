//! コメント一覧。Explorer の下区画と全画面モーダルが同じ実装を共有する。

use std::collections::HashSet;

use conductor_core::icons::{IconSet, KIND_QUESTION, KIND_SUGGEST, PANEL_COMMENTS, expand_arrow};
use conductor_core::keymap::Action;
use conductor_core::review_store::{CommentKind, CommentStatus, ReviewComment};
use conductor_core::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::list::{ListCursor, Viewport, row_line};
use crate::modal::{CommentEditor, Confirm, Modal};
use crate::review::ReviewState;
use crate::task::{ReviewWrite, Task};
use crate::workspace::Focus;

/// 平坦化した 1 行。返信を開いているコメントの下に返信の行が続く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Comment(usize),
    Reply { comment: usize, reply: usize },
}

#[derive(Debug, Default)]
pub struct CommentList {
    cursor: ListCursor,
    /// 返信を開いているコメントの id。
    expanded: HashSet<String>,
    view: Viewport,
}

impl CommentList {
    pub fn cursor(&self) -> ListCursor {
        self.cursor
    }

    pub fn viewport(&self) -> Viewport {
        self.view
    }

    pub fn set_viewport(&mut self, view: Viewport) {
        self.view = view;
    }

    pub fn is_expanded(&self, comment_id: &str) -> bool {
        self.expanded.contains(comment_id)
    }

    pub fn rows(&self, review: &ReviewState) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, comment) in review.comments().iter().enumerate() {
            rows.push(Row::Comment(i));
            if self.expanded.contains(&comment.id) {
                let replies = review.replies(&comment.id).len();
                rows.extend((0..replies).map(|reply| Row::Reply { comment: i, reply }));
            }
        }
        rows
    }

    pub fn selected_row(&self, review: &ReviewState) -> Option<Row> {
        self.rows(review).get(self.cursor.selected()).copied()
    }

    /// カーソルの下にあるコメント。返信の行にいれば、その親。
    pub fn selected_comment<'a>(&self, review: &'a ReviewState) -> Option<&'a ReviewComment> {
        let index = match self.selected_row(review)? {
            Row::Comment(i) | Row::Reply { comment: i, .. } => i,
        };
        review.comments().get(index)
    }

    pub fn clamp(&mut self, review: &ReviewState) {
        self.cursor.clamp(self.rows(review).len(), self.view);
    }

    pub fn scroll(&mut self, delta: isize, review: &ReviewState) {
        self.cursor.pan(delta, self.rows(review).len(), self.view);
    }

    pub fn click(&mut self, y: u16, review: &ReviewState) -> Vec<Effect> {
        let len = self.rows(review).len();
        let Some(row) = self.cursor.index_at(y, len, self.view) else {
            return Vec::new();
        };
        self.cursor.select(row, len, self.view);
        self.reveal(review).into_iter().collect()
    }

    pub fn update(&mut self, action: Action, review: &ReviewState) -> Option<Vec<Effect>> {
        let rows = self.rows(review);
        let len = rows.len();
        match action {
            Action::NavigateDown => self.cursor.step(1, len, self.view),
            Action::NavigateUp => self.cursor.step(-1, len, self.view),
            Action::GoToTop => self.cursor.select(0, len, self.view),
            Action::GoToBottom => self.cursor.select(usize::MAX, len, self.view),
            Action::CollapseOrLeft => self.collapse(&rows, review),
            Action::ExpandOrRight | Action::ViewCommentDetail => self.expand(review),
            Action::Select => return Some(self.reveal(review).into_iter().collect()),
            Action::ToggleResolve => return Some(self.toggle_resolve(review)),
            Action::EditComment => return Some(self.edit(review)),
            Action::ReplyToComment => return Some(self.reply(review)),
            Action::DeleteComment => return Some(self.delete(review)),
            _ => return None,
        }
        Some(Vec::new())
    }

    /// 返信の行から畳むと自分の行が消えるので、先に親へ寄ってから畳む。
    fn collapse(&mut self, rows: &[Row], review: &ReviewState) {
        let Some(&row) = rows.get(self.cursor.selected()) else {
            return;
        };
        if let Row::Reply { comment, .. } = row
            && let Some(at) = rows.iter().position(|r| *r == Row::Comment(comment))
        {
            self.cursor.select(at, rows.len(), self.view);
        }
        if let Some(comment) = self.selected_comment(review) {
            self.expanded.remove(&comment.id);
        }
        self.clamp(review);
    }

    fn expand(&mut self, review: &ReviewState) {
        let Some(comment) = self.selected_comment(review) else {
            return;
        };
        if review.replies(&comment.id).is_empty() {
            return;
        }
        let id = comment.id.clone();
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.clamp(review);
    }

    /// 返信の行からは親の位置へ飛ぶ。
    fn reveal(&self, review: &ReviewState) -> Option<Effect> {
        let comment = self.selected_comment(review)?;
        Some(Effect::OpenFile {
            path: comment.file_path.clone().into(),
            line: Some(comment.line_start as usize),
            diff: None,
            preview: false,
        })
    }

    fn toggle_resolve(&self, review: &ReviewState) -> Vec<Effect> {
        let Some(comment) = self.selected_comment(review) else {
            return Vec::new();
        };
        vec![Effect::Spawn(Task::WriteReview(ReviewWrite::SetStatus {
            id: comment.id.clone(),
            status: flip_status(comment.status),
        }))]
    }

    fn edit(&self, review: &ReviewState) -> Vec<Effect> {
        let Some(row) = self.selected_row(review) else {
            return Vec::new();
        };
        let Some(comment) = self.selected_comment(review) else {
            return Vec::new();
        };
        let editor = match row {
            Row::Comment(_) => CommentEditor::edit_comment(comment),
            Row::Reply { reply, .. } => {
                let Some(reply) = review.replies(&comment.id).get(reply) else {
                    return Vec::new();
                };
                CommentEditor::edit_reply(reply)
            }
        };
        vec![Effect::PushModal(Modal::CommentEditor(editor))]
    }

    fn reply(&self, review: &ReviewState) -> Vec<Effect> {
        let Some(comment) = self.selected_comment(review) else {
            return Vec::new();
        };
        vec![Effect::PushModal(Modal::CommentEditor(
            CommentEditor::reply_to(comment),
        ))]
    }

    fn delete(&self, review: &ReviewState) -> Vec<Effect> {
        let Some(row) = self.selected_row(review) else {
            return Vec::new();
        };
        let Some(comment) = self.selected_comment(review) else {
            return Vec::new();
        };
        let (question, write) = match row {
            Row::Comment(_) => (
                match review.replies(&comment.id).len() {
                    0 => "Delete this comment?".to_string(),
                    n => format!("Delete this comment and its {n} replies?"),
                },
                ReviewWrite::DeleteComment {
                    id: comment.id.clone(),
                },
            ),
            Row::Reply { reply, .. } => {
                let Some(reply) = review.replies(&comment.id).get(reply) else {
                    return Vec::new();
                };
                (
                    "Delete this reply?".to_string(),
                    ReviewWrite::DeleteReply {
                        id: reply.id.clone(),
                    },
                )
            }
        };
        vec![Effect::PushModal(Modal::Confirm(Confirm {
            question,
            on_yes: vec![Effect::Spawn(Task::WriteReview(write))],
        }))]
    }
}

pub fn flip_status(status: CommentStatus) -> CommentStatus {
    match status {
        CommentStatus::Pending => CommentStatus::Resolved,
        CommentStatus::Resolved => CommentStatus::Pending,
    }
}

pub fn kind_glyph(kind: CommentKind, set: IconSet) -> &'static str {
    match kind {
        CommentKind::Suggest => KIND_SUGGEST.get(set),
        CommentKind::Question => KIND_QUESTION.get(set),
    }
}

pub fn title(review: &ReviewState, set: IconSet) -> String {
    format!(
        " {}Comments ({}/{}) ",
        PANEL_COMMENTS.labeled(set),
        review.pending_count(),
        review.comments().len()
    )
}

pub fn lines(
    list: &CommentList,
    review: &ReviewState,
    theme: &Theme,
    set: IconSet,
    height: usize,
    focused: bool,
) -> Vec<Line<'static>> {
    let rows = list.rows(review);
    if rows.is_empty() {
        return vec![Line::styled(
            "  no comments on this branch",
            Style::default().fg(theme.muted),
        )];
    }
    let cursor = list.cursor();
    cursor
        .visible(rows.len(), list.viewport())
        .take(height)
        .filter_map(|row| {
            let spans = match *rows.get(row)? {
                Row::Comment(index) => {
                    comment_spans(review, list, review.comments().get(index)?, theme, set)
                }
                Row::Reply { comment, reply } => {
                    let comment = review.comments().get(comment)?;
                    reply_spans(review.replies(&comment.id).get(reply)?, theme)
                }
            };
            Some(row_line(spans, theme, row == cursor.selected(), focused))
        })
        .collect()
}

fn comment_spans(
    review: &ReviewState,
    list: &CommentList,
    comment: &ReviewComment,
    theme: &Theme,
    set: IconSet,
) -> Vec<Span<'static>> {
    let replies = review.replies(&comment.id).len();
    let arrow = if replies == 0 {
        "  ".to_string()
    } else {
        format!("{} ", expand_arrow(list.is_expanded(&comment.id), set))
    };
    let resolved = comment.status == CommentStatus::Resolved;
    // 解決済みは印も本文も後退させる。ミュートな本文の上に明るい印が乗ると、
    // もう見なくてよい行にこそ目が引き寄せられる。
    let (mark, mark_fg) = match resolved {
        true => ("\u{2713}", theme.muted),
        false => ("\u{25cb}", theme.warning),
    };
    let body_fg = if resolved { theme.muted } else { theme.fg };
    let kind_fg = match comment.kind {
        CommentKind::Suggest => theme.success,
        CommentKind::Question => theme.info,
    };
    let mut spans = vec![
        Span::styled(arrow, Style::default().fg(theme.muted)),
        Span::styled(format!("{mark} "), Style::default().fg(mark_fg)),
        Span::styled(
            format!("{} ", kind_glyph(comment.kind, set)),
            Style::default().fg(if resolved { theme.muted } else { kind_fg }),
        ),
        Span::styled(location(comment), Style::default().fg(theme.muted)),
        Span::styled(first_line(&comment.body), Style::default().fg(body_fg)),
    ];
    if replies > 0 {
        spans.push(Span::styled(
            format!("  \u{21a9}{replies}"),
            Style::default().fg(theme.muted),
        ));
    }
    spans
}

fn reply_spans(
    reply: &conductor_core::review_store::ReviewReply,
    theme: &Theme,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!("    \u{21b3} {} ", reply.author),
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            first_line(&reply.body),
            Style::default().fg(theme.reply_text),
        ),
    ]
}

fn location(comment: &ReviewComment) -> String {
    let name = comment
        .file_path
        .rsplit('/')
        .next()
        .unwrap_or(&comment.file_path);
    match comment.line_end {
        Some(end) if end != comment.line_start => {
            format!("{name}:L{}-{end}  ", comment.line_start)
        }
        _ => format!("{name}:L{}  ", comment.line_start),
    }
}

/// 本文の 1 行目だけ。残りの行数は末尾に出す — 何も出さないと 1 行のコメントと
/// 見分けが付かない。
fn first_line(body: &str) -> String {
    let mut lines = body.lines();
    let head = lines.next().unwrap_or("").to_string();
    match lines.count() {
        0 => head,
        rest => format!("{head} +{rest}"),
    }
}

pub fn open_modal() -> Effect {
    Effect::PushModal(Modal::CommentList(CommentList::default()))
}

/// esc でモーダルを閉じつつ Viewer へ抜けたいので、飛ぶ Effect には Focus を添える。
pub fn jump_effects(mut effects: Vec<Effect>) -> Vec<Effect> {
    if effects.iter().any(|e| matches!(e, Effect::OpenFile { .. })) {
        effects.insert(0, Effect::PopModal);
        effects.push(Effect::Focus(Focus::Viewer));
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::Snapshot;
    use crate::review::tests::comment as fixture;
    use conductor_core::review_store::{Author, ReviewReply};

    fn reply(id: &str, body: &str) -> ReviewReply {
        ReviewReply {
            id: id.into(),
            body: body.into(),
            author: Author::User,
            created_at: "now".into(),
        }
    }

    /// a.rs:1 は返信 2 件、b.rs:7 は解決済みで返信なし。
    fn review() -> ReviewState {
        let mut resolved = fixture("b", "b.rs", 7, None);
        resolved.status = CommentStatus::Resolved;
        let mut state = ReviewState::default();
        state.install(Ok(Snapshot {
            branch: "main".into(),
            comments: vec![fixture("a", "a.rs", 1, Some(3)), resolved],
            replies: [(
                "a".to_string(),
                vec![reply("r1", "ok"), reply("r2", "sure")],
            )]
            .into_iter()
            .collect(),
            ..Snapshot::default()
        }));
        state
    }

    fn list(review: &ReviewState) -> CommentList {
        let mut list = CommentList::default();
        list.set_viewport(Viewport::new(0, 20));
        list.clamp(review);
        list
    }

    #[test]
    fn 返信のあるコメントだけが開き畳むと親へ寄る() {
        let review = review();
        let mut list = list(&review);
        assert_eq!(list.rows(&review), [Row::Comment(0), Row::Comment(1)]);

        list.update(Action::ExpandOrRight, &review);
        assert_eq!(
            list.rows(&review),
            [
                Row::Comment(0),
                Row::Reply {
                    comment: 0,
                    reply: 0
                },
                Row::Reply {
                    comment: 0,
                    reply: 1
                },
                Row::Comment(1),
            ]
        );

        list.update(Action::NavigateDown, &review);
        list.update(Action::NavigateDown, &review);
        list.update(Action::CollapseOrLeft, &review);
        assert_eq!(list.cursor().selected(), 0);
        assert_eq!(list.rows(&review).len(), 2);

        list.update(Action::GoToBottom, &review);
        list.update(Action::ExpandOrRight, &review);
        assert_eq!(list.rows(&review).len(), 2, "返信が無ければ開かない");
    }

    #[test]
    fn 一覧の操作はコメントを名指しした効果になる() {
        let review = review();
        let mut list = list(&review);
        list.update(Action::ExpandOrRight, &review);
        list.update(Action::NavigateDown, &review);

        let jump = list.update(Action::Select, &review).unwrap();
        let [Effect::OpenFile { path, line, .. }] = jump.as_slice() else {
            panic!("{jump:?}");
        };
        assert_eq!(
            (path.to_str(), *line),
            (Some("a.rs"), Some(1)),
            "返信からは親へ"
        );

        let edit = list.update(Action::EditComment, &review).unwrap();
        let [Effect::PushModal(Modal::CommentEditor(editor))] = edit.as_slice() else {
            panic!("{edit:?}");
        };
        assert_eq!(editor.input.text(), "ok", "返信の行では返信を編集する");

        let delete = list.update(Action::DeleteComment, &review).unwrap();
        let [Effect::PushModal(Modal::Confirm(confirm))] = delete.as_slice() else {
            panic!("{delete:?}");
        };
        assert!(confirm.question.contains("reply"), "{}", confirm.question);
    }

    #[test]
    fn コメントの削除は返信の巻き添えを問う() {
        let review = review();
        let list = list(&review);
        let effects = list.delete(&review);
        let [Effect::PushModal(Modal::Confirm(confirm))] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert!(confirm.question.contains('2'), "{}", confirm.question);
    }

    #[test]
    fn 解決の切り替えは今の状態を裏返す() {
        let review = review();
        let mut list = list(&review);
        for (row, want) in [(0, CommentStatus::Resolved), (1, CommentStatus::Pending)] {
            list.update(Action::GoToTop, &review);
            for _ in 0..row {
                list.update(Action::NavigateDown, &review);
            }
            let effects = list.update(Action::ToggleResolve, &review).unwrap();
            let [Effect::Spawn(Task::WriteReview(ReviewWrite::SetStatus { status, .. }))] =
                effects.as_slice()
            else {
                panic!("{effects:?}");
            };
            assert_eq!(*status, want, "row={row}");
        }
    }

    #[test]
    fn 行は場所と返信数を添え解決済みは後退する() {
        let review = review();
        let mut list = list(&review);
        list.update(Action::ExpandOrRight, &review);
        let rendered: Vec<String> = lines(
            &list,
            &review,
            &Theme::default(),
            IconSet::Unicode,
            10,
            true,
        )
        .iter()
        .map(Line::to_string)
        .collect();

        assert!(rendered[0].contains("a.rs:L1-3"), "{:?}", rendered[0]);
        assert!(rendered[0].contains("\u{21a9}2"), "{:?}", rendered[0]);
        assert!(rendered[1].contains("ok"), "返信の行: {:?}", rendered[1]);
        assert!(
            rendered[3].contains('\u{2713}'),
            "解決済み: {:?}",
            rendered[3]
        );
        assert!(!rendered[0].contains('\u{2713}'));
    }

    #[test]
    fn 本文は1行目だけを出し残りの行数を添える() {
        let mut multi = fixture("m", "a.rs", 1, None);
        multi.body = "first\nsecond\nthird".into();
        let mut review = ReviewState::default();
        review.install(Ok(Snapshot {
            comments: vec![multi],
            ..Snapshot::default()
        }));
        let list = list(&review);
        let rendered = lines(
            &list,
            &review,
            &Theme::default(),
            IconSet::Unicode,
            10,
            false,
        );
        let text = rendered[0].to_string();
        assert!(text.contains("first +2"), "{text}");
        assert!(!text.contains("second"), "{text}");
    }

    #[test]
    fn コメントが無ければその旨を1行出す() {
        let review = ReviewState::default();
        let list = list(&review);
        let rendered = lines(
            &list,
            &review,
            &Theme::default(),
            IconSet::Unicode,
            10,
            false,
        );
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].to_string().contains("no comments"));
    }
}
