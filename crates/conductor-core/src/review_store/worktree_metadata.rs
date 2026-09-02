//! ブランチをキーにしたメタ情報: ベースブランチ (worktree_metadata)、変更サマリ
//! (change_summary)、PR の素性 (pr_review_meta)。

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::ReviewStore;
use super::model::{Author, PrReviewMeta};

impl ReviewStore {
    pub fn save_worktree_base_branch(&self, branch: &str, base_branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO worktree_metadata (branch, base_branch) VALUES (?1, ?2)",
            params![branch, base_branch],
        )?;
        Ok(())
    }

    pub fn get_worktree_base_branch(&self, branch: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT base_branch FROM worktree_metadata WHERE branch = ?1",
                params![branch],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// base_branch が指定ブランチである直接の子ブランチ。
    pub fn get_worktree_children(&self, base: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT branch FROM worktree_metadata WHERE base_branch = ?1")?;
        let rows = stmt.query_map(params![base], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// ブランチ単位の変更サマリを保存 (置き換え) する。created_at は初回の値を保つ。
    pub fn save_change_summary(&self, branch: &str, body: &str, author: Author) -> Result<()> {
        self.conn.execute(
            "INSERT INTO change_summary (branch, body, author)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(branch) DO UPDATE SET
                 body = excluded.body,
                 author = excluded.author,
                 updated_at = datetime('now')",
            params![branch, body, author],
        )?;
        Ok(())
    }

    pub fn get_change_summary(&self, branch: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT body FROM change_summary WHERE branch = ?1",
                params![branch],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn save_pr_review_meta(&self, branch: &str, meta: &PrReviewMeta) -> Result<()> {
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
            params![
                branch,
                meta.pr_number,
                meta.pr_url,
                meta.pr_title,
                meta.base_ref,
                meta.head_ref,
                meta.author,
            ],
        )?;
        Ok(())
    }

    pub fn get_pr_review_meta(&self, branch: &str) -> Result<Option<PrReviewMeta>> {
        Ok(self
            .conn
            .query_row(
                "SELECT pr_number, pr_url, pr_title, base_ref, head_ref, author
                 FROM pr_review_meta WHERE branch = ?1",
                params![branch],
                |row| {
                    Ok(PrReviewMeta {
                        pr_number: row.get(0)?,
                        pr_url: row.get(1)?,
                        pr_title: row.get(2)?,
                        base_ref: row.get(3)?,
                        head_ref: row.get(4)?,
                        author: row.get(5)?,
                    })
                },
            )
            .optional()?)
    }
}
