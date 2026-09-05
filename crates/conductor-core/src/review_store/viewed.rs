//! viewed_files テーブル: ブランチごとの「読んだ」印。

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::params;

use super::ReviewStore;

impl ReviewStore {
    pub fn set_viewed(&self, branch: &str, file_path: &str, viewed: bool) -> Result<()> {
        if viewed {
            self.conn.execute(
                "INSERT OR IGNORE INTO viewed_files (branch, file_path) VALUES (?1, ?2)",
                params![branch, file_path],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM viewed_files WHERE branch = ?1 AND file_path = ?2",
                params![branch, file_path],
            )?;
        }
        Ok(())
    }

    pub fn viewed_files(&self, branch: &str) -> Result<HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT file_path FROM viewed_files WHERE branch = ?1")?;
        let rows = stmt.query_map(params![branch], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
