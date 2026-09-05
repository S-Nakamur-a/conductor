//! session_history テーブル: ターミナル出力スナップショットの保存、一覧、検索。

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::SessionHistory;

const SEARCH_LIMIT: i64 = 50;

impl ReviewStore {
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

    /// 新しい順に limit 件。
    pub fn list_session_history(&self, limit: usize) -> Result<Vec<SessionHistory>> {
        self.collect_history("", params![limit as i64])
    }

    /// 出力とラベルを部分一致で検索し、新しい順に 50 件まで返す。
    pub fn search_session_history(&self, query: &str) -> Result<Vec<SessionHistory>> {
        self.collect_history(
            "WHERE output_text LIKE ?2 OR label LIKE ?2",
            params![SEARCH_LIMIT, format!("%{query}%")],
        )
    }

    fn collect_history(
        &self,
        filter: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<SessionHistory>> {
        let sql = format!(
            "SELECT worktree, label, kind, output_text, saved_at
             FROM session_history {filter}
             ORDER BY saved_at DESC, rowid DESC
             LIMIT ?1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok(SessionHistory {
                worktree: row.get(0)?,
                label: row.get(1)?,
                kind: row.get(2)?,
                output_text: row.get(3)?,
                saved_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
