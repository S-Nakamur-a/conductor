//! AI walkthrough generation orchestration for [`App`].
//!
//! Runs [`crate::walkthrough::generate`] on a background thread — which asks
//! whichever model `[api]` names, never a `claude` process Conductor spawns
//! itself — and reflects the result into the review database via
//! [`crate::review_store::ReviewStore`].
//!
//! One generation may be in flight per branch ([`WalkthroughGenerations`]), so
//! a reviewer touring one worktree can still start a walkthrough in another.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use super::*;

/// An in-flight generation: where its result will arrive, which branch it is
/// for, and the flag that stops it.
pub struct WalkthroughGeneration {
    pub branch: String,
    result: Receiver<Result<crate::walkthrough::Generated, String>>,
    cancel: Arc<AtomicBool>,
}

impl WalkthroughGeneration {
    /// Signal the worker to stop. The AI caller checks this between polls of
    /// its child, so an external command is killed rather than left running.
    fn abort(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A generation that stopped running, as handed back by
/// [`WalkthroughGenerations::take_finished`]. Carries the branch it was for,
/// because the handle itself is gone by the time the caller reconciles the
/// database row it left behind.
pub struct FinishedGeneration {
    pub branch: String,
    pub outcome: Result<crate::walkthrough::Generated, String>,
}

/// Every walkthrough generation in flight in this Conductor instance, at most
/// one per branch.
///
/// Keyed by branch, not by "one at a time", because the branch is the only
/// thing a generation actually contends for: `begin_walkthrough` deletes and
/// re-inserts the `walkthroughs` row for its branch and `save_walkthrough`
/// replaces it, so two generations on one branch would race for a single row
/// and the loser's steps would vanish. Generations for *different* branches —
/// which means different worktrees, since git won't check one branch out
/// twice — touch disjoint rows and a database that is already WAL +
/// `busy_timeout` (see `review_store::schema`), so they can run side by side.
/// Serializing them was over-broad: it made a reviewer touring one worktree
/// unable to start a walkthrough in another.
#[derive(Default)]
pub struct WalkthroughGenerations {
    by_branch: HashMap<String, WalkthroughGeneration>,
}

impl WalkthroughGenerations {
    /// Whether a generation for `branch` is currently in flight.
    pub fn is_generating(&self, branch: &str) -> bool {
        self.by_branch.contains_key(branch)
    }

    /// Register a freshly spawned generation. The caller is expected to have
    /// checked [`Self::is_generating`] first: inserting over a live handle
    /// would drop its receiver, so the worker's result would go nowhere and
    /// strand that branch's row in `generating` forever.
    pub fn insert(&mut self, generation: WalkthroughGeneration) {
        debug_assert!(
            !self.is_generating(&generation.branch),
            "would orphan the in-flight generation for {}",
            generation.branch
        );
        self.by_branch
            .insert(generation.branch.clone(), generation);
    }

    /// Drain every in-flight generation's result channel, removing the ones
    /// that are no longer running and returning what each one produced.
    ///
    /// Removal is what makes a dead worker self-healing: a thread that panicked
    /// or dropped its sender without a result releases its slot here, so the
    /// next request for that branch starts a fresh generation instead of being
    /// told one is already running.
    pub fn take_finished(&mut self) -> Vec<FinishedGeneration> {
        let mut finished = Vec::new();
        self.by_branch.retain(|branch, generation| {
            let outcome = match generation.result.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Empty) => return true,
                // The worker died without sending: treat it as a failure rather
                // than leaving the row in `generating` forever.
                Err(TryRecvError::Disconnected) => {
                    Err("walkthrough generation ended without a result".to_string())
                }
            };
            finished.push(FinishedGeneration {
                branch: branch.clone(),
                outcome,
            });
            false
        });
        finished
    }

    /// Whether nothing is in flight (lets the caller skip polling entirely).
    pub fn is_empty(&self) -> bool {
        self.by_branch.is_empty()
    }

    /// Stop every in-flight generation (used when the app shuts down).
    pub fn abort_all(&mut self) {
        for (_, generation) in self.by_branch.drain() {
            generation.abort();
        }
    }
}

