//! CRUD for reusable comment templates (the `comment_templates` table).

use anyhow::Result;
use rusqlite::params;

use super::ReviewStore;
use super::model::{CommentKind, CommentTemplate};

impl ReviewStore {
    /// Return all comment templates, ordered by creation time.
    pub fn list_templates(&self) -> Result<Vec<CommentTemplate>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, body, kind FROM comment_templates ORDER BY created_at")?;

        let rows = stmt.query_map([], |row| {
            let kind_str: String = row.get(3)?;
            let kind = match kind_str.as_str() {
                "suggest" => CommentKind::Suggest,
                "question" => CommentKind::Question,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        format!("unknown CommentKind: {other}").into(),
                    ));
                }
            };
            Ok(CommentTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                body: row.get(2)?,
                kind,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Delete a comment template by id.
    pub fn delete_template(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM comment_templates WHERE id = ?1", params![id])?;
        if changed == 0 {
            anyhow::bail!("template not found: {id}");
        }
        Ok(())
    }
}
