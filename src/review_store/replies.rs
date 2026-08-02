//! レビューコメントへの返信（review_replies テーブル）の CRUD。

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::{Author, ReviewReply};

impl ReviewStore {
    /// レビューコメントへの返信を挿入する。
    pub fn add_reply(&self, review_id: &str, body: &str, author: Author) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO review_replies (id, review_id, body, author)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, review_id, body, author.as_str()],
        )?;
        Ok(())
    }

    /// 指定したレビューコメントの全返信を作成日時順で返す。
    pub fn get_replies(&self, review_id: &str) -> Result<Vec<ReviewReply>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, body, author, created_at
             FROM review_replies
             WHERE review_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![review_id], |row| {
            let author_str: String = row.get(2)?;
            let author = match author_str.as_str() {
                "user" => Author::User,
                "claude" => Author::Claude,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        format!("unknown Author: {other}").into(),
                    ));
                }
            };
            Ok(ReviewReply {
                id: row.get(0)?,
                body: row.get(1)?,
                author,
                created_at: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// id を指定して返信を1件削除する。親コメントと他の返信はそのまま残る
    /// （全返信をカスケード削除する delete_review とは対照的）。
    pub fn delete_reply(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM review_replies WHERE id = ?1", params![id])?;
        if changed == 0 {
            anyhow::bail!("reply not found: {id}");
        }
        Ok(())
    }

    /// 返信1件の本文テキストを編集する。
    pub fn update_reply_body(&self, id: &str, body: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE review_replies SET body = ?1 WHERE id = ?2",
            params![body, id],
        )?;
        if changed == 0 {
            anyhow::bail!("reply not found: {id}");
        }
        Ok(())
    }

    /// 指定した worktree の全コメントについて返信数を返す。
    ///
    /// review_id から返信数への map を返す。
    pub fn reply_counts_for_worktree(
        &self,
        worktree: &str,
    ) -> Result<std::collections::HashMap<String, usize>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.review_id, COUNT(*)
             FROM review_replies r
             JOIN reviews rv ON rv.id = r.review_id
             WHERE rv.worktree = ?1
             GROUP BY r.review_id",
        )?;
        let rows = stmt.query_map(params![worktree], |row| {
            let review_id: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((review_id, count as usize))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (id, count) = row?;
            map.insert(id, count);
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::CommentKind;
    use super::super::test_support::test_store;
    use super::*;

    #[test]
    fn add_and_get_replies() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/main.rs",
                42,
                None,
                CommentKind::Suggest,
                "fix this",
                "abc",
                Author::Claude,
                None,
            )
            .unwrap();

        // 最初は返信なし。
        let replies = store.get_replies(&review.id).unwrap();
        assert!(replies.is_empty());

        let counts = store.reply_counts_for_worktree("wt1").unwrap();
        assert!(counts.is_empty());

        // ユーザからの返信を追加する。
        store
            .add_reply(&review.id, "I'll fix it", Author::User)
            .unwrap();

        let replies = store.get_replies(&review.id).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body, "I'll fix it");
        assert_eq!(replies[0].author, Author::User);

        // もう1件、Claude からの返信を追加する。
        store
            .add_reply(&review.id, "Thanks!", Author::Claude)
            .unwrap();

        let replies = store.get_replies(&review.id).unwrap();
        assert_eq!(replies.len(), 2);

        // 件数を確認する。
        let counts = store.reply_counts_for_worktree("wt1").unwrap();
        assert_eq!(counts.get(&review.id), Some(&2));

        // 別の worktree には返信がない。
        let counts = store.reply_counts_for_worktree("wt2").unwrap();
        assert!(counts.is_empty());
    }

    #[test]
    fn replies_cascade_delete() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/app.rs",
                10,
                None,
                CommentKind::Question,
                "why?",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        store
            .add_reply(&review.id, "because reasons", Author::Claude)
            .unwrap();
        assert_eq!(store.get_replies(&review.id).unwrap().len(), 1);

        // レビューを削除すると返信もカスケード削除されるはず。
        store.delete_review(&review.id).unwrap();
        let replies = store.get_replies(&review.id).unwrap();
        assert!(replies.is_empty());
    }

    #[test]
    fn delete_reply_removes_only_that_reply_not_the_parent() {
        let store = test_store();
        let review = store
            .add_review(
                "wt1",
                "src/app.rs",
                10,
                None,
                CommentKind::Question,
                "why?",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        store.add_reply(&review.id, "first", Author::Claude).unwrap();
        store.add_reply(&review.id, "second", Author::User).unwrap();

        let replies = store.get_replies(&review.id).unwrap();
        assert_eq!(replies.len(), 2);

        // 最初の返信だけを削除する。
        store.delete_reply(&replies[0].id).unwrap();
        let after = store.get_replies(&review.id).unwrap();
        assert_eq!(after.len(), 1, "only the targeted reply should be removed");
        assert_eq!(after[0].body, "second");
        // 親コメントはまだ存在していなければならない（かつてのバグはこれを消していた）。
        assert!(
            store
                .reviews_for_worktree("wt1")
                .unwrap()
                .iter()
                .any(|c| c.id == review.id)
        );
    }

    #[test]
    fn update_reply_body_edits_only_that_reply() {
        let store = test_store();
        let review = store
            .add_review(
                "wt1",
                "src/app.rs",
                10,
                None,
                CommentKind::Question,
                "why?",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        store.add_reply(&review.id, "typo", Author::User).unwrap();
        let id = store.get_replies(&review.id).unwrap()[0].id.clone();

        store.update_reply_body(&id, "fixed").unwrap();
        assert_eq!(store.get_replies(&review.id).unwrap()[0].body, "fixed");
        // 存在しない返信の編集はサイレントな no-op ではなくエラーになる。
        assert!(store.update_reply_body("nope", "x").is_err());
        assert!(store.delete_reply("nope").is_err());
    }
}
