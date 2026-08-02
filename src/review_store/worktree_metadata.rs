//! ブランチごとのメタデータ: ベースブランチ（worktree_metadata）、ブランチ単位の
//! 変更サマリ（change_summary）、PR レビューメタデータ（pr_review_meta）。

use anyhow::Result;
use rusqlite::params;

use super::ReviewStore;
use super::model::{Author, PrReviewMeta};

impl ReviewStore {
    /// worktree ブランチのベースブランチを保存する。
    pub fn save_worktree_base_branch(&self, branch: &str, base_branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO worktree_metadata (branch, base_branch) VALUES (?1, ?2)",
            params![branch, base_branch],
        )?;
        Ok(())
    }

    /// worktree ブランチについて保存済みのベースブランチを取得する。
    pub fn get_worktree_base_branch(&self, branch: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT base_branch FROM worktree_metadata WHERE branch = ?1")?;
        let result = stmt
            .query_row(params![branch], |row| row.get::<_, String>(0))
            .ok();
        Ok(result)
    }

    /// ブランチ単位の変更サマリ（差分全体の「何を・なぜ」）を保存（または置き換え）する。
    /// diff の上にバナーとして表示され、PR 本文としても再利用できる。書き込みのたびに
    /// updated_at を更新し、既存行に対する COALESCE により置き換え時も created_at を保持する。
    ///
    /// このメソッドを経由して書き込む経路は2つある。単独の概要を書く set_change_summary
    /// MCP ツールと、walkthrough のサマリをここに書き込む save_walkthrough で、後者では
    /// walkthrough 生成の副作用として SUMMARY 疑似ファイルが埋まる。mcp-serve は同じ
    /// バイナリなので、この upsert の実装を二重に持って同期を取る必要はない。
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

    /// ブランチの変更サマリを取得する（書き込まれていれば）。
    pub fn get_change_summary(&self, branch: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT body FROM change_summary WHERE branch = ?1")?;
        let result = stmt
            .query_row(params![branch], |row| row.get::<_, String>(0))
            .ok();
        Ok(result)
    }

    /// base_branch が指定ブランチと一致する全ブランチ（直接の子）を取得する。
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

    /// ブランチの PR メタデータを挿入または置き換える。
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

    /// ブランチの PR メタデータを取得する（保存されていれば）。
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

        // 書き込まれるまでは存在しない。
        assert_eq!(store.get_change_summary("feat/x").unwrap(), None);

        store
            .save_change_summary("feat/x", "Refactor the parser for clarity.", Author::Claude)
            .unwrap();
        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("Refactor the parser for clarity.")
        );

        // 置き換えは同じキーのまま body を上書きする（主キーによる upsert）。
        store
            .save_change_summary("feat/x", "Updated summary.", Author::User)
            .unwrap();
        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("Updated summary.")
        );

        // ブランチごとに独立している。
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

        // upsert なので重複せず上書きされる。
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
