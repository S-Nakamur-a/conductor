//! セッション出力履歴（session_history テーブル）の保存・一覧・検索。

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::SessionHistory;

impl ReviewStore {
    /// セッション出力のスナップショットを履歴データベースに保存する。
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

    /// 直近のセッション履歴レコードを新しい順に返す。件数は limit で制限する。
    pub fn list_session_history(&self, limit: usize) -> Result<Vec<SessionHistory>> {
        let mut stmt = self.conn.prepare(
            "SELECT worktree, label, kind, output_text, saved_at
             FROM session_history
             ORDER BY saved_at DESC, rowid DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(SessionHistory {
                worktree: row.get(0)?,
                label: row.get(1)?,
                kind: row.get(2)?,
                output_text: row.get(3)?,
                saved_at: row.get(4)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// output_text と label を対象に全文検索する（SQL LIKE で % ワイルドカード）。
    /// 結果は50件まで。
    pub fn search_session_history(&self, query: &str) -> Result<Vec<SessionHistory>> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT worktree, label, kind, output_text, saved_at
             FROM session_history
             WHERE output_text LIKE ?1 OR label LIKE ?1
             ORDER BY saved_at DESC, rowid DESC
             LIMIT 50",
        )?;

        let rows = stmt.query_map(params![pattern], |row| {
            Ok(SessionHistory {
                worktree: row.get(0)?,
                label: row.get(1)?,
                kind: row.get(2)?,
                output_text: row.get(3)?,
                saved_at: row.get(4)?,
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
    fn セッション履歴の保存と一覧と検索() {
        let store = test_store();

        // 最初は空。
        let history = store.list_session_history(50).unwrap();
        assert!(history.is_empty());

        // 履歴レコードをいくつか保存する。
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

        // 一覧は3件全て返す（新しい順）。
        let history = store.list_session_history(50).unwrap();
        assert_eq!(history.len(), 3);
        // 新しい順なので sess-3 が先頭のはず。
        assert_eq!(history[0].label, "CC:2");
        assert_eq!(history[0].worktree, "wt2");
        assert_eq!(history[0].kind, "claude_code");

        // limit が効く。
        let history = store.list_session_history(2).unwrap();
        assert_eq!(history.len(), 2);

        // 出力テキストで検索する。
        let results = store.search_session_history("Error").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "CC:2");

        // label で検索する。
        let results = store.search_session_history("SH:1").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].output_text, "ls -la\ntotal 42");

        // 該当なしの検索。
        let results = store.search_session_history("nonexistent").unwrap();
        assert!(results.is_empty());
    }
}
