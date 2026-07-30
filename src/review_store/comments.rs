//! CRUD and queries for review comments (the `reviews` table).

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::{Author, CommentKind, CommentStatus, ReviewComment};

/// Shortest id prefix [`ReviewStore::resolve_id_prefix`] will match on.
///
/// Comment ids are surfaced to Claude as their first 8 characters, so 8 is both
/// what the MCP tools advertise and what a caller has actually seen.
pub const MIN_ID_PREFIX_LEN: usize = 8;

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
    pub fn get_review(&self, id: &str) -> Result<ReviewComment> {
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

    /// Resolve a full comment id or a unique prefix of one to the full id.
    ///
    /// Shorter than [`MIN_ID_PREFIX_LEN`] is rejected: that is the length the
    /// tools advertise, and it is also how comment ids are printed back, so
    /// anything shorter is a mistake rather than a legitimate shorthand.
    ///
    /// `prefix` must consist only of hex digits and `-` (the alphabet a UUID
    /// is drawn from) or this returns `Ok(None)` without touching the
    /// database — a bare `LIKE` pattern built from an unvalidated prefix
    /// would let `%`/`_` act as SQL wildcards (e.g. `prefix = "%"` matching
    /// any comment), and rejecting that shape up front costs nothing a
    /// legitimate id or id-prefix would ever need. When multiple rows match,
    /// the first by `id` is returned rather than treated as ambiguous — this
    /// mirrors the Node MCP server it replaces, just made deterministic with
    /// an explicit `ORDER BY`.
    pub fn resolve_id_prefix(&self, prefix: &str) -> Result<Option<String>> {
        // The tools advertise "ID or unique prefix (min 8 chars)" — enforce it
        // rather than just documenting it. A one- or two-character prefix
        // matches whichever id happens to sort first, so a model that mistypes
        // an id would resolve or reply to *someone else's* comment and be told
        // it succeeded.
        if prefix.len() < MIN_ID_PREFIX_LEN {
            return Ok(None);
        }
        // Ids are UUIDs, so anything outside hex-and-dashes cannot match one.
        // Rejecting it here also keeps `%` and `_` — LIKE's wildcards — from
        // reaching the pattern below, where they would match unrelated rows.
        if !prefix.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Ok(None);
        }
        let pattern = format!("{prefix}%");
        let result = self.conn.query_row(
            "SELECT id FROM reviews WHERE id LIKE ?1 ORDER BY id LIMIT 1",
            params![pattern],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Return pending review comments, optionally narrowed by branch,
    /// worktree, and/or file path.
    ///
    /// `branch` matches the `branch` column **or** the `worktree` column
    /// (`OR`, both bound to the same value). Under the v4 schema's `CHECK
    /// (branch IS NULL OR worktree = branch)` the `branch = ?` side can never
    /// be the one that makes a match — a non-null `branch` always agrees with
    /// `worktree` already — so the `OR` is redundant against this schema. It
    /// is kept anyway for parity with the Node MCP server this replaces,
    /// which predates that CHECK and could see rows where the two disagreed
    /// (see `docs/spec-s6-mcp-tools.md` §1).
    pub fn pending_reviews(
        &self,
        branch: Option<&str>,
        worktree: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<Vec<ReviewComment>> {
        let mut sql = String::from(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    commit_ref, author, branch, created_at, updated_at
             FROM reviews WHERE status = 'pending'",
        );
        let mut bind: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(w) = worktree {
            sql.push_str(" AND worktree = ?");
            bind.push(Box::new(w.to_string()));
        }
        if let Some(b) = branch {
            sql.push_str(" AND (branch = ? OR worktree = ?)");
            bind.push(Box::new(b.to_string()));
            bind.push(Box::new(b.to_string()));
        }
        if let Some(f) = file_path {
            sql.push_str(" AND file_path = ?");
            bind.push(Box::new(f.to_string()));
        }
        sql.push_str(" ORDER BY file_path, line_start");

        let mut stmt = self.conn.prepare(&sql)?;
        collect_reviews(&mut stmt, rusqlite::params_from_iter(bind.iter()))
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

    #[test]
    fn pending_reviews_filters_by_status() {
        let store = test_store();

        let pending = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "still open",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        let resolved = store
            .add_review(
                "wt1",
                "src/b.rs",
                2,
                None,
                CommentKind::Suggest,
                "done",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        store
            .update_review_status(&resolved.id, CommentStatus::Resolved)
            .unwrap();

        let rows = store.pending_reviews(None, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, pending.id);
    }

    /// Exercises the `(branch = ? OR worktree = ?)` clause. Under the v4
    /// CHECK (`branch IS NULL OR worktree = branch`), `branch = ?` can never
    /// be the side that produces a match on its own — a non-null `branch`
    /// always already agrees with `worktree` — so this test only isolates
    /// the `worktree = ?` half (`via_worktree`, whose `branch` is NULL); the
    /// `via_branch` row happens to also satisfy `worktree = ?`, it does not
    /// prove the `branch = ?` side does anything under this schema. Both
    /// rows must still come back for a `branch` filter of `"feat/x"`.
    #[test]
    fn pending_reviews_matches_branch_or_worktree_column() {
        let store = test_store();

        // The v4 CHECK (`branch IS NULL OR worktree = branch`) forces
        // `worktree` to agree with a non-null `branch`, so this row matches
        // both columns — the row below is what isolates the `worktree`-only
        // path.
        let via_branch = store
            .add_review(
                "feat/x",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "matches via branch",
                "abc",
                Author::User,
                Some("feat/x"),
            )
            .unwrap();
        let via_worktree = store
            .add_review(
                "feat/x",
                "src/b.rs",
                2,
                None,
                CommentKind::Suggest,
                "matches via worktree",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        let rows = store.pending_reviews(Some("feat/x"), None, None).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(rows.len(), 2);
        assert!(ids.contains(&via_branch.id.as_str()));
        assert!(ids.contains(&via_worktree.id.as_str()));
    }

    #[test]
    fn pending_reviews_filters_by_file_path() {
        let store = test_store();

        let a = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "on a.rs",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        store
            .add_review(
                "wt1",
                "src/b.rs",
                2,
                None,
                CommentKind::Suggest,
                "on b.rs",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        let rows = store.pending_reviews(None, None, Some("src/a.rs")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, a.id);
    }

    #[test]
    fn resolve_id_prefix_finds_by_8char_prefix() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        let resolved = store
            .resolve_id_prefix(&review.id[..8])
            .unwrap()
            .expect("prefix should resolve");
        assert_eq!(resolved, review.id);
    }

    #[test]
    fn resolve_id_prefix_returns_none_when_no_match() {
        let store = test_store();
        store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        assert_eq!(store.resolve_id_prefix("deadbeef").unwrap(), None);
    }

    #[test]
    fn resolve_id_prefix_rejects_like_wildcards() {
        let store = test_store();
        store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        // Without the hex/`-` validation this would resolve to whichever
        // comment sorts first by id — a security-relevant escape.
        assert_eq!(store.resolve_id_prefix("%").unwrap(), None);
    }

    #[test]
    fn resolve_id_prefix_is_deterministic_with_multiple_matches() {
        let store = test_store();

        // Hand-crafted ids sharing a prefix — real UUIDs are random, so this
        // is the only way to reliably provoke an ambiguous prefix. Inserted
        // in descending id order on purpose: inserting ascending would let
        // rowid order (SQLite's default without an ORDER BY) coincide with
        // id order and pass even if the `ORDER BY id` were dropped from the
        // query.
        for id in [
            "aaaaaaaa-2222-0000-0000-000000000000",
            "aaaaaaaa-1111-0000-0000-000000000000",
        ] {
            store
                .conn
                .execute(
                    "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, commit_ref)
                     VALUES (?1, 'wt1', 'src/a.rs', 1, 'suggest', 'note', 'abc')",
                    params![id],
                )
                .unwrap();
        }

        let resolved = store.resolve_id_prefix("aaaaaaaa").unwrap().unwrap();
        assert_eq!(resolved, "aaaaaaaa-1111-0000-0000-000000000000");
    }

    #[test]
    fn resolve_id_prefix_rejects_underscore_wildcard() {
        let store = test_store();
        store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        // `_` is `LIKE`'s single-character wildcard — the other one besides
        // `%`, and just as much an escape if a bare LIKE pattern were built
        // from an unvalidated prefix.
        assert_eq!(store.resolve_id_prefix("_").unwrap(), None);
    }

    #[test]
    fn resolve_id_prefix_rejects_empty_string() {
        let store = test_store();
        store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        assert_eq!(store.resolve_id_prefix("").unwrap(), None);
    }

    #[test]
    fn resolve_id_prefix_rejects_non_hex_letters() {
        let store = test_store();
        store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        // Contains no `%`/`_` at all — a validator that only strips LIKE
        // wildcard characters (rather than actually checking the alphabet
        // is hex digits + `-`) would let this slip through unchanged.
        assert_eq!(store.resolve_id_prefix("xyz").unwrap(), None);
    }

    /// A prefix shorter than the advertised 8 characters resolves to nothing.
    /// Without this, a mistyped id like `"a"` silently matches whichever
    /// comment sorts first — resolving or replying to someone else's comment
    /// and reporting success.
    #[test]
    fn resolve_id_prefix_rejects_prefixes_shorter_than_advertised() {
        let store = test_store();
        let review = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        // Genuine leading characters of a real id, and still refused.
        for len in 1..MIN_ID_PREFIX_LEN {
            let short_prefix = &review.id[..len];
            assert_eq!(
                store.resolve_id_prefix(short_prefix).unwrap(),
                None,
                "{len}-char prefix must not resolve"
            );
        }
        // The advertised length does resolve, so the bound is the only thing
        // being tested here.
        assert_eq!(
            store
                .resolve_id_prefix(&review.id[..MIN_ID_PREFIX_LEN])
                .unwrap(),
            Some(review.id.clone())
        );
    }
}
