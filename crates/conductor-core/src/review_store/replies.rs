//! review_replies テーブル: 返信の CRUD。

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::comments::ensure_found;
use super::model::{Author, ReviewReply};

impl ReviewStore {
    pub fn add_reply(&self, review_id: &str, body: &str, author: Author) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO review_replies (id, review_id, body, author) VALUES (?1, ?2, ?3, ?4)",
            params![id, review_id, body, author],
        )?;
        Ok(())
    }

    /// コメントへの返信を作成順で返す。
    pub fn get_replies(&self, review_id: &str) -> Result<Vec<ReviewReply>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, body, author, created_at
             FROM review_replies
             WHERE review_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![review_id], |row| {
            Ok(ReviewReply {
                id: row.get(0)?,
                body: row.get(1)?,
                author: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 返信 1 件だけを消す。親コメントと他の返信は残る。
    pub fn delete_reply(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM review_replies WHERE id = ?1", params![id])?;
        ensure_found(changed, "reply", id)
    }

    pub fn update_reply_body(&self, id: &str, body: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE review_replies SET body = ?1 WHERE id = ?2",
            params![body, id],
        )?;
        ensure_found(changed, "reply", id)
    }

    /// worktree の各コメント id に対する返信数。返信の無いコメントは載らない。
    pub fn reply_counts_for_worktree(&self, worktree: &str) -> Result<HashMap<String, usize>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.review_id, COUNT(*)
             FROM review_replies r
             JOIN reviews rv ON rv.id = r.review_id
             WHERE rv.worktree = ?1
             GROUP BY r.review_id",
        )?;
        let rows = stmt.query_map(params![worktree], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
