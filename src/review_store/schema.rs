//! ReviewStore::open のためのデータベーススキーマ作成とバージョンベースのマイグレーション。

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::ReviewStore;

impl ReviewStore {
    /// 指定パスのレビューデータベースを開く（なければ作成する）。
    /// すべてのマイグレーションを実行する。
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("failed to open database at {}", db_path.display()))?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("failed to enable foreign keys")?;

        // busy_timeout は、下の WAL 切り替えより前に設定しておく必要がある。
        // journal_mode の切り替えは一瞬だが排他ロックを要求するため、
        // busy_timeout が事前に設定されていないと、既にこの DB を開いている
        // TUI 側の接続がある場合、待たずに即座に SQLITE_BUSY で失敗してしまう。
        conn.execute_batch("PRAGMA busy_timeout = 5000;")
            .context("failed to set busy_timeout")?;

        // WAL により TUI と mcp-serve プロセスが同時にこのデータベースを開いた
        // ままにできる。失敗してもログに記録するだけで伝播はさせない。WAL が
        // 無くてもレビュー機能自体は動くが、ここで Err を返すと
        // app/lifecycle.rs が review_store を丸ごと破棄してしまい、それは
        // デフォルトの journal mode のまま動くよりも悪い結果になる
        // （しかも RUST_LOG を設定していなければサイレントに起きる）。
        if let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
            log::warn!("failed to switch database to WAL journal mode: {e}");
        }

        // 一度も変更されていないテーブルを作成する。
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                id          TEXT PRIMARY KEY,
                worktree    TEXT NOT NULL,
                label       TEXT,
                kind        TEXT NOT NULL CHECK (kind IN ('claude_code', 'shell')),
                pid         INTEGER,
                started_at  TEXT NOT NULL DEFAULT (datetime('now')),
                is_active   INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS worktree_state (
                worktree    TEXT PRIMARY KEY,
                last_viewed_file TEXT,
                last_viewed_line INTEGER,
                scroll_positions TEXT
            );

            -- Single-row table holding cross-cutting UI state for this repo
            -- (currently just which worktree was last selected).
            CREATE TABLE IF NOT EXISTS ui_state (
                id                INTEGER PRIMARY KEY CHECK (id = 1),
                selected_worktree TEXT
            );

            CREATE TABLE IF NOT EXISTS comment_templates (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                body        TEXT NOT NULL,
                kind        TEXT NOT NULL DEFAULT 'suggest' CHECK (kind IN ('suggest', 'question')),
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS session_history (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL,
                worktree    TEXT NOT NULL,
                label       TEXT NOT NULL DEFAULT '',
                kind        TEXT NOT NULL CHECK (kind IN ('claude_code', 'shell')),
                output_text TEXT NOT NULL,
                saved_at    TEXT NOT NULL DEFAULT (datetime('now'))
            );
            ",
        )
        .context("failed to run CREATE TABLE migrations")?;

        // reviews テーブルに対するバージョンベースのマイグレーション。
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

        if version < 1 {
            // reviews テーブルが既に存在するか（旧スキーマかどうか）を確認する。
            let table_exists: bool = conn.query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='reviews'",
                [],
                |r| r.get(0),
            )?;

            if table_exists {
                // v0（line_number を持つ旧スキーマ）から v1 へマイグレーションする。
                conn.execute_batch(
                    "
                    ALTER TABLE reviews RENAME COLUMN line_number TO line_start;
                    ALTER TABLE reviews ADD COLUMN line_end   INTEGER;
                    ALTER TABLE reviews ADD COLUMN author     TEXT NOT NULL DEFAULT 'user';
                    ALTER TABLE reviews ADD COLUMN branch     TEXT;
                    ",
                )
                .context("failed to migrate reviews table to v1")?;
            } else {
                // 新規データベース。新スキーマで reviews テーブルを作成する。
                conn.execute_batch(
                    "
                    CREATE TABLE reviews (
                        id          TEXT PRIMARY KEY,
                        worktree    TEXT NOT NULL,
                        file_path   TEXT NOT NULL,
                        line_start  INTEGER NOT NULL,
                        line_end    INTEGER,
                        kind        TEXT NOT NULL CHECK (kind IN ('suggest', 'question')),
                        body        TEXT NOT NULL,
                        status      TEXT NOT NULL DEFAULT 'pending'
                                      CHECK (status IN ('pending', 'resolved')),
                        commit_ref  TEXT NOT NULL,
                        author      TEXT NOT NULL DEFAULT 'user'
                                      CHECK (author IN ('user', 'claude')),
                        branch      TEXT,
                        created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                        updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    ",
                )
                .context("failed to create reviews table")?;
            }

            // review_replies テーブルを作成する。
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS review_replies (
                    id          TEXT PRIMARY KEY,
                    review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
                    body        TEXT NOT NULL,
                    author      TEXT NOT NULL DEFAULT 'user'
                                  CHECK (author IN ('user', 'claude')),
                    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
                );
                ",
            )
            .context("failed to create review_replies table")?;

            conn.execute_batch("PRAGMA user_version = 1;")
                .context("failed to set user_version")?;
        }

        if version < 2 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS daily_stats (
                    date             TEXT PRIMARY KEY,
                    reviews_created  INTEGER NOT NULL DEFAULT 0,
                    branches_created INTEGER NOT NULL DEFAULT 0,
                    commits_made     INTEGER NOT NULL DEFAULT 0,
                    sessions_used    INTEGER NOT NULL DEFAULT 0,
                    first_seen_at    TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE IF NOT EXISTS session_stats (
                    id               TEXT PRIMARY KEY,
                    started_at       TEXT NOT NULL DEFAULT (datetime('now')),
                    ended_at         TEXT,
                    reviews_created  INTEGER NOT NULL DEFAULT 0,
                    branches_created INTEGER NOT NULL DEFAULT 0,
                    commits_made     INTEGER NOT NULL DEFAULT 0
                );

                PRAGMA user_version = 2;
                ",
            )
            .context("failed to migrate to v2 (daily_stats, session_stats)")?;
        }

        if version < 3 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS worktree_metadata (
                    branch       TEXT PRIMARY KEY,
                    base_branch  TEXT NOT NULL,
                    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
                );

                PRAGMA user_version = 3;
                ",
            )
            .context("failed to migrate to v3 (worktree_metadata)")?;
        }

        if version < 4 {
            // これまで各書き込み側（Rust の TUI と、隣接する Node の MCP サーバ）が
            // それぞれ重複して持っていた2つの事実をスキーマ自体に移し、どちらか
            // 一方がもう一方をミラーする必要をなくす。
            //   * commit_ref のデフォルトは 'HEAD' になる。書き込み側は省略可能になる。
            //   * worktree = branch は CHECK で強制する。これにより、暗黙の共有前提
            //     だったものが保証付きの契約になる。branch カラムが存在する前に
            //     作られた旧レコードは branch IS NULL であり、CHECK は意図的にそれを許可する。
            // SQLite は既存カラムへの DEFAULT 追加も、テーブルレベルの CHECK の
            // その場での追加もできないため、テーブルを作り直す。入れ替え中は
            // 外部キー制約を無効化する（review_replies は reviews を名前で参照し
            // id は保持されるため整合性は保たれる）。
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .context("failed to disable foreign keys for v4 migration")?;
            conn.execute_batch(
                "
                BEGIN;

                CREATE TABLE reviews_new (
                    id          TEXT PRIMARY KEY,
                    worktree    TEXT NOT NULL,
                    file_path   TEXT NOT NULL,
                    line_start  INTEGER NOT NULL,
                    line_end    INTEGER,
                    kind        TEXT NOT NULL CHECK (kind IN ('suggest', 'question')),
                    body        TEXT NOT NULL,
                    status      TEXT NOT NULL DEFAULT 'pending'
                                  CHECK (status IN ('pending', 'resolved')),
                    commit_ref  TEXT NOT NULL DEFAULT 'HEAD',
                    author      TEXT NOT NULL DEFAULT 'user'
                                  CHECK (author IN ('user', 'claude')),
                    branch      TEXT,
                    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    CHECK (branch IS NULL OR worktree = branch)
                );

                INSERT INTO reviews_new
                    (id, worktree, file_path, line_start, line_end, kind, body,
                     status, commit_ref, author, branch, created_at, updated_at)
                SELECT
                     id, worktree, file_path, line_start, line_end, kind, body,
                     status, commit_ref, author, branch, created_at, updated_at
                FROM reviews;

                DROP TABLE reviews;
                ALTER TABLE reviews_new RENAME TO reviews;

                PRAGMA user_version = 4;

                COMMIT;
                ",
            )
            .context("failed to migrate to v4 (commit_ref default, worktree=branch check)")?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .context("failed to re-enable foreign keys after v4 migration")?;
        }

        if version < 5 {
            // ブランチ単位の変更サマリ。差分全体の「何を・なぜ」であり、行に
            // 紐づく reviews に対する PR 本文的な対応物。worktree_metadata では
            // なく専用テーブルにしているのは、あちらは MCP 側の書き込み元が
            // 知らない非 null の base_branch を要求するため。こちらは branch を
            // キーにした INSERT OR REPLACE 1本で足りる。
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS change_summary (
                    branch      TEXT PRIMARY KEY,
                    body        TEXT NOT NULL,
                    author      TEXT NOT NULL DEFAULT 'claude'
                                  CHECK (author IN ('user', 'claude')),
                    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
                );

                PRAGMA user_version = 5;
                ",
            )
            .context("failed to migrate to v5 (change_summary)")?;
        }

        if version < 6 {
            // レビューモードの PR walkthrough と GitHub への投稿対応。
            // walkthroughs.branch は UNIQUE で世代履歴は持たない。再生成時は
            // 既存行を削除して作り直す（1ブランチにつき現行の walkthrough が
            // 1本あればよい機能なので、バージョン管理・世代管理の仕組みは
            // その複雑さに見合わないと判断した）。
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS walkthroughs (
                    id          TEXT PRIMARY KEY,
                    branch      TEXT NOT NULL UNIQUE,
                    title       TEXT,
                    summary     TEXT,
                    status      TEXT NOT NULL DEFAULT 'generating'
                                  CHECK (status IN ('generating', 'ready', 'failed')),
                    error       TEXT,
                    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE IF NOT EXISTS walkthrough_steps (
                    id             TEXT PRIMARY KEY,
                    walkthrough_id TEXT NOT NULL REFERENCES walkthroughs(id) ON DELETE CASCADE,
                    seq            INTEGER NOT NULL,
                    file_path      TEXT NOT NULL,
                    line_start     INTEGER,
                    line_end       INTEGER,
                    kind           TEXT NOT NULL CHECK (kind IN ('intent', 'core', 'ripple', 'test')),
                    title          TEXT NOT NULL,
                    body           TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS pr_review_meta (
                    branch      TEXT PRIMARY KEY,
                    pr_number   INTEGER,
                    pr_url      TEXT,
                    pr_title    TEXT,
                    base_ref    TEXT,
                    head_ref    TEXT,
                    author      TEXT,
                    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
                );

                ALTER TABLE reviews ADD COLUMN published_at TEXT;

                PRAGMA user_version = 6;
                ",
            )
            .context(
                "failed to migrate to v6 (walkthroughs, walkthrough_steps, pr_review_meta, reviews.published_at)",
            )?;
        }

        if version < 7 {
            // walkthrough を生成した時点のブランチの先端コミットを記録しておき、
            // HEAD が変わっていない再生成リクエストをスキップできるようにする
            // （diff が動いていなければ walkthrough も動いていないはず）。
            // null 許容: v7 より前の行にはコミットが記録されておらず、
            // 「不明」として扱われる（＝スキップされることはない）。
            conn.execute_batch(
                "
                ALTER TABLE walkthroughs ADD COLUMN head_commit TEXT;
                PRAGMA user_version = 7;
                ",
            )
            .context("failed to migrate to v7 (walkthroughs.head_commit)")?;
        }

        if version < 8 {
            // walkthrough の廃止。ツアーの表示は revidere の成果物
            // (<worktree>/.revidere/review.json) が引き継いだので、この 2 つの
            // テーブルを読む者はもういない。v6/v7 を書き換えて「最初から
            // 作らない」ことにはしない。既存の DB は v6 を通過済みなので、
            // 過去のマイグレーションを書き換えると新旧で辿る道が食い違う。
            conn.execute_batch(
                "
                DROP TABLE IF EXISTS walkthrough_steps;
                DROP TABLE IF EXISTS walkthroughs;
                PRAGMA user_version = 8;
                ",
            )
            .context("failed to migrate to v8 (drop walkthroughs)")?;
        }

        Ok(Self { conn })
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{Author, CommentKind};
    use super::super::test_support::test_store;
    use super::*;

    #[test]
    fn open_sets_wal_journal_mode() {
        // WAL はファイルバックのデータベースでのみ有効になる。test_store() が
        // 使う :memory: では切り替わらないため、この PRAGMA を検証するには
        // 実際の tempdir 上の DB が必要になる。
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("conductor.db");
        let store = ReviewStore::open(&db_path).unwrap();

        let journal_mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode, "wal");

        let busy_timeout: i64 = store
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
    }

    #[test]
    fn v4_commit_ref_defaults_to_head() {
        let store = test_store();
        // commit_ref を省略して挿入する。v4 スキーマのデフォルトが埋めるはずで、
        // これにより Node 側の MCP 書き込み元は 'HEAD' をミラーする必要がなくなる。
        store
            .conn
            .execute(
                "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, branch)
                 VALUES ('r1', 'feat/x', 'src/main.rs', 1, 'suggest', 'note', 'feat/x')",
                [],
            )
            .unwrap();
        // commit_ref は ReviewComment には載らない（読む側がいない）ので、
        // デフォルトが入ったことは列を直接引いて確かめる。
        let commit_ref: String = store
            .conn
            .query_row("SELECT commit_ref FROM reviews WHERE id = 'r1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(commit_ref, "HEAD");
    }

    #[test]
    fn v4_check_rejects_worktree_branch_mismatch() {
        let store = test_store();
        // worktree != branch（両方 non-null）は CHECK に違反しなければならない。
        // これにより、ずれた書き込み元は到達不能な行を挿入するのではなく
        // 派手に失敗する。
        let result = store.conn.execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, commit_ref, branch)
             VALUES ('r1', 'feat/x', 'src/main.rs', 1, 'suggest', 'note', 'HEAD', 'feat/y')",
            [],
        );
        assert!(
            result.is_err(),
            "worktree != branch should violate the CHECK"
        );
    }

    #[test]
    fn v4_check_allows_null_branch() {
        let store = test_store();
        // branch カラムが存在する前に作られた旧レコードは branch IS NULL であり、
        // CHECK はそれを許可し続けなければならない。
        store
            .conn
            .execute(
                "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, commit_ref, branch)
                 VALUES ('r1', 'wt1', 'src/main.rs', 1, 'suggest', 'note', 'HEAD', NULL)",
                [],
            )
            .unwrap();
        let reviews = store.reviews_for_worktree("wt1").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].branch, None);
    }

    #[test]
    fn migrates_an_existing_v5_db_all_the_way_forward() {
        // ディスク上にある v6 より前のデータベースをシミュレートする。まず新規
        // ストアを開き（最新バージョンまで一気にマイグレーションされる）、それを
        // 手作業で v5 データベースの姿まで巻き戻し、ReviewStore::open が実際に
        // 遭遇するのと同じ形で開き直す。
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("conductor.db");

        {
            let store = ReviewStore::open(&db_path).unwrap();
            store
                .add_review(
                    "feat/x",
                    "src/main.rs",
                    1,
                    None,
                    CommentKind::Suggest,
                    "predates the v6 migration",
                    "abc123",
                    Author::User,
                    Some("feat/x"),
                )
                .unwrap();

            store
                .conn
                .execute_batch(
                    "
                    DROP TABLE pr_review_meta;
                    ALTER TABLE reviews DROP COLUMN published_at;
                    PRAGMA user_version = 5;
                    ",
                )
                .unwrap();
        }

        let store = ReviewStore::open(&db_path).unwrap();
        let version: i32 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        // v5 の db を開き直すと、最新までの全マイグレーションが実行される
        // （v6 のテーブル群、v7 の列追加、そして v8 の walkthrough 削除）。
        assert_eq!(version, 8);

        // 既存データはマイグレーションを生き延びている。
        let reviews = store.reviews_for_worktree("feat/x").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].body, "predates the v6 migration");

        // v6 で足したもののうち、生き残っている側は使える。
        assert!(store.get_pr_review_meta("feat/x").unwrap().is_none());
        assert_eq!(store.unpublished_reviews("feat/x").unwrap().len(), 1);

        // v8 で落とした側は、テーブルごと無くなっている。
        for table in ["walkthroughs", "walkthrough_steps"] {
            let found: i64 = store
                .conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 0, "{table} should be gone");
        }
    }
}
