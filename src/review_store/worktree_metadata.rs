//! Per-branch metadata: base branch (`worktree_metadata`), the branch-level
//! change summary (`change_summary`), and PR review metadata (`pr_review_meta`).

use anyhow::Result;
use rusqlite::params;

use super::ReviewStore;
use super::model::{Author, PrReviewMeta};

impl ReviewStore {
    /// Persist the base branch for a worktree branch.
    pub fn save_worktree_base_branch(&self, branch: &str, base_branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO worktree_metadata (branch, base_branch) VALUES (?1, ?2)",
            params![branch, base_branch],
        )?;
        Ok(())
    }

    /// Retrieve the persisted base branch for a worktree branch.
    pub fn get_worktree_base_branch(&self, branch: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT base_branch FROM worktree_metadata WHERE branch = ?1")?;
        let result = stmt
            .query_row(params![branch], |row| row.get::<_, String>(0))
            .ok();
        Ok(result)
    }

    /// Persist (or replace) the branch-level change summary — the "what & why"
    /// of the whole diff, shown as a banner above the diff and reusable as a PR
    /// body. `updated_at` is bumped on every write; `created_at` is preserved on
    /// replace via the COALESCE against the existing row.
    ///
    /// Two paths write it, both through this method: the `set_change_summary`
    /// MCP tool for a standalone overview, and `save_walkthrough`, which writes
    /// the walkthrough's summary here so the SUMMARY pseudo-file is filled in as
    /// a side effect of generating a walkthrough. Since `mcp-serve` is this same
    /// binary, there is no second implementation of this upsert to keep in sync.
    pub fn save_change_summary(&self, branch: &str, body: &str, author: Author) -> Result<()> {
        self.conn.execute(
            "INSERT INTO change_summary (branch, body, author, created_at, updated_at)
             VALUES (?1, ?2, ?3,
                     COALESCE((SELECT created_at FROM change_summary WHERE branch = ?1), datetime('now')),
                     datetime('now'))
             ON CONFLICT(branch) DO UPDATE SET
                 body = excluded.body,
                 author = excluded.author,
                 updated_at = datetime('now')",
            params![branch, body, author.as_str()],
        )?;
        Ok(())
    }

    /// Retrieve the change summary for a branch, if one has been written.
    pub fn get_change_summary(&self, branch: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT body FROM change_summary WHERE branch = ?1")?;
        let result = stmt
            .query_row(params![branch], |row| row.get::<_, String>(0))
            .ok();
        Ok(result)
    }

    /// Retrieve all branches whose base_branch equals the given branch (direct children).
    pub fn get_worktree_children(&self, base: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT branch FROM worktree_metadata WHERE base_branch = ?1")?;
        let rows = stmt.query_map(params![base], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Insert or replace the PR metadata for a branch.
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    pub fn save_pr_review_meta(
        &self,
        branch: &str,
        pr_number: Option<i64>,
        pr_url: Option<&str>,
        pr_title: Option<&str>,
        base_ref: Option<&str>,
        head_ref: Option<&str>,
        author: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pr_review_meta
                (branch, pr_number, pr_url, pr_title, base_ref, head_ref, author)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(branch) DO UPDATE SET
                 pr_number = excluded.pr_number,
                 pr_url    = excluded.pr_url,
                 pr_title  = excluded.pr_title,
                 base_ref  = excluded.base_ref,
                 head_ref  = excluded.head_ref,
                 author    = excluded.author",
            params![branch, pr_number, pr_url, pr_title, base_ref, head_ref, author],
        )?;
        Ok(())
    }

    /// Retrieve the PR metadata for a branch, if any has been saved.
    pub fn get_pr_review_meta(&self, branch: &str) -> Result<Option<PrReviewMeta>> {
        match self.conn.query_row(
            "SELECT branch, pr_number, pr_url, pr_title, base_ref, head_ref, author, created_at
             FROM pr_review_meta WHERE branch = ?1",
            params![branch],
            row_to_pr_review_meta,
        ) {
            Ok(meta) => Ok(Some(meta)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn row_to_pr_review_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrReviewMeta> {
    Ok(PrReviewMeta {
        branch: row.get(0)?,
        pr_number: row.get(1)?,
        pr_url: row.get(2)?,
        pr_title: row.get(3)?,
        base_ref: row.get(4)?,
        head_ref: row.get(5)?,
        author: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;
    use super::*;

    #[test]
    fn change_summary_save_get_and_replace() {
        let store = test_store();

        // Absent until written.
        assert_eq!(store.get_change_summary("feat/x").unwrap(), None);

        store
            .save_change_summary("feat/x", "Refactor the parser for clarity.", Author::Claude)
            .unwrap();
        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("Refactor the parser for clarity.")
        );

        // Replacing keeps the same key and overwrites the body (PK upsert).
        store
            .save_change_summary("feat/x", "Updated summary.", Author::User)
            .unwrap();
        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("Updated summary.")
        );

        // Independent per branch.
        assert_eq!(store.get_change_summary("feat/y").unwrap(), None);
    }

    #[test]
    fn pr_review_meta_upsert_and_get() {
        let store = test_store();

        assert!(store.get_pr_review_meta("feat/x").unwrap().is_none());

        store
            .save_pr_review_meta(
                "feat/x",
                Some(42),
                Some("https://github.com/o/r/pull/42"),
                Some("Add feature"),
                Some("main"),
                Some("feat/x"),
                Some("octocat"),
            )
            .unwrap();

        let meta = store.get_pr_review_meta("feat/x").unwrap().unwrap();
        assert_eq!(meta.pr_number, Some(42));
        assert_eq!(meta.pr_url.as_deref(), Some("https://github.com/o/r/pull/42"));
        assert_eq!(meta.author.as_deref(), Some("octocat"));

        // Upsert overwrites rather than duplicating.
        store
            .save_pr_review_meta(
                "feat/x",
                Some(42),
                Some("https://github.com/o/r/pull/42"),
                Some("Add feature (renamed)"),
                Some("main"),
                Some("feat/x"),
                Some("octocat"),
            )
            .unwrap();
        let meta = store.get_pr_review_meta("feat/x").unwrap().unwrap();
        assert_eq!(meta.pr_title.as_deref(), Some("Add feature (renamed)"));
    }
}
