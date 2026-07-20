//! CRUD for replies to review comments (the `review_replies` table).

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::{Author, ReviewReply};

impl ReviewStore {
    /// Insert a reply to a review comment.
    pub fn add_reply(&self, review_id: &str, body: &str, author: Author) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO review_replies (id, review_id, body, author)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, review_id, body, author.as_str()],
        )?;
        Ok(())
    }

    /// Return all replies for a given review comment, ordered by creation time.
    pub fn get_replies(&self, review_id: &str) -> Result<Vec<ReviewReply>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, review_id, body, author, created_at
             FROM review_replies
             WHERE review_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![review_id], |row| {
            let author_str: String = row.get(3)?;
            let author = match author_str.as_str() {
                "user" => Author::User,
                "claude" => Author::Claude,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        format!("unknown Author: {other}").into(),
                    ));
                }
            };
            Ok(ReviewReply {
                id: row.get(0)?,
                review_id: row.get(1)?,
                body: row.get(2)?,
                author,
                created_at: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Delete a single reply by id, leaving the parent comment and its other
    /// replies intact. (Contrast with `delete_review`, which cascade-deletes
    /// every reply.)
    pub fn delete_reply(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM review_replies WHERE id = ?1", params![id])?;
        if changed == 0 {
            anyhow::bail!("reply not found: {id}");
        }
        Ok(())
    }

    /// Edit the body text of a single reply.
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

    /// Return reply counts for all comments in a given worktree.
    ///
    /// Returns a map of review_id → reply count.
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

        // Initially no replies.
        let replies = store.get_replies(&review.id).unwrap();
        assert!(replies.is_empty());

        let counts = store.reply_counts_for_worktree("wt1").unwrap();
        assert!(counts.is_empty());

        // Add a user reply.
        store
            .add_reply(&review.id, "I'll fix it", Author::User)
            .unwrap();

        let replies = store.get_replies(&review.id).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body, "I'll fix it");
        assert_eq!(replies[0].author, Author::User);
        assert_eq!(replies[0].review_id, review.id);

        // Add another reply (from Claude).
        store
            .add_reply(&review.id, "Thanks!", Author::Claude)
            .unwrap();

        let replies = store.get_replies(&review.id).unwrap();
        assert_eq!(replies.len(), 2);

        // Check counts.
        let counts = store.reply_counts_for_worktree("wt1").unwrap();
        assert_eq!(counts.get(&review.id), Some(&2));

        // No replies for a different worktree.
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

        // Deleting the review should cascade-delete the replies.
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

        // Delete only the first reply.
        store.delete_reply(&replies[0].id).unwrap();
        let after = store.get_replies(&review.id).unwrap();
        assert_eq!(after.len(), 1, "only the targeted reply should be removed");
        assert_eq!(after[0].body, "second");
        // The parent comment must still exist (the old bug deleted it).
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
        // Editing a non-existent reply is an error, not a silent no-op.
        assert!(store.update_reply_body("nope", "x").is_err());
        assert!(store.delete_reply("nope").is_err());
    }
}
