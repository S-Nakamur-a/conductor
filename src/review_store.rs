//! SQLite-backed review/annotation database.
//!
//! Stores code review comments, session metadata, and worktree UI state using
//! `rusqlite` so that review state persists across application restarts.
//!
//! The database lives at `<git-root>/.conductor/conductor.db`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// The kind of review comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    Suggest,
    Question,
}

impl CommentKind {
    /// Convert to the string representation stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentKind::Suggest => "suggest",
            CommentKind::Question => "question",
        }
    }
}

impl std::fmt::Display for CommentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The author of a review comment or reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Claude,
}

impl Author {
    /// Convert to the string representation stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            Author::User => "user",
            Author::Claude => "claude",
        }
    }
}

impl std::fmt::Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The resolution status of a review comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentStatus {
    Pending,
    Resolved,
}

impl CommentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentStatus::Pending => "pending",
            CommentStatus::Resolved => "resolved",
        }
    }
}

impl std::fmt::Display for CommentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Data structs
// ---------------------------------------------------------------------------

/// A single review comment attached to a file and line range.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReviewComment {
    pub id: String,
    pub worktree: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub kind: CommentKind,
    pub body: String,
    pub status: CommentStatus,
    pub commit_ref: String,
    pub author: Author,
    pub branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A reusable comment template (saved feedback pattern).
#[derive(Debug, Clone)]
pub struct CommentTemplate {
    pub id: String,
    pub name: String,
    pub body: String,
    pub kind: CommentKind,
}

/// A reply to a review comment.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReviewReply {
    pub id: String,
    pub review_id: String,
    pub body: String,
    pub author: Author,
    pub created_at: String,
}

/// Daily activity statistics.
#[derive(Debug, Clone)]
pub struct DailyStats {
    pub reviews_created: i64,
    pub branches_created: i64,
    pub commits_made: i64,
}

/// Summary statistics for the current session.
#[derive(Debug, Clone, Default)]
pub struct SessionStatsSnapshot {
    pub reviews_created: i64,
    pub branches_created: i64,
    pub commits_made: i64,
}

/// Streak information.
#[derive(Debug, Clone)]
pub struct StreakInfo {
    pub consecutive_days: u32,
}

/// A saved session history record.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionHistory {
    pub id: String,
    pub session_id: String,
    pub worktree: String,
    pub label: String,
    pub kind: String,
    pub output_text: String,
    pub saved_at: String,
}

// ---------------------------------------------------------------------------
// Helper: db_path
// ---------------------------------------------------------------------------

/// Return the path to the conductor database for a given repository root,
/// creating the `.conductor` directory if it does not yet exist.
pub fn db_path(repo_root: &Path) -> PathBuf {
    let dir = repo_root.join(".conductor");
    // Best-effort directory creation; errors will surface when we try to open
    // the database file.
    let _ = fs::create_dir_all(&dir);
    dir.join("conductor.db")
}

// ---------------------------------------------------------------------------
// ReviewStore
// ---------------------------------------------------------------------------

