//! reviews テーブル: コメントの CRUD と絞り込み。

use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use super::ReviewStore;
use super::model::{CommentStatus, NewReview, ReviewComment};

/// [ReviewStore::resolve_id_prefix] が受け付ける最短のプレフィックス長。
/// MCP ツールがコメント id を先頭 8 文字で見せているので、それと揃える。
pub const MIN_ID_PREFIX_LEN: usize = 8;

const SELECT_REVIEW: &str =
    "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
            author, branch, created_at
     FROM reviews";

impl ReviewStore {
    /// コメントを挿入して、保存された姿を返す。
    pub fn add_review(&self, review: NewReview<'_>) -> Result<ReviewComment> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, line_end, kind, body, author, branch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?2)",
            params![
                id,
                review.branch,
                review.file_path,
                review.line_start,
                review.line_end,
                review.kind,
                review.body,
                review.author,
            ],
        )?;
        self.get_review(&id)
    }

    pub fn get_review(&self, id: &str) -> Result<ReviewComment> {
        let sql = format!("{SELECT_REVIEW} WHERE id = ?1");
        Ok(self.conn.query_row(&sql, params![id], row_to_review)?)
    }

    pub fn update_review_body(&self, id: &str, body: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![body, id],
        )?;
        ensure_found(changed, "review", id)
    }

    pub fn update_review_status(&self, id: &str, status: CommentStatus) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status, id],
        )?;
        ensure_found(changed, "review", id)
    }

    /// 返信ごと削除する (ON DELETE CASCADE)。
    pub fn delete_review(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM reviews WHERE id = ?1", params![id])?;
        ensure_found(changed, "review", id)
    }

    /// worktree のコメントをファイル、行の順で返す。
    pub fn reviews_for_worktree(&self, worktree: &str) -> Result<Vec<ReviewComment>> {
        let sql = format!("{SELECT_REVIEW} WHERE worktree = ?1 ORDER BY file_path, line_start");
        self.collect_reviews(&sql, params![worktree])
    }

    /// まとめて投稿済みにする。一度付けると [Self::unpublished_reviews] から外れるので、
    /// 投稿のリトライが二重投稿にならない。
    pub fn mark_published(&self, comment_ids: &[String], timestamp: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for id in comment_ids {
            tx.execute(
                "UPDATE reviews SET published_at = ?1 WHERE id = ?2",
                params![timestamp, id],
            )?;
        }
        Ok(tx.commit()?)
    }

    pub fn unpublished_reviews(&self, branch: &str) -> Result<Vec<ReviewComment>> {
        let sql = format!(
            "{SELECT_REVIEW} WHERE branch = ?1 AND published_at IS NULL ORDER BY file_path, line_start"
        );
        self.collect_reviews(&sql, params![branch])
    }

    /// 完全な id か、その一意なプレフィックスを完全な id に解決する。
    ///
    /// [MIN_ID_PREFIX_LEN] 未満は打ち間違いとみなして `Ok(None)`。UUID の文字集合
    /// (16 進数と -) 以外を含むものも `Ok(None)`。複数当たれば id 順で最初の 1 件。
    pub fn resolve_id_prefix(&self, prefix: &str) -> Result<Option<String>> {
        // 短すぎるプレフィックスや LIKE のワイルドカードを通すと、id 順で最初に
        // 来た他人のコメントに解決され、しかも成功したと報告されてしまう。
        if prefix.len() < MIN_ID_PREFIX_LEN
            || !prefix.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM reviews WHERE id LIKE ?1 ORDER BY id LIMIT 1",
                params![format!("{prefix}%")],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// 未解決のコメント。branch / worktree / file_path で任意に絞れる。
    pub fn pending_reviews(
        &self,
        branch: Option<&str>,
        worktree: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<Vec<ReviewComment>> {
        let mut sql = format!("{SELECT_REVIEW} WHERE status = 'pending'");
        let mut bind: Vec<&dyn rusqlite::ToSql> = Vec::new();
        if let Some(w) = &worktree {
            sql.push_str(" AND worktree = ?");
            bind.push(w);
        }
        // branch 列が入る前の行は branch IS NULL なので、worktree 列でも当てる。
        if let Some(b) = &branch {
            sql.push_str(" AND (branch = ? OR worktree = ?)");
            bind.push(b);
            bind.push(b);
        }
        if let Some(f) = &file_path {
            sql.push_str(" AND file_path = ?");
            bind.push(f);
        }
        sql.push_str(" ORDER BY file_path, line_start");
        self.collect_reviews(&sql, bind.as_slice())
    }

    fn collect_reviews(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, row_to_review)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

pub(super) fn ensure_found(changed: usize, what: &str, id: &str) -> Result<()> {
    if changed == 0 {
        anyhow::bail!("{what} not found: {id}");
    }
    Ok(())
}

fn row_to_review(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewComment> {
    Ok(ReviewComment {
        id: row.get(0)?,
        worktree: row.get(1)?,
        file_path: row.get(2)?,
        line_start: row.get(3)?,
        line_end: row.get(4)?,
        kind: row.get(5)?,
        body: row.get(6)?,
        status: row.get(7)?,
        author: row.get(8)?,
        branch: row.get(9)?,
        created_at: row.get(10)?,
    })
}