impl App {
    /// Kick off walkthrough generation for the selected worktree's branch:
    /// insert the `generating` row, then start the background worker.
    /// Re-running regenerates from scratch — except when a `ready` walkthrough
    /// already exists for the current branch tip, which is a no-op that just
    /// re-shows it (the diff, and so the walkthrough, hasn't changed). While a
    /// generation for *this* branch is already in flight it's a no-op with a
    /// status hint; other branches' generations are unaffected and keep
    /// running, so several worktrees can be generating at once.
    pub fn cmd_generate_walkthrough(&mut self, force: bool) {
        if self.review_store.is_none() {
            self.set_status(
                "Review database unavailable — cannot generate a walkthrough.".to_string(),
                StatusLevel::Error,
            );
            return;
        }
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status(
                "No worktree selected — open one to generate a walkthrough.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        // Only one generation may be in flight *per branch*: replacing the
        // handle would drop the running worker's receiver, stranding its
        // branch's row in `generating` forever. Generations for other branches
        // are none of this branch's business — they write disjoint rows, so
        // they run concurrently (see [`WalkthroughGenerations`]).
        if self.walkthrough_gens.is_generating(&branch) {
            self.set_status(
                "A walkthrough is already being generated for this branch.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let Some((wt_path, head_oid)) = self
            .worktrees
            .get(self.selected_worktree)
            .map(|w| (w.path.clone(), w.head_oid.clone()))
        else {
            return;
        };

        // Skip regeneration when a ready walkthrough already covers this exact
        // branch tip: the diff hasn't moved, so the walkthrough hasn't either.
        // Only when the current HEAD is actually known — an unknown tip (or a
        // pre-tracking row) never matches, so it always regenerates. `force`
        // (Alt+w / the palette's force entry) bypasses this to rebuild anyway.
        let up_to_date = !force
            && head_oid.as_deref().is_some_and(|head| {
            self.review_store
                .as_ref()
                .and_then(|s| s.get_walkthrough(&branch).ok().flatten())
                .is_some_and(|(w, _)| {
                    w.status == crate::walkthrough::WalkthroughStatus::Ready
                        && w.head_commit.as_deref() == Some(head)
                })
        });
        if up_to_date {
            let short: String = head_oid
                .as_deref()
                .map(|h| h.chars().take(8).collect())
                .unwrap_or_default();
            self.viewer_state.explorer.explorer_bottom_view =
                crate::viewer::ExplorerBottomView::Walkthrough;
            self.set_status(
                format!(
                    "Walkthrough already up to date for commit {short} — showing it. \
                     Alt+w (or the palette's force entry) to regenerate anyway."
                ),
                StatusLevel::Info,
            );
            return;
        }

        // Insert the `generating` row first so the UI (and a timeout) always
        // have a row to reflect, then spawn. Base ref comes from the PR meta
        // when this branch was taken in via PR intake. Record the branch tip
        // so the next same-commit regenerate short-circuits above.
        let store = self.review_store.as_ref().expect("checked above");
        if let Err(e) = store.begin_walkthrough(&branch, head_oid.as_deref()) {
            let msg = format!("Failed to start walkthrough: {e}");
            self.set_status(msg, StatusLevel::Error);
            return;
        }
        let base_ref = store
            .get_pr_review_meta(&branch)
            .ok()
            .flatten()
            .and_then(|m| m.base_ref);
        let api = self.config.api.clone();
        let language = self.config.review.walkthrough_language.clone();
        // `[review] walkthrough_model` is not passed on: which model answers is
        // the configured command's business now, not Conductor's. A user who
        // wants a specific model puts it in `[api] command`.
        let worktree = wt_path.clone();
        let branch_for_thread = branch.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::walkthrough::generate(
                    &api,
                    &worktree,
                    &branch_for_thread,
                    base_ref.as_deref(),
                    language.as_deref(),
                    &worker_cancel,
                )
            }))
            .unwrap_or_else(|_| Err("walkthrough generation thread panicked".to_string()));
            // A closed receiver means the app moved on (or shut down); nothing
            // to report to, and nothing to clean up.
            let _ = tx.send(outcome);
        });

        self.walkthrough_gens.insert(WalkthroughGeneration {
            branch: branch.clone(),
            result: rx,
            cancel,
        });
        // Display-only switch — no `set_focus`, so kicking off a generation
        // from the palette never steals focus from an active terminal input;
        // it just makes the in-progress state visible once the reviewer does
        // look at the Explorer.
        self.viewer_state.explorer.explorer_bottom_view =
            crate::viewer::ExplorerBottomView::Walkthrough;
        self.set_status(
            "Generating walkthrough in the background — this takes a few minutes.".to_string(),
            StatusLevel::Info,
        );
        self.refresh_reviews();
    }

    /// Stop every in-flight generation so none outlives the app as an orphaned
    /// subprocess. Called once on shutdown (see `event_loop.rs`, right before
    /// it returns on `should_quit`) — a generation still running at that point
    /// would otherwise keep burning tokens with no one left to read its result.
    pub fn shutdown_walkthrough_generation(&mut self) {
        self.walkthrough_gens.abort_all();
    }

    /// Drain the in-flight generations' result channels and reconcile each
    /// one's database row with it. Called from
    /// [`App::poll_all_background_ops`](Self::poll_all_background_ops).
    ///
    /// Unlike the old headless-session path, the row's `ready` state is written
    /// *here*, from the parsed reply — the model has no way to write it itself
    /// over the plain text seam, which is also why a malformed reply can no
    /// longer leave a row stuck in `generating`.
    pub fn poll_walkthrough_generation(&mut self) {
        if self.walkthrough_gens.is_empty() {
            return;
        }
        let finished = self.walkthrough_gens.take_finished();
        if finished.is_empty() {
            return;
        }
        for generation in finished {
            let (message, level) = self.reconcile_finished_generation(generation);
            self.set_status(message, level);
        }
        self.refresh_reviews();
    }

    /// Turn one finished generation into the status message to flash, writing
    /// a `failed` row when it did not produce a usable walkthrough.
    ///
    /// Messages name the branch: with several worktrees generating at once,
    /// the one that finishes is often not the one the reviewer is looking at.
    fn reconcile_finished_generation(
        &mut self,
        finished: FinishedGeneration,
    ) -> (String, StatusLevel) {
        let FinishedGeneration { branch, outcome } = finished;
        match outcome {
            Ok(generated) => self.save_generated_walkthrough(&branch, generated),
            Err(error) => {
                if let Some(store) = &self.review_store {
                    let _ = store.fail_walkthrough(&branch, &error);
                }
                (
                    format!("Walkthrough failed for '{branch}': {error}"),
                    StatusLevel::Error,
                )
            }
        }
    }

    /// Write a parsed generation into the review database, returning the status
    /// line to show for it.
    ///
    /// The inline comments are best-effort on purpose: they are the optional
    /// extra on top of the tour, so a comment that fails to insert must not
    /// turn a perfectly good walkthrough into a failed one.
    fn save_generated_walkthrough(
        &mut self,
        branch: &str,
        generated: crate::walkthrough::Generated,
    ) -> (String, StatusLevel) {
        let Some(store) = &self.review_store else {
            return (
                "Walkthrough generated but the review database is unavailable.".to_string(),
                StatusLevel::Error,
            );
        };
        let step_count = generated.steps.len();
        if let Err(e) = store.save_walkthrough(
            branch,
            &generated.title,
            &generated.summary,
            &generated.steps,
        ) {
            let msg = format!("Failed to save walkthrough: {e}");
            let _ = store.fail_walkthrough(branch, &msg);
            return (msg, StatusLevel::Error);
        }

        let mut saved_comments = 0usize;
        for comment in &generated.comments {
            let Some(line_start) = comment.line_start else {
                continue;
            };
            let result = store.add_review(
                branch,
                &comment.file_path,
                line_start,
                comment.line_end,
                crate::review_store::CommentKind::Question,
                &comment.body,
                "HEAD",
                crate::review_store::Author::Claude,
                Some(branch),
            );
            match result {
                Ok(_) => saved_comments += 1,
                Err(e) => log::warn!("failed to save generated inline comment: {e}"),
            }
        }

        let comments = if saved_comments > 0 {
            format!(", {saved_comments} inline comment(s)")
        } else {
            String::new()
        };
        (
            format!("Walkthrough ready for '{branch}' ({step_count} step(s){comments})."),
            StatusLevel::Success,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;

    type Outcome = Result<crate::walkthrough::Generated, String>;

    /// A registered generation plus the sender its worker thread would hold.
    /// Keeping the sender alive is what "still running" means to
    /// [`WalkthroughGenerations::take_finished`]; dropping it is how a worker
    /// that died without a result looks from here.
    fn generation(branch: &str) -> (WalkthroughGeneration, Sender<Outcome>, Arc<AtomicBool>) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (
            WalkthroughGeneration {
                branch: branch.to_string(),
                result: rx,
                cancel: cancel.clone(),
            },
            tx,
            cancel,
        )
    }

    fn sample_generated() -> crate::walkthrough::Generated {
        crate::walkthrough::Generated {
            title: "t".to_string(),
            summary: "s".to_string(),
            steps: Vec::new(),
            comments: Vec::new(),
        }
    }

    #[test]
    fn different_branches_generate_side_by_side() {
        // The bug this replaced: one in-flight generation blocked every other
        // worktree's branch, not just its own.
        let mut generations = WalkthroughGenerations::default();
        let (a, _tx_a, _) = generation("feature/a");
        let (b, _tx_b, _) = generation("feature/b");
        generations.insert(a);
        generations.insert(b);

        assert!(generations.is_generating("feature/a"));
        assert!(generations.is_generating("feature/b"));
        // Neither displaced nor finished the other.
        assert!(generations.take_finished().is_empty());

        generations.abort_all();
        assert!(generations.is_empty());
    }

    #[test]
    fn only_the_same_branch_is_refused() {
        let mut generations = WalkthroughGenerations::default();
        let (a, _tx_a, _) = generation("feature/a");
        generations.insert(a);

        // This predicate is the guard `cmd_generate_walkthrough` consults.
        assert!(generations.is_generating("feature/a"));
        assert!(!generations.is_generating("feature/b"));
    }

    #[test]
    fn a_finished_generation_frees_its_branch() {
        let mut generations = WalkthroughGenerations::default();
        let (a, tx, _) = generation("feature/a");
        generations.insert(a);
        tx.send(Ok(sample_generated())).expect("receiver is alive");

        let finished = generations.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].branch, "feature/a");
        assert!(finished[0].outcome.is_ok());
        assert!(!generations.is_generating("feature/a"));
    }

    #[test]
    fn a_dead_worker_frees_its_branch() {
        // Stale-lock recovery: a thread that panicked (or otherwise dropped its
        // sender without a result) must release its slot, so the next request
        // regenerates rather than being told one is already running.
        let mut generations = WalkthroughGenerations::default();
        let (a, tx, _) = generation("feature/a");
        generations.insert(a);
        drop(tx);

        let finished = generations.take_finished();
        assert_eq!(finished.len(), 1);
        let err = finished[0].outcome.as_ref().unwrap_err();
        assert!(err.contains("without a result"), "got: {err}");
        assert!(!generations.is_generating("feature/a"));

        // And the branch accepts a fresh generation immediately.
        let (again, _tx, _) = generation("feature/a");
        generations.insert(again);
        assert!(generations.is_generating("feature/a"));
    }

    #[test]
    fn abort_all_signals_every_worker_to_stop() {
        let mut generations = WalkthroughGenerations::default();
        let (a, _tx_a, cancel_a) = generation("feature/a");
        let (b, _tx_b, cancel_b) = generation("feature/b");
        generations.insert(a);
        generations.insert(b);

        generations.abort_all();

        // The flag each worker (and, through it, the AI caller's child) polls.
        assert!(cancel_a.load(Ordering::Relaxed));
        assert!(cancel_b.load(Ordering::Relaxed));
        assert!(generations.is_empty());
    }
}
