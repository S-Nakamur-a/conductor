//! CRUD and queries for review comments (the `reviews` table).

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::{Author, CommentKind, CommentStatus, ReviewComment};

impl ReviewStore {
    /// Insert a new review comment and return it.
    #[allow(clippy::too_many_arguments)]
    pub fn add_review(
        &self,
        worktree: &str,
        file_path: &str,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: &str,
        commit_ref: &str,
        author: Author,
        branch: Option<&str>,
    ) -> Result<ReviewComment> {
        let id = Uuid::new_v4().to_string();

        self.conn.execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, line_end, kind, body, commit_ref, author, branch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                worktree,
                file_path,
                line_start as i64,
                line_end.map(|n| n as i64),
                kind.as_str(),
                body,
                commit_ref,
                author.as_str(),
                branch,
            ],
        )?;

        // Read back to get the server-side defaults (created_at, updated_at).
        self.get_review(&id)
    }

    /// Fetch a single review by id.
    fn get_review(&self, id: &str) -> Result<ReviewComment> {
        self.conn
            .query_row(
                "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                        commit_ref, author, branch, created_at, updated_at
                 FROM reviews WHERE id = ?1",
                params![id],
                row_to_review,
            )
            .map_err(Into::into)
    }

    /// Edit the body text of a review comment.
    pub fn update_review_body(&self, id: &str, body: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![body, id],
        )?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// Delete a review comment by id.
    pub fn delete_review(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM reviews WHERE id = ?1", params![id])?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// Update the status of a review comment.
    pub fn update_review_status(&self, id: &str, status: CommentStatus) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// Return all reviews for a given worktree, ordered by file then line.
    pub fn reviews_for_worktree(&self, worktree: &str) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    commit_ref, author, branch, created_at, updated_at
             FROM reviews
             WHERE worktree = ?1
             ORDER BY file_path, line_start",
        )?;
        collect_reviews(&mut stmt, params![worktree])
    }

    /// Return reviews for a specific file within a worktree.
    #[allow(dead_code)]
    pub fn reviews_for_file(&self, worktree: &str, file_path: &str) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    commit_ref, author, branch, created_at, updated_at
             FROM reviews
             WHERE worktree = ?1 AND file_path = ?2
             ORDER BY line_start",
        )?;
        collect_reviews(&mut stmt, params![worktree, file_path])
    }

    /// Mark the given review comments as published to GitHub, stamping them
    /// all with the same `timestamp` (one publish batch = one moment in
    /// time). Once set, `unpublished_reviews` no longer returns them, so a
    /// retried publish doesn't repost the same comment.
    pub fn mark_published(&self, comment_ids: &[String], timestamp: &str) -> Result<()> {
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| -> Result<()> {
            for id in comment_ids {
                self.conn.execute(
                    "UPDATE reviews SET published_at = ?1 WHERE id = ?2",
                    params![timestamp, id],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Return a branch's review comments that have not yet been posted to
    /// GitHub (`published_at IS NULL`).
    pub fn unpublished_reviews(&self, branch: &str) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    commit_ref, author, branch, created_at, updated_at
             FROM reviews
             WHERE branch = ?1 AND published_at IS NULL
             ORDER BY file_path, line_start",
        )?;
        collect_reviews(&mut stmt, params![branch])
    }
}

/// Convert a `rusqlite::Row` into a `ReviewComment`.
///
/// Expected column order (13 columns):
///   0:id, 1:worktree, 2:file_path, 3:line_start, 4:line_end,
///   5:kind, 6:body, 7:status, 8:commit_ref, 9:author, 10:branch,
///   11:created_at, 12:updated_at
fn row_to_review(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewComment> {
    let kind_str: String = row.get(5)?;
    let status_str: String = row.get(7)?;
    let author_str: String = row.get(9)?;

    let kind = match kind_str.as_str() {
        "suggest" => CommentKind::Suggest,
        "question" => CommentKind::Question,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                format!("unknown CommentKind: {other}").into(),
            ));
        }
    };

    let status = match status_str.as_str() {
        "pending" => CommentStatus::Pending,
        "resolved" => CommentStatus::Resolved,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                format!("unknown CommentStatus: {other}").into(),
            ));
        }
    };

    let author = match author_str.as_str() {
        "user" => Author::User,
        "claude" => Author::Claude,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                format!("unknown Author: {other}").into(),
            ));
        }
    };

    Ok(ReviewComment {
        id: row.get(0)?,
        worktree: row.get(1)?,
        file_path: row.get(2)?,
        line_start: row.get::<_, i64>(3)? as u32,
        line_end: row.get::<_, Option<i64>>(4)?.map(|n| n as u32),
        kind,
        body: row.get(6)?,
        status,
        commit_ref: row.get(8)?,
        author,
        branch: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// Execute a prepared statement and collect all matching rows into a `Vec<ReviewComment>`.
pub(super) fn collect_reviews(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<ReviewComment>> {
    let rows = stmt.query_map(params, row_to_review)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;
    use super::*;

    #[test]
    fn add_and_retrieve_review() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/main.rs",
                42,
                None,
                CommentKind::Suggest,
                "use guard clause",
                "abc123",
                Author::User,
                None,
            )
            .unwrap();

        assert_eq!(review.worktree, "wt1");
        assert_eq!(review.file_path, "src/main.rs");
        assert_eq!(review.line_start, 42);
        assert_eq!(review.line_end, None);
        assert_eq!(review.kind, CommentKind::Suggest);
        assert_eq!(review.body, "use guard clause");
        assert_eq!(review.status, CommentStatus::Pending);
        assert_eq!(review.commit_ref, "abc123");
        assert_eq!(review.author, Author::User);
        assert_eq!(review.branch, None);

        // Retrieve by worktree
        let reviews = store.reviews_for_worktree("wt1").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].id, review.id);

        // Retrieve by file
        let reviews = store.reviews_for_file("wt1", "src/main.rs").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].id, review.id);

        // No reviews for a different file
        let reviews = store.reviews_for_file("wt1", "src/lib.rs").unwrap();
        assert!(reviews.is_empty());
    }

    #[test]
    fn update_body() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/app.rs",
                5,
                None,
                CommentKind::Suggest,
                "original",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        store.update_review_body(&review.id, "edited").unwrap();
        let reviews = store.reviews_for_worktree("wt1").unwrap();
        assert_eq!(reviews[0].body, "edited");
    }

    #[test]
    fn line_range_and_author() {
        let store = test_store();

        // worktree and branch carry the same branch name (the v4 CHECK enforces
        // this); the comment column stores it in both.
        let review = store
            .add_review(
                "feature/x",
                "src/main.rs",
                10,
                Some(20),
                CommentKind::Suggest,
                "refactor this block",
                "abc",
                Author::Claude,
                Some("feature/x"),
            )
            .unwrap();

        assert_eq!(review.line_start, 10);
        assert_eq!(review.line_end, Some(20));
        assert_eq!(review.author, Author::Claude);
        assert_eq!(review.branch.as_deref(), Some("feature/x"));

        // Single-line (line_end = None)
        let r2 = store
            .add_review(
                "wt1",
                "src/main.rs",
                5,
                None,
                CommentKind::Question,
                "why?",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        assert_eq!(r2.line_start, 5);
        assert_eq!(r2.line_end, None);
        assert_eq!(r2.author, Author::User);
        assert_eq!(r2.branch, None);
    }

    #[test]
    fn mark_published_hides_reviews_from_unpublished_query() {
        let store = test_store();

        let r1 = store
            .add_review(
                "feat/x",
                "src/main.rs",
                1,
                None,
                CommentKind::Suggest,
                "first",
                "abc123",
                Author::User,
                Some("feat/x"),
            )
            .unwrap();
        let r2 = store
            .add_review(
                "feat/x",
                "src/lib.rs",
                2,
                None,
                CommentKind::Question,
                "second",
                "abc123",
                Author::User,
                Some("feat/x"),
            )
            .unwrap();

        let unpublished = store.unpublished_reviews("feat/x").unwrap();
        assert_eq!(unpublished.len(), 2);

        store
            .mark_published(std::slice::from_ref(&r1.id), "2026-07-05T00:00:00Z")
            .unwrap();

        let unpublished = store.unpublished_reviews("feat/x").unwrap();
        assert_eq!(unpublished.len(), 1);
        assert_eq!(unpublished[0].id, r2.id);
    }
}
