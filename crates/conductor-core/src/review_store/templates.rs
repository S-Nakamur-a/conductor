//! comment_templates テーブル: コメント雛形の一覧と削除。

use anyhow::Result;
use rusqlite::params;

use super::ReviewStore;
use super::comments::ensure_found;
use super::model::CommentTemplate;

impl ReviewStore {
    /// 雛形を作成順で返す。
    pub fn list_templates(&self) -> Result<Vec<CommentTemplate>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, body, kind FROM comment_templates ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| {
            Ok(CommentTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                body: row.get(2)?,
                kind: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_template(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM comment_templates WHERE id = ?1", params![id])?;
        ensure_found(changed, "template", id)
    }
}
