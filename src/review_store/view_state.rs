//! Restore-on-restart UI state: the last-viewed file/scroll position per
//! worktree (`worktree_state`) and the last-selected worktree (`ui_state`).

use anyhow::Result;
use rusqlite::params;

use super::ReviewStore;

impl ReviewStore {
    /// Persist the last-viewed file (relative path) and scroll position for a
    /// worktree branch. `file` may be `None` when no file was open.
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

    /// Retrieve `(last_viewed_file, last_viewed_line)` for a worktree branch.
    pub fn get_view_state(&self, worktree: &str) -> Result<Option<(Option<String>, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT last_viewed_file, last_viewed_line FROM worktree_state WHERE worktree = ?1",
        )?;
        let result = stmt
            .query_row(params![worktree], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                ))
            })
            .ok();
        Ok(result)
    }

    /// Persist which worktree branch was last selected (per-repo).
    pub fn set_selected_worktree(&self, branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ui_state (id, selected_worktree) VALUES (1, ?1)",
            params![branch],
        )?;
        Ok(())
    }

    /// Retrieve the last selected worktree branch, if any.
    pub fn get_selected_worktree(&self) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT selected_worktree FROM ui_state WHERE id = 1")?;
        let result = stmt
            .query_row([], |row| row.get::<_, Option<String>>(0))
            .ok()
            .flatten();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;

    #[test]
    fn view_state_save_and_get() {
        let store = test_store();

        // Absent worktree returns None.
        assert_eq!(store.get_view_state("feat/x").unwrap(), None);

        // Save and read back a file + scroll.
        store
            .save_view_state("feat/x", Some("src/main.rs"), 42)
            .unwrap();
        assert_eq!(
            store.get_view_state("feat/x").unwrap(),
            Some((Some("src/main.rs".to_string()), 42))
        );

        // Upsert overwrites the previous value (no duplicate rows).
        store
            .save_view_state("feat/x", Some("src/app/mod.rs"), 7)
            .unwrap();
        assert_eq!(
            store.get_view_state("feat/x").unwrap(),
            Some((Some("src/app/mod.rs".to_string()), 7))
        );

        // A None file (nothing open) round-trips.
        store.save_view_state("feat/x", None, 0).unwrap();
        assert_eq!(store.get_view_state("feat/x").unwrap(), Some((None, 0)));
    }

    #[test]
    fn selected_worktree_save_and_get() {
        let store = test_store();

        assert_eq!(store.get_selected_worktree().unwrap(), None);

        store.set_selected_worktree("feat/a").unwrap();
        assert_eq!(
            store.get_selected_worktree().unwrap(),
            Some("feat/a".to_string())
        );

        // Single-row table: setting again replaces, never accumulates.
        store.set_selected_worktree("feat/b").unwrap();
        assert_eq!(
            store.get_selected_worktree().unwrap(),
            Some("feat/b".to_string())
        );
    }
}
