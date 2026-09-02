//! 再起動時に復元する表示状態: worktree ごとの最終表示位置 (worktree_state) と
//! 最後に選択していた worktree (ui_state)。

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::ReviewStore;

impl ReviewStore {
    /// 何も開いていなければ file は None。
    pub fn save_view_state(&self, worktree: &str, file: Option<&str>, line: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO worktree_state (worktree, last_viewed_file, last_viewed_line)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(worktree) DO UPDATE SET
                 last_viewed_file = excluded.last_viewed_file,
                 last_viewed_line = excluded.last_viewed_line",
            params![worktree, file, line],
        )?;
        Ok(())
    }

    /// (最終表示ファイル, 行)。記録が無ければ None。
    pub fn get_view_state(&self, worktree: &str) -> Result<Option<(Option<String>, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_viewed_file, last_viewed_line FROM worktree_state WHERE worktree = ?1",
                params![worktree],
                |row| Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?.unwrap_or(0))),
            )
            .optional()?)
    }

    pub fn set_selected_worktree(&self, branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ui_state (id, selected_worktree) VALUES (1, ?1)",
            params![branch],
        )?;
        Ok(())
    }

    pub fn get_selected_worktree(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT selected_worktree FROM ui_state WHERE id = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }
}
