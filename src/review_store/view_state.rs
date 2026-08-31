//! 再起動時に復元する UI 状態: worktree ごとの最終表示ファイル/スクロール位置
//! (worktree_state) と、最後に選択していた worktree (ui_state)。

use anyhow::Result;
use rusqlite::params;

use super::ReviewStore;

impl ReviewStore {
    /// worktree ブランチごとに最終表示ファイル（相対パス）とスクロール位置を保存する。
    /// ファイルが開かれていなかった場合 file は None になり得る。
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

    /// worktree ブランチの (last_viewed_file, last_viewed_line) を取得する。
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

    /// リポジトリごとに、最後に選択されていた worktree ブランチを保存する。
    pub fn set_selected_worktree(&self, branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ui_state (id, selected_worktree) VALUES (1, ?1)",
            params![branch],
        )?;
        Ok(())
    }

    /// 最後に選択されていた worktree ブランチを取得する（あれば）。
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
    fn ビュー状態の保存と取得() {
        let store = test_store();

        // 存在しない worktree は None を返す。
        assert_eq!(store.get_view_state("feat/x").unwrap(), None);

        // ファイルとスクロール位置を保存して読み戻す。
        store
            .save_view_state("feat/x", Some("src/main.rs"), 42)
            .unwrap();
        assert_eq!(
            store.get_view_state("feat/x").unwrap(),
            Some((Some("src/main.rs".to_string()), 42))
        );

        // upsert なので前の値を上書きする（行が重複しない）。
        store
            .save_view_state("feat/x", Some("src/app/mod.rs"), 7)
            .unwrap();
        assert_eq!(
            store.get_view_state("feat/x").unwrap(),
            Some((Some("src/app/mod.rs".to_string()), 7))
        );

        // file が None（何も開いていない状態）でも往復できる。
        store.save_view_state("feat/x", None, 0).unwrap();
        assert_eq!(store.get_view_state("feat/x").unwrap(), Some((None, 0)));
    }

    #[test]
    fn 選択中worktreeの保存と取得() {
        let store = test_store();

        assert_eq!(store.get_selected_worktree().unwrap(), None);

        store.set_selected_worktree("feat/a").unwrap();
        assert_eq!(
            store.get_selected_worktree().unwrap(),
            Some("feat/a".to_string())
        );

        // 1行だけのテーブルなので、再設定すると置き換わり蓄積しない。
        store.set_selected_worktree("feat/b").unwrap();
        assert_eq!(
            store.get_selected_worktree().unwrap(),
            Some("feat/b".to_string())
        );
    }
}
