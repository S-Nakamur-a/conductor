//! Gamification: daily activity stats (`daily_stats`), consecutive-day
//! streak calculation, and per-session stats (`session_stats`).

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::{DailyStats, SessionStatsSnapshot, StreakInfo};

impl ReviewStore {
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
        self.conn
            .execute("INSERT INTO session_stats (id) VALUES (?1)", params![id])?;
        Ok(id)
    }

    /// Increment a counter for the current stats session.
    pub fn increment_session_stat(&self, session_id: &str, field: &str) -> Result<()> {
        let valid_field = match field {
            "reviews_created" | "branches_created" | "commits_made" => field,
            _ => anyhow::bail!("invalid session stat field: {field}"),
        };
        self.conn.execute(
            &format!("UPDATE session_stats SET {valid_field} = {valid_field} + 1 WHERE id = ?1"),
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
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;

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
        assert!(
            store
                .increment_daily_stat("reviews_created; DROP TABLE daily_stats")
                .is_err()
        );
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
            store
                .conn
                .execute(
                    "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
                    rusqlite::params![date.format("%Y-%m-%d").to_string()],
                )
                .unwrap();
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
            store
                .conn
                .execute(
                    "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
                    rusqlite::params![date.format("%Y-%m-%d").to_string()],
                )
                .unwrap();
        }
        // Skip day -2, add day -3 (should not count).
        let old_date = today - chrono::Duration::days(3);
        store
            .conn
            .execute(
                "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
                rusqlite::params![old_date.format("%Y-%m-%d").to_string()],
            )
            .unwrap();

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
            store
                .conn
                .execute(
                    "INSERT INTO daily_stats (date, reviews_created) VALUES (?1, 1)",
                    rusqlite::params![date.format("%Y-%m-%d").to_string()],
                )
                .unwrap();
        }

        let streak = store.calculate_streak().unwrap();
        assert_eq!(streak.consecutive_days, 2);
    }

    // ── Gamification: session stats ─────────────────────────────

    #[test]
    fn session_stats_lifecycle() {
        let store = test_store();
        let sid = store.start_stats_session().unwrap();
        store
            .increment_session_stat(&sid, "reviews_created")
            .unwrap();
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

        store
            .increment_session_stat(&s1, "reviews_created")
            .unwrap();
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
