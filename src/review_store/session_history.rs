//! Save, list, and search session output history (the `session_history` table).

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::SessionHistory;

impl ReviewStore {
    /// Save a snapshot of a session's output to the history database.
    pub fn save_session_history(
        &self,
        session_id: &str,
        worktree: &str,
        label: &str,
        kind: &str,
        output: &str,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO session_history (id, session_id, worktree, label, kind, output_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, session_id, worktree, label, kind, output],
        )?;
        Ok(())
    }

    /// Return recent session history records (newest first), limited to `limit`.
    pub fn list_session_history(&self, limit: usize) -> Result<Vec<SessionHistory>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, worktree, label, kind, output_text, saved_at
             FROM session_history
             ORDER BY saved_at DESC, rowid DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(SessionHistory {
                id: row.get(0)?,
                session_id: row.get(1)?,
                worktree: row.get(2)?,
                label: row.get(3)?,
                kind: row.get(4)?,
                output_text: row.get(5)?,
                saved_at: row.get(6)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Full-text search on output_text and label (SQL LIKE with % wildcards).
    /// Limited to 50 results.
    pub fn search_session_history(&self, query: &str) -> Result<Vec<SessionHistory>> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, worktree, label, kind, output_text, saved_at
             FROM session_history
             WHERE output_text LIKE ?1 OR label LIKE ?1
             ORDER BY saved_at DESC, rowid DESC
             LIMIT 50",
        )?;

        let rows = stmt.query_map(params![pattern], |row| {
            Ok(SessionHistory {
                id: row.get(0)?,
                session_id: row.get(1)?,
                worktree: row.get(2)?,
                label: row.get(3)?,
                kind: row.get(4)?,
                output_text: row.get(5)?,
                saved_at: row.get(6)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;

    #[test]
    fn session_history_save_list_search() {
        let store = test_store();

        // Initially empty.
        let history = store.list_session_history(50).unwrap();
        assert!(history.is_empty());

        // Save some history records.
        store
            .save_session_history("sess-1", "wt1", "CC:1", "claude_code", "Hello world output")
            .unwrap();
        store
            .save_session_history("sess-2", "wt1", "SH:1", "shell", "ls -la\ntotal 42")
            .unwrap();
        store
            .save_session_history(
                "sess-3",
                "wt2",
                "CC:2",
                "claude_code",
                "Error: file not found",
            )
            .unwrap();

        // List returns all three (newest first).
        let history = store.list_session_history(50).unwrap();
        assert_eq!(history.len(), 3);
        // Newest first — sess-3 should be first.
        assert_eq!(history[0].session_id, "sess-3");
        assert_eq!(history[0].worktree, "wt2");
        assert_eq!(history[0].kind, "claude_code");

        // Limit works.
        let history = store.list_session_history(2).unwrap();
        assert_eq!(history.len(), 2);

        // Search by output text.
        let results = store.search_session_history("Error").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "sess-3");

        // Search by label.
        let results = store.search_session_history("SH:1").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "sess-2");

        // Search with no matches.
        let results = store.search_session_history("nonexistent").unwrap();
        assert!(results.is_empty());
    }
}
