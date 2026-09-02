//! ブランチ 1 本ぶんのレビュー。コメント・返信・変更サマリ・viewed をまとめて持つ。
//!
//! Viewer も Explorer もモーダルもこれを読むので、パネルではなく [crate::workspace::Ctx]
//! 経由で配る。書き換えるのは svc から返る [Snapshot] を丸ごと入れ替えるときだけで、
//! 部分更新はしない。MCP が同じ DB を書いている以上、手元の差分を積み上げても
//! 次の再読込で捨てられる。

use std::collections::{HashMap, HashSet};

use conductor_core::review_store::{CommentStatus, ReviewComment, ReviewReply};

/// DB から読んだ 1 ブランチぶんの全て。
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub branch: String,
    /// file_path, line_start 順。
    pub comments: Vec<ReviewComment>,
    pub replies: HashMap<String, Vec<ReviewReply>>,
    pub summary: Option<String>,
    pub viewed: HashSet<String>,
}

#[derive(Debug, Default)]
pub struct ReviewState {
    snapshot: Snapshot,
    pub error: Option<String>,
}

impl ReviewState {
    pub fn branch(&self) -> &str {
        &self.snapshot.branch
    }

    pub fn comments(&self) -> &[ReviewComment] {
        &self.snapshot.comments
    }

    pub fn replies(&self, comment_id: &str) -> &[ReviewReply] {
        self.snapshot
            .replies
            .get(comment_id)
            .map_or(&[], Vec::as_slice)
    }

    pub fn summary(&self) -> Option<&str> {
        self.snapshot.summary.as_deref()
    }

    pub fn is_viewed(&self, path: &str) -> bool {
        self.snapshot.viewed.contains(path)
    }

    pub fn install(&mut self, loaded: Result<Snapshot, String>) {
        match loaded {
            Ok(snapshot) => {
                self.snapshot = snapshot;
                self.error = None;
            }
            // 手元のコメントは消さない。読めなかっただけで、消えたわけではない。
            Err(reason) => self.error = Some(reason),
        }
    }

    /// そのファイルのコメント。行の昇順。
    pub fn for_file(&self, path: &str) -> Vec<&ReviewComment> {
        self.snapshot
            .comments
            .iter()
            .filter(|c| c.file_path == path)
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.snapshot
            .comments
            .iter()
            .filter(|c| c.status == CommentStatus::Pending)
            .count()
    }
}

/// スレッドが描かれる行。範囲コメントは終端にだけ出る。
pub fn anchor_of(comment: &ReviewComment) -> usize {
    comment.line_end.unwrap_or(comment.line_start) as usize
}

/// その行を覆うコメント。範囲コメントは途中の行でも当たる。
pub fn covering<'a>(comments: &[&'a ReviewComment], line_1: usize) -> Vec<&'a ReviewComment> {
    comments
        .iter()
        .copied()
        .filter(|c| {
            let start = c.line_start as usize;
            start <= line_1 && line_1 <= anchor_of(c)
        })
        .collect()
}

/// その行を覆うコメントのうち、最も早く終わるもの。
///
/// 入れ子の範囲では外側だけを名指しする手段が無くなるが、開閉と返信と解決が
/// 揃って同じ 1 件を指すほうが、押した先が読めなくなるより良い。
pub fn innermost<'a>(comments: &[&'a ReviewComment], line_1: usize) -> Option<&'a ReviewComment> {
    covering(comments, line_1)
        .into_iter()
        .min_by_key(|c| anchor_of(c))
}

/// スレッドを開閉するときに実際に動く行。
pub fn anchor_for(comments: &[&ReviewComment], line_1: usize) -> Option<usize> {
    innermost(comments, line_1).map(anchor_of)
}

/// スレッドが描かれる行の集合。
pub fn anchors(comments: &[&ReviewComment]) -> HashSet<usize> {
    comments.iter().copied().map(anchor_of).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use conductor_core::review_store::{Author, CommentKind};

    pub(crate) fn comment(id: &str, file: &str, start: u32, end: Option<u32>) -> ReviewComment {
        ReviewComment {
            id: id.into(),
            worktree: "main".into(),
            file_path: file.into(),
            line_start: start,
            line_end: end,
            kind: CommentKind::Suggest,
            body: format!("body of {id}"),
            status: CommentStatus::Pending,
            author: Author::User,
            branch: Some("main".into()),
            created_at: "2026-01-01".into(),
        }
    }

    fn state(comments: Vec<ReviewComment>) -> ReviewState {
        let mut state = ReviewState::default();
        state.install(Ok(Snapshot {
            branch: "main".into(),
            comments,
            ..Snapshot::default()
        }));
        state
    }

    #[test]
    fn 重なった範囲は共有行で両方に当たり終端は自分だけを持つ() {
        let comments = vec![
            comment("outer", "a.rs", 10, Some(20)),
            comment("inner", "a.rs", 11, Some(19)),
        ];
        let state = state(comments);
        let file = state.for_file("a.rs");

        let ids = |line| {
            covering(&file, line)
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(9), Vec::<&str>::new());
        assert_eq!(ids(10), ["outer"], "境界は外側だけ");
        assert_eq!(ids(15), ["outer", "inner"]);
        assert_eq!(ids(19), ["outer", "inner"]);
        assert_eq!(ids(20), ["outer"]);

        assert_eq!(anchors(&file), HashSet::from([19, 20]));
        assert_eq!(anchor_for(&file, 15), Some(19), "内側の終端へ寄る");
        assert_eq!(innermost(&file, 15).map(|c| c.id.as_str()), Some("inner"));
        assert_eq!(anchor_for(&file, 20), Some(20), "終端行は自分自身");
        assert_eq!(anchor_for(&file, 9), None);
    }

    #[test]
    fn 別のファイルのコメントは混ざらない() {
        let state = state(vec![
            comment("a", "a.rs", 1, None),
            comment("b", "b.rs", 1, None),
        ]);
        assert_eq!(state.for_file("a.rs").len(), 1);
        assert_eq!(state.for_file("c.rs").len(), 0);
    }

    /// 読み込みに失敗しただけでコメントが画面から消えると、DB が一時的にロックされた
    /// だけでレビュー中の指摘が全部消えたように見える。
    #[test]
    fn 読み込みの失敗は手元のコメントを消さない() {
        let mut state = state(vec![comment("a", "a.rs", 1, None)]);
        state.install(Err("locked".into()));
        assert_eq!(state.comments().len(), 1);
        assert_eq!(state.error.as_deref(), Some("locked"));

        state.install(Ok(Snapshot::default()));
        assert!(state.error.is_none());
        assert!(state.comments().is_empty());
    }
}