/// Manages the SQLite database for reviews, sessions, and worktree state.
pub struct ReviewStore {
    conn: Connection,
}

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

        Ok(Self { conn })
    }

    // -- Reviews ------------------------------------------------------------

    /// Insert a new review comment and return it.
    #[allow(clippy::too_many_arguments)]
    pub fn add_review(
        &self,
        worktree: &str,
        file_path: &str,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: &str,
        commit_ref: &str,
        author: Author,
        branch: Option<&str>,
    ) -> Result<ReviewComment> {
        let id = Uuid::new_v4().to_string();

        self.conn.execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, line_end, kind, body, commit_ref, author, branch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                worktree,
                file_path,
                line_start as i64,
                line_end.map(|n| n as i64),
                kind.as_str(),
                body,
                commit_ref,
                author.as_str(),
                branch,
            ],
        )?;

        // Read back to get the server-side defaults (created_at, updated_at).
        self.get_review(&id)
    }

    /// Fetch a single review by id.
    fn get_review(&self, id: &str) -> Result<ReviewComment> {
        self.conn
            .query_row(
                "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                        commit_ref, author, branch, created_at, updated_at
                 FROM reviews WHERE id = ?1",
                params![id],
                row_to_review,
            )
            .map_err(Into::into)
    }

    /// Edit the body text of a review comment.
    pub fn update_review_body(&self, id: &str, body: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![body, id],
        )?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// Delete a review comment by id.
    pub fn delete_review(&self, id: &str) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM reviews WHERE id = ?1",
            params![id],
        )?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// Update the status of a review comment.
    pub fn update_review_status(&self, id: &str, status: CommentStatus) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// Return all reviews for a given worktree, ordered by file then line.
    pub fn reviews_for_worktree(&self, worktree: &str) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    commit_ref, author, branch, created_at, updated_at
             FROM reviews
             WHERE worktree = ?1
             ORDER BY file_path, line_start",
        )?;
        collect_reviews(&mut stmt, params![worktree])
    }

    /// Return reviews for a specific file within a worktree.
    #[allow(dead_code)]
    pub fn reviews_for_file(
        &self,
        worktree: &str,
        file_path: &str,
    ) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    commit_ref, author, branch, created_at, updated_at
             FROM reviews
             WHERE worktree = ?1 AND file_path = ?2
             ORDER BY line_start",
        )?;
        collect_reviews(&mut stmt, params![worktree, file_path])
    }

    // -- Replies ------------------------------------------------------------

    /// Insert a reply to a review comment.
    pub fn add_reply(&self, review_id: &str, body: &str, author: Author) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO review_replies (id, review_id, body, author)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, review_id, body, author.as_str()],
        )?;
        Ok(())
    }

    /// Return all replies for a given review comment, ordered by creation time.
    pub fn get_replies(&self, review_id: &str) -> Result<Vec<ReviewReply>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, review_id, body, author, created_at
             FROM review_replies
             WHERE review_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![review_id], |row| {
            let author_str: String = row.get(3)?;
            let author = match author_str.as_str() {
                "user" => Author::User,
                "claude" => Author::Claude,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        format!("unknown Author: {other}").into(),
                    ))
                }
            };
            Ok(ReviewReply {
                id: row.get(0)?,
                review_id: row.get(1)?,
                body: row.get(2)?,
                author,
                created_at: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Return reply counts for all comments in a given worktree.
    ///
    /// Returns a map of review_id → reply count.
    pub fn reply_counts_for_worktree(
        &self,
        worktree: &str,
    ) -> Result<std::collections::HashMap<String, usize>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.review_id, COUNT(*)
             FROM review_replies r
             JOIN reviews rv ON rv.id = r.review_id
             WHERE rv.worktree = ?1
             GROUP BY r.review_id",
        )?;
        let rows = stmt.query_map(params![worktree], |row| {
            let review_id: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((review_id, count as usize))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (id, count) = row?;
            map.insert(id, count);
        }
        Ok(map)
    }

    // -- Comment Templates --------------------------------------------------

    /// Return all comment templates, ordered by creation time.
    pub fn list_templates(&self) -> Result<Vec<CommentTemplate>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, body, kind FROM comment_templates ORDER BY created_at",
        )?;

        let rows = stmt.query_map([], |row| {
            let kind_str: String = row.get(3)?;
            let kind = match kind_str.as_str() {
                "suggest" => CommentKind::Suggest,
                "question" => CommentKind::Question,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        format!("unknown CommentKind: {other}").into(),
                    ))
                }
            };
            Ok(CommentTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                body: row.get(2)?,
                kind,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Delete a comment template by id.
    pub fn delete_template(&self, id: &str) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM comment_templates WHERE id = ?1",
            params![id],
        )?;
        if changed == 0 {
            anyhow::bail!("template not found: {id}");
        }
        Ok(())
    }

    // -- Session History ----------------------------------------------------

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

    // -- Daily Stats / Gamification -----------------------------------------

    /// Increment a counter in the daily_stats table for today.
    pub fn increment_daily_stat(&self, field: &str) -> Result<()> {
        let valid_field = match field {
            "reviews_created" | "branches_created" | "commits_made" | "sessions_used" => field,
            _ => anyhow::bail!("invalid stat field: {field}"),
        };
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.conn.execute(
            &format!(
                "INSERT INTO daily_stats (date, {valid_field})
                 VALUES (?1, 1)
                 ON CONFLICT(date) DO UPDATE SET {valid_field} = {valid_field} + 1"
            ),
            params![today],
        )?;
        Ok(())
    }

    /// Get today's stats.
    pub fn get_today_stats(&self) -> Result<DailyStats> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let result = self.conn.query_row(
            "SELECT reviews_created, branches_created, commits_made
             FROM daily_stats WHERE date = ?1",
            params![today],
            |row| {
                Ok(DailyStats {
                    reviews_created: row.get(0)?,
                    branches_created: row.get(1)?,
                    commits_made: row.get(2)?,
                })
            },
        );
        match result {
            Ok(stats) => Ok(stats),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(DailyStats {
                reviews_created: 0,
                branches_created: 0,
                commits_made: 0,
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Calculate the current consecutive usage streak (in days).
    pub fn calculate_streak(&self) -> Result<StreakInfo> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        let mut stmt = self
            .conn
            .prepare("SELECT date FROM daily_stats ORDER BY date DESC")?;
        let dates: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        if dates.is_empty() {
            return Ok(StreakInfo {
                consecutive_days: 0,
            });
        }

        let mut streak = 0u32;
        let mut expected = chrono::Local::now().date_naive();

        if dates.first().map(|d| d.as_str()) != Some(today.as_str()) {
            expected = expected.pred_opt().unwrap_or(expected);
        }

        for date_str in &dates {
            if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                if date == expected {
                    streak += 1;
                    expected = expected.pred_opt().unwrap_or(expected);
                } else if date < expected {
                    break;
                }
            }
        }

        Ok(StreakInfo {
            consecutive_days: streak,
        })
    }

    /// Start a new stats-tracking session. Returns the session ID.
    pub fn start_stats_session(&self) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO session_stats (id) VALUES (?1)",
            params![id],
        )?;
        Ok(id)
    }

    /// Increment a counter for the current stats session.
    pub fn increment_session_stat(&self, session_id: &str, field: &str) -> Result<()> {
        let valid_field = match field {
            "reviews_created" | "branches_created" | "commits_made" => field,
            _ => anyhow::bail!("invalid session stat field: {field}"),
        };
        self.conn.execute(
            &format!(
                "UPDATE session_stats SET {valid_field} = {valid_field} + 1 WHERE id = ?1"
            ),
            params![session_id],
        )?;
        Ok(())
    }

    /// End a stats session, recording the end time. Returns a snapshot.
    pub fn end_stats_session(&self, session_id: &str) -> Result<SessionStatsSnapshot> {
        self.conn.execute(
            "UPDATE session_stats SET ended_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;
        let snap = self.conn.query_row(
            "SELECT reviews_created, branches_created, commits_made
             FROM session_stats WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(SessionStatsSnapshot {
                    reviews_created: row.get(0)?,
                    branches_created: row.get(1)?,
                    commits_made: row.get(2)?,
                })
            },
        )?;
        Ok(snap)
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

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Convert a `rusqlite::Row` into a `ReviewComment`.
///
/// Expected column order (13 columns):
///   0:id, 1:worktree, 2:file_path, 3:line_start, 4:line_end,
///   5:kind, 6:body, 7:status, 8:commit_ref, 9:author, 10:branch,
///   11:created_at, 12:updated_at
fn row_to_review(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewComment> {
    let kind_str: String = row.get(5)?;
    let status_str: String = row.get(7)?;
    let author_str: String = row.get(9)?;

    let kind = match kind_str.as_str() {
        "suggest" => CommentKind::Suggest,
        "question" => CommentKind::Question,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                format!("unknown CommentKind: {other}").into(),
            ))
        }
    };

    let status = match status_str.as_str() {
        "pending" => CommentStatus::Pending,
        "resolved" => CommentStatus::Resolved,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                format!("unknown CommentStatus: {other}").into(),
            ))
        }
    };

    let author = match author_str.as_str() {
        "user" => Author::User,
        "claude" => Author::Claude,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                format!("unknown Author: {other}").into(),
            ))
        }
    };

    Ok(ReviewComment {
        id: row.get(0)?,
        worktree: row.get(1)?,
        file_path: row.get(2)?,
        line_start: row.get::<_, i64>(3)? as u32,
        line_end: row.get::<_, Option<i64>>(4)?.map(|n| n as u32),
        kind,
        body: row.get(6)?,
        status,
        commit_ref: row.get(8)?,
        author,
        branch: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// Execute a prepared statement and collect all matching rows into a `Vec<ReviewComment>`.
fn collect_reviews(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<ReviewComment>> {
    let rows = stmt.query_map(params, row_to_review)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory ReviewStore for testing.
    fn test_store() -> ReviewStore {
        ReviewStore::open(Path::new(":memory:")).expect("open in-memory DB")
    }

    #[test]
    fn add_and_retrieve_review() {
        let store = test_store();

        let review = store
            .add_review("wt1", "src/main.rs", 42, None, CommentKind::Suggest, "use guard clause", "abc123", Author::User, None)
            .unwrap();

        assert_eq!(review.worktree, "wt1");
        assert_eq!(review.file_path, "src/main.rs");
        assert_eq!(review.line_start, 42);
        assert_eq!(review.line_end, None);
        assert_eq!(review.kind, CommentKind::Suggest);
        assert_eq!(review.body, "use guard clause");
        assert_eq!(review.status, CommentStatus::Pending);
        assert_eq!(review.commit_ref, "abc123");
        assert_eq!(review.author, Author::User);
        assert_eq!(review.branch, None);

        // Retrieve by worktree
        let reviews = store.reviews_for_worktree("wt1").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].id, review.id);

        // Retrieve by file
        let reviews = store.reviews_for_file("wt1", "src/main.rs").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].id, review.id);

        // No reviews for a different file
        let reviews = store.reviews_for_file("wt1", "src/lib.rs").unwrap();
        assert!(reviews.is_empty());
    }

    #[test]
    fn update_body() {
        let store = test_store();

        let review = store
            .add_review("wt1", "src/app.rs", 5, None, CommentKind::Suggest, "original", "abc", Author::User, None)
            .unwrap();

        store.update_review_body(&review.id, "edited").unwrap();
        let reviews = store.reviews_for_worktree("wt1").unwrap();
        assert_eq!(reviews[0].body, "edited");
    }

    #[test]
    fn db_path_creates_directory() {
        let tmp = std::env::temp_dir().join("conductor_test_db_path");
        // Clean up from any previous run
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let path = db_path(&tmp);
        assert_eq!(path, tmp.join(".conductor").join("conductor.db"));
        assert!(tmp.join(".conductor").is_dir());

        // Clean up
        let _ = std::fs::remove_dir_all(&tmp);
    }

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
            .save_session_history("sess-3", "wt2", "CC:2", "claude_code", "Error: file not found")
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

    #[test]
    fn line_range_and_author() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1", "src/main.rs", 10, Some(20),
                CommentKind::Suggest, "refactor this block",
                "abc", Author::Claude, Some("feature/x"),
            )
            .unwrap();

        assert_eq!(review.line_start, 10);
        assert_eq!(review.line_end, Some(20));
        assert_eq!(review.author, Author::Claude);
        assert_eq!(review.branch.as_deref(), Some("feature/x"));

        // Single-line (line_end = None)
        let r2 = store
            .add_review(
                "wt1", "src/main.rs", 5, None,
                CommentKind::Question, "why?",
                "abc", Author::User, None,
            )
            .unwrap();
        assert_eq!(r2.line_start, 5);
        assert_eq!(r2.line_end, None);
        assert_eq!(r2.author, Author::User);
        assert_eq!(r2.branch, None);
    }

    #[test]
    fn add_and_get_replies() {
        let store = test_store();

        let review = store
            .add_review("wt1", "src/main.rs", 42, None, CommentKind::Suggest, "fix this", "abc", Author::Claude, None)
            .unwrap();

        // Initially no replies.
        let replies = store.get_replies(&review.id).unwrap();
        assert!(replies.is_empty());

        let counts = store.reply_counts_for_worktree("wt1").unwrap();
        assert!(counts.is_empty());

        // Add a user reply.
        store.add_reply(&review.id, "I'll fix it", Author::User).unwrap();

        let replies = store.get_replies(&review.id).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body, "I'll fix it");
        assert_eq!(replies[0].author, Author::User);
        assert_eq!(replies[0].review_id, review.id);

        // Add another reply (from Claude).
        store.add_reply(&review.id, "Thanks!", Author::Claude).unwrap();

        let replies = store.get_replies(&review.id).unwrap();
        assert_eq!(replies.len(), 2);

        // Check counts.
        let counts = store.reply_counts_for_worktree("wt1").unwrap();
        assert_eq!(counts.get(&review.id), Some(&2));

        // No replies for a different worktree.
        let counts = store.reply_counts_for_worktree("wt2").unwrap();
        assert!(counts.is_empty());
    }

    #[test]
    fn replies_cascade_delete() {
        let store = test_store();

        let review = store
            .add_review("wt1", "src/app.rs", 10, None, CommentKind::Question, "why?", "abc", Author::User, None)
            .unwrap();

        store.add_reply(&review.id, "because reasons", Author::Claude).unwrap();
        assert_eq!(store.get_replies(&review.id).unwrap().len(), 1);

        // Deleting the review should cascade-delete the replies.
        store.delete_review(&review.id).unwrap();
        let replies = store.get_replies(&review.id).unwrap();
        assert!(replies.is_empty());
    }

    // ── Gamification: daily stats ─────────────────────────────────

    #[test]
    fn daily_stats_increment_and_streak() {
        let store = test_store();
        store.increment_daily_stat("reviews_created").unwrap();
        store.increment_daily_stat("reviews_created").unwrap();
        store.increment_daily_stat("branches_created").unwrap();

        let today = store.get_today_stats().unwrap();
        assert_eq!(today.reviews_created, 2);
        assert_eq!(today.branches_created, 1);
        assert_eq!(today.commits_made, 0);

        let streak = store.calculate_streak().unwrap();
        assert_eq!(streak.consecutive_days, 1);
    }

    #[test]
    fn daily_stats_invalid_field_rejected() {
        let store = test_store();
        assert!(store.increment_daily_stat("invalid_field").is_err());
        assert!(store.increment_daily_stat("").is_err());
        assert!(store.increment_daily_stat("reviews_created; DROP TABLE daily_stats").is_err());
    }

    #[test]
    fn daily_stats_all_fields_increment_independently() {
        let store = test_store();
        store.increment_daily_stat("reviews_created").unwrap();
        store.increment_daily_stat("branches_created").unwrap();
        store.increment_daily_stat("commits_made").unwrap();
        store.increment_daily_stat("sessions_used").unwrap();

        let stats = store.get_today_stats().unwrap();
        assert_eq!(stats.reviews_created, 1);
        assert_eq!(stats.branches_created, 1);
        assert_eq!(stats.commits_made, 1);
    }

    #[test]
    fn get_today_stats_returns_zeros_when_empty() {
        let store = test_store();
        let stats = store.get_today_stats().unwrap();
        assert_eq!(stats.reviews_created, 0);
        assert_eq!(stats.branches_created, 0);
        assert_eq!(stats.commits_made, 0);
    }

    // ── Gamification: streak calculation ────────────────────────

    #[test]
    fn streak_zero_when_no_activity() {
        let store = test_store();
        let streak = store.calculate_streak().unwrap();
        assert_eq!(streak.consecutive_days, 0);
    }

    #[test]
    fn streak_counts_consecutive_past_days() {
        let store = test_store();
        let today = chrono::Local::now().date_naive();

        // Insert activity for today and the previous 4 days.
        for i in 0..5 {
            let date = today - chrono::Duration::days(i);
            store.conn.execute(
                "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
                rusqlite::params![date.format("%Y-%m-%d").to_string()],
            ).unwrap();
        }

        let streak = store.calculate_streak().unwrap();
        assert_eq!(streak.consecutive_days, 5);
    }

    #[test]
    fn streak_breaks_on_gap() {
        let store = test_store();
        let today = chrono::Local::now().date_naive();

        // Today and yesterday have activity.
        for i in 0..2 {
            let date = today - chrono::Duration::days(i);
            store.conn.execute(
                "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
                rusqlite::params![date.format("%Y-%m-%d").to_string()],
            ).unwrap();
        }
        // Skip day -2, add day -3 (should not count).
        let old_date = today - chrono::Duration::days(3);
        store.conn.execute(
            "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
            rusqlite::params![old_date.format("%Y-%m-%d").to_string()],
        ).unwrap();

        let streak = store.calculate_streak().unwrap();
        assert_eq!(streak.consecutive_days, 2);
    }

    #[test]
    fn streak_starts_from_yesterday_if_no_today() {
        let store = test_store();
        let today = chrono::Local::now().date_naive();

        // Activity only yesterday and the day before — no today.
        for i in 1..3 {
            let date = today - chrono::Duration::days(i);
            store.conn.execute(
                "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
                rusqlite::params![date.format("%Y-%m-%d").to_string()],
            ).unwrap();
        }

        let streak = store.calculate_streak().unwrap();
        assert_eq!(streak.consecutive_days, 2);
    }

    // ── Gamification: session stats ─────────────────────────────

    #[test]
    fn session_stats_lifecycle() {
        let store = test_store();
        let sid = store.start_stats_session().unwrap();
        store.increment_session_stat(&sid, "reviews_created").unwrap();
        store.increment_session_stat(&sid, "commits_made").unwrap();
        store.increment_session_stat(&sid, "commits_made").unwrap();
        let snap = store.end_stats_session(&sid).unwrap();
        assert_eq!(snap.reviews_created, 1);
        assert_eq!(snap.commits_made, 2);
    }

    #[test]
    fn session_stats_invalid_field_rejected() {
        let store = test_store();
        let sid = store.start_stats_session().unwrap();
        // "sessions_used" is valid for daily but NOT for session stats.
        assert!(store.increment_session_stat(&sid, "sessions_used").is_err());
        assert!(store.increment_session_stat(&sid, "bogus").is_err());
    }

    #[test]
    fn session_stats_end_with_zero_counts() {
        let store = test_store();
        let sid = store.start_stats_session().unwrap();
        let snap = store.end_stats_session(&sid).unwrap();
        assert_eq!(snap.reviews_created, 0);
        assert_eq!(snap.branches_created, 0);
        assert_eq!(snap.commits_made, 0);
    }

    #[test]
    fn multiple_sessions_are_independent() {
        let store = test_store();
        let s1 = store.start_stats_session().unwrap();
        let s2 = store.start_stats_session().unwrap();

        store.increment_session_stat(&s1, "reviews_created").unwrap();
        store.increment_session_stat(&s2, "commits_made").unwrap();
        store.increment_session_stat(&s2, "commits_made").unwrap();

        let snap1 = store.end_stats_session(&s1).unwrap();
        let snap2 = store.end_stats_session(&s2).unwrap();

        assert_eq!(snap1.reviews_created, 1);
        assert_eq!(snap1.commits_made, 0);
        assert_eq!(snap2.reviews_created, 0);
        assert_eq!(snap2.commits_made, 2);
    }
}
