//! AI walkthrough generation lifecycle: start, save, fail, and fetch a
//! branch's walkthrough (the `walkthroughs` and `walkthrough_steps` tables).

use anyhow::{Context, Result};
use rusqlite::params;
use uuid::Uuid;

use crate::walkthrough::{
    NewWalkthroughStep, Walkthrough, WalkthroughStatus, WalkthroughStep, WalkthroughStepKind,
};

use super::{Author, ReviewStore};

impl ReviewStore {
    /// Start (or restart) walkthrough generation for a branch: delete any
    /// existing walkthrough for it (no generation history is kept — see the
    /// v6 migration note) and insert a fresh `generating` row, so the caller
    /// has an id to poll for completion / detect a stuck generation.
    /// Start a fresh walkthrough for `branch`, recording the branch tip
    /// (`head_commit`, the HEAD commit OID) it's being generated against so a
    /// later same-commit regenerate can be skipped. Pass `None` when the tip
    /// is unknown.
    pub fn begin_walkthrough(
        &self,
        branch: &str,
        head_commit: Option<&str>,
    ) -> Result<Walkthrough> {
        self.conn.execute(
            "DELETE FROM walkthroughs WHERE branch = ?1",
            params![branch],
        )?;
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO walkthroughs (id, branch, status, head_commit)
             VALUES (?1, ?2, 'generating', ?3)",
            params![id, branch, head_commit],
        )?;
        self.walkthrough_row_by_id(&id)
    }

    /// Save a completed walkthrough: replaces the branch's steps and marks it
    /// `ready`, in one transaction. `begin_walkthrough` must have already
    /// created the row — this only updates/populates it, so a generation
    /// that skipped the "generating" placeholder is treated as an error
    /// rather than silently creating one (the placeholder is what a stuck- or
    /// failed-generation UI depends on existing).
    ///
    /// `summary` is written twice on purpose: onto the walkthrough row (for
    /// round-trip fidelity) and into `change_summary`, which backs the SUMMARY
    /// pseudo-file. Generating a walkthrough is the only thing that writes that
    /// table, so the two must land together — hence inside the same
    /// transaction, so a failed save leaves no summary describing a walkthrough
    /// that was never stored.
    ///
    /// Not called in production: the actual write path is the conductor MCP
    /// server's `save_walkthrough` tool (`plugins/conductor/mcp/conductor-comment/src/index.ts`),
    /// which the headless `claude -p` session invokes directly over stdio —
    /// this Rust method exists only so the save round-trip can be tested
    /// without spawning a Node process. The two implementations must be kept
    /// in sync by hand: a schema or invariant change here needs the same
    /// change made in `index.ts`, and vice versa.
    #[allow(dead_code)]
    pub fn save_walkthrough(
        &self,
        branch: &str,
        title: &str,
        summary: &str,
        steps: &[NewWalkthroughStep],
    ) -> Result<()> {
        let walkthrough_id: String = self
            .conn
            .query_row(
                "SELECT id FROM walkthroughs WHERE branch = ?1",
                params![branch],
                |row| row.get(0),
            )
            .with_context(|| {
                format!("no walkthrough row for branch {branch} — call begin_walkthrough first")
            })?;

        self.conn.execute_batch("BEGIN;")?;
        let result = (|| -> Result<()> {
            self.conn.execute(
                "UPDATE walkthroughs
                 SET title = ?1, summary = ?2, status = 'ready', error = NULL,
                     updated_at = datetime('now')
                 WHERE id = ?3",
                params![title, summary, walkthrough_id],
            )?;
            self.save_change_summary(branch, summary, Author::Claude)?;
            self.conn.execute(
                "DELETE FROM walkthrough_steps WHERE walkthrough_id = ?1",
                params![walkthrough_id],
            )?;
            for (seq, step) in steps.iter().enumerate() {
                let step_id = Uuid::new_v4().to_string();
                self.conn.execute(
                    "INSERT INTO walkthrough_steps
                        (id, walkthrough_id, seq, file_path, line_start, line_end, kind, title, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        step_id,
                        walkthrough_id,
                        seq as i64,
                        step.file_path,
                        step.line_start,
                        step.line_end,
                        step.kind.as_str(),
                        step.title,
                        step.body,
                    ],
                )?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Mark a branch's walkthrough as failed, recording why. Requires
    /// `begin_walkthrough` to have created the row first.
    pub fn fail_walkthrough(&self, branch: &str, error: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE walkthroughs
             SET status = 'failed', error = ?1, updated_at = datetime('now')
             WHERE branch = ?2",
            params![error, branch],
        )?;
        if changed == 0 {
            anyhow::bail!(
                "no walkthrough row for branch {branch} — call begin_walkthrough first"
            );
        }
        Ok(())
    }

    /// Retrieve a branch's walkthrough header and its steps (ordered by
    /// `seq`), or `None` if no walkthrough has been started for it.
    pub fn get_walkthrough(
        &self,
        branch: &str,
    ) -> Result<Option<(Walkthrough, Vec<WalkthroughStep>)>> {
        let walkthrough = match self.conn.query_row(
            "SELECT id, branch, title, summary, status, error, created_at, updated_at, head_commit
             FROM walkthroughs WHERE branch = ?1",
            params![branch],
            row_to_walkthrough,
        ) {
            Ok(w) => w,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let mut stmt = self.conn.prepare(
            "SELECT id, walkthrough_id, seq, file_path, line_start, line_end, kind, title, body
             FROM walkthrough_steps
             WHERE walkthrough_id = ?1
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![walkthrough.id], row_to_walkthrough_step)?;
        let mut steps = Vec::new();
        for row in rows {
            steps.push(row?);
        }
        Ok(Some((walkthrough, steps)))
    }

    /// Fetch a walkthrough row by id (used right after `begin_walkthrough`
    /// inserts it, to read back server-side defaults like `created_at`).
    fn walkthrough_row_by_id(&self, id: &str) -> Result<Walkthrough> {
        self.conn
            .query_row(
                "SELECT id, branch, title, summary, status, error, created_at, updated_at, head_commit
                 FROM walkthroughs WHERE id = ?1",
                params![id],
                row_to_walkthrough,
            )
            .map_err(Into::into)
    }
}

fn row_to_walkthrough(row: &rusqlite::Row<'_>) -> rusqlite::Result<Walkthrough> {
    let status_str: String = row.get(4)?;
    let status = WalkthroughStatus::from_str(&status_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            format!("unknown WalkthroughStatus: {status_str}").into(),
        )
    })?;

    Ok(Walkthrough {
        id: row.get(0)?,
        branch: row.get(1)?,
        title: row.get(2)?,
        summary: row.get(3)?,
        status,
        error: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        head_commit: row.get(8)?,
    })
}

fn row_to_walkthrough_step(row: &rusqlite::Row<'_>) -> rusqlite::Result<WalkthroughStep> {
    let kind_str: String = row.get(6)?;
    let kind = WalkthroughStepKind::from_str(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            format!("unknown WalkthroughStepKind: {kind_str}").into(),
        )
    })?;

    Ok(WalkthroughStep {
        id: row.get(0)?,
        walkthrough_id: row.get(1)?,
        seq: row.get(2)?,
        file_path: row.get(3)?,
        line_start: row.get(4)?,
        line_end: row.get(5)?,
        kind,
        title: row.get(7)?,
        body: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;
    use super::*;

    #[test]
    fn walkthrough_lifecycle() {
        let store = test_store();

        // No walkthrough yet.
        assert!(store.get_walkthrough("feat/x").unwrap().is_none());

        let started = store.begin_walkthrough("feat/x", Some("abc1234")).unwrap();
        assert_eq!(started.branch, "feat/x");
        assert_eq!(started.status, WalkthroughStatus::Generating);
        // The branch tip is recorded so a same-commit regenerate can be skipped.
        assert_eq!(started.head_commit.as_deref(), Some("abc1234"));

        let (walkthrough, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.id, started.id);
        assert_eq!(walkthrough.status, WalkthroughStatus::Generating);
        assert_eq!(walkthrough.head_commit.as_deref(), Some("abc1234"));
        assert!(steps.is_empty());

        let new_steps = vec![
            NewWalkthroughStep {
                file_path: "src/main.rs".to_string(),
                line_start: Some(10),
                line_end: Some(20),
                kind: WalkthroughStepKind::Intent,
                title: "Why this change exists".to_string(),
                body: "Fixes a startup crash.".to_string(),
            },
            NewWalkthroughStep {
                file_path: "src/lib.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Core,
                title: "Core fix".to_string(),
                body: "Guards against the null case.".to_string(),
            },
        ];
        store
            .save_walkthrough("feat/x", "Fix startup crash", "A short summary.", &new_steps)
            .unwrap();

        let (walkthrough, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Ready);
        assert_eq!(walkthrough.title.as_deref(), Some("Fix startup crash"));
        assert_eq!(walkthrough.summary.as_deref(), Some("A short summary."));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].seq, 0);
        assert_eq!(steps[0].file_path, "src/main.rs");
        assert_eq!(steps[0].kind, WalkthroughStepKind::Intent);
        assert_eq!(steps[1].seq, 1);
        assert_eq!(steps[1].kind, WalkthroughStepKind::Core);

        // Re-generating replaces the row entirely (no history kept); passing
        // no tip leaves head_commit null.
        let restarted = store.begin_walkthrough("feat/x", None).unwrap();
        assert_ne!(restarted.id, started.id);
        assert_eq!(restarted.head_commit, None);
        let (walkthrough, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Generating);
        assert!(steps.is_empty());

        store.fail_walkthrough("feat/x", "Claude Code exited early").unwrap();
        let (walkthrough, _) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Failed);
        assert_eq!(
            walkthrough.error.as_deref(),
            Some("Claude Code exited early")
        );
    }

    #[test]
    fn fail_walkthrough_without_begin_is_an_error() {
        let store = test_store();
        assert!(store.fail_walkthrough("feat/x", "boom").is_err());
    }

    #[test]
    fn save_walkthrough_without_begin_is_an_error() {
        let store = test_store();
        assert!(
            store
                .save_walkthrough("feat/x", "title", "summary", &[])
                .is_err()
        );
    }

    /// The walkthrough's `summary` is also the branch's change summary — the
    /// SUMMARY pseudo-file's content. Generating a walkthrough is the only
    /// thing that writes it, so if this link breaks the SUMMARY pane silently
    /// stays empty forever.
    #[test]
    fn save_walkthrough_also_writes_the_change_summary() {
        let store = test_store();
        store.begin_walkthrough("feat/x", None).unwrap();
        assert_eq!(store.get_change_summary("feat/x").unwrap(), None);

        store
            .save_walkthrough("feat/x", "Fix startup crash", "何をなぜ変えたか。", &[])
            .unwrap();

        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("何をなぜ変えたか。")
        );

        // Re-generating replaces it, so the pane always shows the latest
        // overview rather than accumulating stale ones.
        store.begin_walkthrough("feat/x", None).unwrap();
        store
            .save_walkthrough("feat/x", "Fix startup crash", "更新後の概要。", &[])
            .unwrap();
        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("更新後の概要。")
        );
    }

    /// A save rejected for want of `begin_walkthrough` must not leave a change
    /// summary behind describing a walkthrough that was never stored. Note this
    /// covers only the pre-transaction guard — a failure *inside* the
    /// transaction is handled by the surrounding BEGIN/ROLLBACK and can't be
    /// provoked through this API, so it isn't asserted here.
    #[test]
    fn save_walkthrough_rejected_before_begin_writes_no_change_summary() {
        let store = test_store();
        assert!(
            store
                .save_walkthrough("feat/x", "title", "summary", &[])
                .is_err()
        );
        assert_eq!(store.get_change_summary("feat/x").unwrap(), None);
    }
}
