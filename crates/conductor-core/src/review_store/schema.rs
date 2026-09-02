//! スキーマ作成と user_version によるマイグレーション。
//!
//! 過去のマイグレーションは書き換えない。既存 DB はそれを通過済みなので、
//! 書き換えると新旧で辿る道が食い違う。廃止は DROP TABLE を足す形で行う。

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::ReviewStore;

const CURRENT_VERSION: i32 = 9;

impl ReviewStore {
    /// DB を開き (無ければ作り)、最新スキーマまでマイグレーションする。
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("failed to open database at {}", db_path.display()))?;
        configure(&conn)?;
        migrate(&conn)?;
        Ok(Self { conn })
    }
}

fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .context("failed to enable foreign keys")?;

    // WAL 切替は排他ロックを要るので、busy_timeout が先でないと TUI が読んでいる
    // 最中の mcp-serve 起動が即 SQLITE_BUSY になる。rusqlite は open 時に同じ
    // 5000ms を入れているが、その既定に依らず明示しておく。
    conn.execute_batch("PRAGMA busy_timeout = 5000;")
        .context("failed to set busy_timeout")?;

    // WAL 無しでもレビュー機能は動く。ここで Err にすると呼び出し側が store を
    // 丸ごと捨てるので、既定 journal のまま動くより悪くなる。
    if let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
        log::warn!("failed to switch database to WAL journal mode: {e}");
    }
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
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

    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if version < 1 {
        migrate_to_v1(conn)?;
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
        migrate_to_v4(conn)?;
    }

    if version < 5 {
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
        conn.execute_batch(
            "
            ALTER TABLE walkthroughs ADD COLUMN head_commit TEXT;
            PRAGMA user_version = 7;
            ",
        )
        .context("failed to migrate to v7 (walkthroughs.head_commit)")?;
    }

    if version < 8 {
        conn.execute_batch(
            "
            DROP TABLE IF EXISTS walkthrough_steps;
            DROP TABLE IF EXISTS walkthroughs;
            PRAGMA user_version = 8;
            ",
        )
        .context("failed to migrate to v8 (drop walkthroughs)")?;
    }

    if version < 9 {
        conn.execute_batch(
            "
            DROP TABLE IF EXISTS daily_stats;
            DROP TABLE IF EXISTS session_stats;
            PRAGMA user_version = 9;
            ",
        )
        .context("failed to migrate to v9 (drop gamification stats)")?;
    }

    debug_assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))?,
        CURRENT_VERSION
    );
    Ok(())
}

fn migrate_to_v1(conn: &Connection) -> Result<()> {
    let reviews_exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='reviews'",
        [],
        |r| r.get(0),
    )?;

    if reviews_exists {
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

        PRAGMA user_version = 1;
        ",
    )
    .context("failed to create review_replies table")
}

// SQLite は既存列への DEFAULT 追加もテーブル CHECK の追加もできないので作り直す。
// review_replies は id を名前で参照し id は保持されるので、入れ替え中だけ
// 外部キーを切っても整合性は崩れない。
fn migrate_to_v4(conn: &Connection) -> Result<()> {
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
        .context("failed to re-enable foreign keys after v4 migration")
}
