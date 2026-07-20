//! Database schema creation and version-based migrations for `ReviewStore::open`.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::ReviewStore;

impl ReviewStore {
    /// Open (or create) the review database at the given path and run
    /// all migrations.
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("failed to open database at {}", db_path.display()))?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("failed to enable foreign keys")?;

        // Create tables that have never changed.
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

        // Version-based migration for the reviews table.
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

        if version < 1 {
            // Check whether the reviews table already exists (old schema).
            let table_exists: bool = conn.query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='reviews'",
                [],
                |r| r.get(0),
            )?;

            if table_exists {
                // Migrate from v0 (old schema with line_number) to v1.
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
                // Fresh database — create the reviews table with the new schema.
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

            // Create the review_replies table.
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
            // Move two facts that were previously duplicated by every writer
            // (the Rust TUI and the sibling Node MCP server) into the schema
            // itself, so neither side has to mirror the other:
            //   * `commit_ref` defaults to 'HEAD' — writers may now omit it.
            //   * `worktree = branch` is enforced by a CHECK, turning a silent
            //     shared assumption into a guarded contract. Legacy rows created
            //     before the `branch` column existed have branch IS NULL, which
            //     the CHECK deliberately permits.
            // SQLite can neither add a DEFAULT to an existing column nor add a
            // table-level CHECK in place, so the table is rebuilt. FK
            // enforcement is disabled for the swap (review_replies references
            // reviews by name and ids are preserved, so integrity holds).
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
            // Branch-level change summary — the "what & why" of the whole diff,
            // the PR-description counterpart to the line-anchored `reviews`. Kept
            // in its own table rather than `worktree_metadata` because that table
            // requires a non-null base_branch the MCP writer doesn't know; here a
            // single INSERT OR REPLACE keyed on branch is enough.
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
            // Review mode's PR walkthrough + GitHub-publish support.
            // `walkthroughs.branch` is UNIQUE with no generation history —
            // re-generating deletes and recreates the row (pamela's ruling:
            // a versioned/generational scheme wasn't worth the complexity
            // for a single-branch, single-current-walkthrough feature).
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
            // Record the branch tip a walkthrough was generated against, so a
            // regenerate request for an unchanged HEAD can be skipped (the
            // diff hasn't moved, so neither has the walkthrough). Nullable:
            // rows from before v7 have no recorded commit and are treated as
            // "unknown" (never skipped).
            conn.execute_batch(
                "
                ALTER TABLE walkthroughs ADD COLUMN head_commit TEXT;
                PRAGMA user_version = 7;
                ",
            )
            .context("failed to migrate to v7 (walkthroughs.head_commit)")?;
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
    fn v4_commit_ref_defaults_to_head() {
        let store = test_store();
        // Insert omitting commit_ref — the v4 schema default should fill it,
        // which is what lets the Node MCP writer stop mirroring 'HEAD'.
        store
            .conn
            .execute(
                "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, branch)
                 VALUES ('r1', 'feat/x', 'src/main.rs', 1, 'suggest', 'note', 'feat/x')",
                [],
            )
            .unwrap();
        let reviews = store.reviews_for_worktree("feat/x").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].commit_ref, "HEAD");
    }

    #[test]
    fn v4_check_rejects_worktree_branch_mismatch() {
        let store = test_store();
        // worktree != branch (both non-null) must violate the CHECK so a
        // drifting writer fails loudly instead of inserting an unreachable row.
        let result = store.conn.execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, commit_ref, branch)
             VALUES ('r1', 'feat/x', 'src/main.rs', 1, 'suggest', 'note', 'HEAD', 'feat/y')",
            [],
        );
        assert!(result.is_err(), "worktree != branch should violate the CHECK");
    }

    #[test]
    fn v4_check_allows_null_branch() {
        let store = test_store();
        // Legacy rows created before the `branch` column existed have branch
        // IS NULL; the CHECK must keep permitting them.
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
    fn migrates_existing_v5_db_to_v6() {
        // Simulate a pre-v6 database on disk: open a fresh store (which
        // migrates straight to the latest version), then hand-roll it back
        // down to what a v5 database looked like, and reopen it the same
        // way ReviewStore::open would encounter it in the wild.
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
                    DROP TABLE walkthroughs;
                    DROP TABLE walkthrough_steps;
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
        // Reopening a v5 db runs every migration up to the latest (v6 tables,
        // then v7's walkthroughs.head_commit).
        assert_eq!(version, 7);

        // Pre-existing data survived the migration.
        let reviews = store.reviews_for_worktree("feat/x").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].body, "predates the v6 migration");

        // The new tables/columns are usable, including v7's head_commit.
        assert!(store.get_walkthrough("feat/x").unwrap().is_none());
        assert!(store.get_pr_review_meta("feat/x").unwrap().is_none());
        assert_eq!(store.unpublished_reviews("feat/x").unwrap().len(), 1);
        let begun = store.begin_walkthrough("feat/x", Some("deadbeef")).unwrap();
        assert_eq!(begun.head_commit.as_deref(), Some("deadbeef"));
    }
}
