//! AI walkthrough generation lifecycle: start, save, fail, and fetch a
//! branch's walkthrough (the `walkthroughs` and `walkthrough_steps` tables).

use anyhow::Result;
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

    /// Save a completed walkthrough for `branch`: upserts the walkthrough
    /// row and replaces its steps, in one transaction. This is production's
    /// write path — the conductor `mcp-serve` binary's `save_walkthrough`
    /// tool calls this method directly (in-process, no separate
    /// implementation to keep in sync). `begin_walkthrough` is no longer a
    /// prerequisite: calling this on a branch with no prior walkthrough
    /// creates one, and calling it again replaces the previous one entirely
    /// (matching the "no generation history" model described on the v6
    /// migration).
    ///
    /// Returns the walkthrough's id — on an upsert that hit an existing row
    /// this is that row's id, not the one generated here, so the caller can
    /// report the id actually in effect.
    ///
    /// `summary` is written twice on purpose: onto the walkthrough row (for
    /// round-trip fidelity) and into `change_summary`, which backs the SUMMARY
    /// pseudo-file. Generating a walkthrough is the only thing that writes that
    /// table, so the two must land together — hence inside the same
    /// transaction, so a failed save leaves no summary describing a walkthrough
    /// that was never stored.
    pub fn save_walkthrough(
        &self,
        branch: &str,
        title: &str,
        summary: &str,
        steps: &[NewWalkthroughStep],
    ) -> Result<String> {
        let candidate_id = Uuid::new_v4().to_string();

        // BEGIN IMMEDIATE (not a bare BEGIN) takes the write lock up front.
        // Under WAL, a deferred transaction that reads before it writes can
        // get SQLITE_BUSY_SNAPSHOT instead of SQLITE_BUSY on a concurrent
        // writer — and busy_timeout's retry handler only fires for the
        // latter, so a deferred transaction here could fail instead of
        // waiting.
        self.conn.execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| -> Result<String> {
            self.conn.execute(
                "INSERT INTO walkthroughs (id, branch, title, summary, status, error, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'ready', NULL,
                         COALESCE((SELECT created_at FROM walkthroughs WHERE branch = ?5), datetime('now')),
                         datetime('now'))
                 ON CONFLICT(branch) DO UPDATE SET
                     title = excluded.title, summary = excluded.summary,
                     status = 'ready', error = NULL, updated_at = datetime('now')",
                params![candidate_id, branch, title, summary, branch],
            )?;
            self.save_change_summary(branch, summary, Author::Claude)?;

            // On conflict the INSERT keeps the existing row's id rather than
            // `candidate_id`, so re-read the id actually in effect instead of
            // assuming the one just generated.
            let walkthrough_id: String = self.conn.query_row(
                "SELECT id FROM walkthroughs WHERE branch = ?1",
                params![branch],
                |row| row.get(0),
            )?;

            self.conn.execute(
                "DELETE FROM walkthrough_steps WHERE walkthrough_id = ?1",
                params![walkthrough_id],
            )?;
            // `seq` comes from the slice's order, not from anything the caller
            // supplied: the MCP tool accepts a per-step `seq`, and a model that
            // numbers steps within each kind (intent 0,1 / core 0,1,2 / …) would
            // otherwise interleave the whole tour — rendered perfectly, reported
            // as success, with nothing to indicate the narrative order was lost.
            // Deriving it here also keeps `seq` dense and unique per walkthrough,
            // so `get_walkthrough`'s `ORDER BY seq` needs no tie-break.
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
            Ok(walkthrough_id)
        })();

        match result {
            // A failing COMMIT still leaves the transaction open, so it needs
            // the same rollback as a failing statement: otherwise every later
            // write on this connection joins the stranded transaction, reports
            // success, and is discarded when the process exits.
            Ok(walkthrough_id) => match self.conn.execute_batch("COMMIT;") {
                Ok(()) => Ok(walkthrough_id),
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK;");
                    Err(e.into())
                }
            },
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
            anyhow::bail!("no walkthrough row for branch {branch} — call begin_walkthrough first");
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

        // `seq` is assigned from the slice's order on write, so it is dense and
        // unique within a walkthrough — `ORDER BY seq` alone is total here, and
        // needs no tie-break. (`, id` would actively hurt: step ids are random
        // UUIDs, so tie-breaking on them would order steps at random rather
        // than by the order they were saved in.)
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
            .save_walkthrough(
                "feat/x",
                "Fix startup crash",
                "A short summary.",
                &new_steps,
            )
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

        store
            .fail_walkthrough("feat/x", "Claude Code exited early")
            .unwrap();
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

    /// `save_walkthrough` no longer requires `begin_walkthrough` — it upserts
    /// the walkthrough row itself, so it's a valid entry point on its own
    /// (this is what the `mcp-serve` tool calls directly).
    #[test]
    fn save_walkthrough_without_begin_upserts() {
        let store = test_store();
        assert!(store.get_walkthrough("feat/x").unwrap().is_none());

        store
            .save_walkthrough("feat/x", "title", "summary", &[])
            .unwrap();

        let (walkthrough, _) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Ready);
        assert_eq!(walkthrough.title.as_deref(), Some("title"));
    }

    /// Saving twice must replace the steps outright (the `DELETE` +
    /// re-`INSERT` inside the transaction, backed by the CASCADE on
    /// `walkthrough_steps.walkthrough_id`), not append to them.
    #[test]
    fn save_walkthrough_replaces_previous_steps() {
        let store = test_store();

        store
            .save_walkthrough(
                "feat/x",
                "title",
                "summary",
                &[NewWalkthroughStep {
                    file_path: "src/old.rs".to_string(),
                    line_start: None,
                    line_end: None,
                    kind: WalkthroughStepKind::Intent,
                    title: "First pass".to_string(),
                    body: "Old body.".to_string(),
                }],
            )
            .unwrap();

        store
            .save_walkthrough(
                "feat/x",
                "title",
                "summary",
                &[NewWalkthroughStep {
                    file_path: "src/new.rs".to_string(),
                    line_start: None,
                    line_end: None,
                    kind: WalkthroughStepKind::Core,
                    title: "Second pass".to_string(),
                    body: "New body.".to_string(),
                }],
            )
            .unwrap();

        let (_, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].file_path, "src/new.rs");
        assert_eq!(steps[0].kind, WalkthroughStepKind::Core);
        assert_eq!(steps[0].body, "New body.");
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

    /// Proves the transaction-safety claim in `save_walkthrough`'s doc
    /// comment: a step insert that fails must roll back the walkthrough row
    /// and the change summary written alongside it in the same transaction,
    /// not leave a summary describing a walkthrough that was never actually
    /// stored. The failure is injected with a trigger rather than a bad
    /// argument, since every argument shape `save_walkthrough` itself would
    /// reject is already caught before any write happens.
    #[test]
    fn failed_step_insert_leaves_no_change_summary() {
        let store = test_store();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER boom BEFORE INSERT ON walkthrough_steps
                 BEGIN SELECT RAISE(ABORT, 'boom'); END",
            )
            .unwrap();

        let steps = vec![NewWalkthroughStep {
            file_path: "src/main.rs".to_string(),
            line_start: None,
            line_end: None,
            kind: WalkthroughStepKind::Intent,
            title: "won't be saved".to_string(),
            body: "the trigger aborts before this lands".to_string(),
        }];
        assert!(
            store
                .save_walkthrough("feat/x", "t", "summary", &steps)
                .is_err()
        );

        // ROLLBACK undoes both the walkthrough upsert and the change summary
        // write, not just the step insert that actually failed.
        assert_eq!(store.get_change_summary("feat/x").unwrap(), None);
        assert!(store.get_walkthrough("feat/x").unwrap().is_none());
    }

    /// The slice's order is the walkthrough's order, and `seq` is derived from
    /// it — so steps always come back in the order they were handed over, with
    /// a dense `0..n`. This is what stops a caller that numbers steps per-kind
    /// from silently interleaving the tour.
    #[test]
    fn save_walkthrough_numbers_steps_by_slice_order() {
        let store = test_store();
        let steps = vec![
            NewWalkthroughStep {
                file_path: "src/a.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Intent,
                title: "a".to_string(),
                body: "a".to_string(),
            },
            NewWalkthroughStep {
                file_path: "src/b.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Core,
                title: "b".to_string(),
                body: "b".to_string(),
            },
            NewWalkthroughStep {
                file_path: "src/c.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Ripple,
                title: "c".to_string(),
                body: "c".to_string(),
            },
        ];
        store
            .save_walkthrough("feat/x", "title", "summary", &steps)
            .unwrap();

        let (_, loaded) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(
            loaded.iter().map(|s| s.seq).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "seq must be dense and follow the slice order"
        );
        assert_eq!(
            loaded
                .iter()
                .map(|s| s.file_path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs", "src/c.rs"],
            "steps must come back in the order they were supplied"
        );
    }

    /// `WalkthroughStepKind::as_str()` and the schema's
    /// `CHECK (kind IN ('intent','core','ripple','test'))` are two separately
    /// written string lists; if they ever drift apart, only the kinds absent
    /// from the CHECK fail — and only at save time, not at compile time. This
    /// exercises all four so such a drift is caught immediately.
    #[test]
    fn save_and_load_round_trips_every_step_kind() {
        let store = test_store();
        let kinds = [
            WalkthroughStepKind::Intent,
            WalkthroughStepKind::Core,
            WalkthroughStepKind::Ripple,
            WalkthroughStepKind::Test,
        ];
        let steps: Vec<NewWalkthroughStep> = kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| NewWalkthroughStep {
                file_path: format!("src/{i}.rs"),
                line_start: None,
                line_end: None,
                kind: *kind,
                title: format!("step {i}"),
                body: format!("body {i}"),
            })
            .collect();
        store
            .save_walkthrough("feat/x", "title", "summary", &steps)
            .unwrap();

        let (_, loaded) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(loaded.len(), kinds.len());
        for (step, kind) in loaded.iter().zip(kinds.iter()) {
            assert_eq!(step.kind, *kind);
            // Round trip through the string form too, not just the enum
            // value already deserialized off the row.
            assert_eq!(WalkthroughStepKind::from_str(kind.as_str()), Some(*kind));
        }
    }
}
