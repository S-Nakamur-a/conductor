//! AI walkthrough generation orchestration for [`App`].
//!
//! Starts a headless `claude -p` session via [`crate::walkthrough`], polls it
//! to completion, and reflects success/failure back into the review database
//! via [`crate::review_store::ReviewStore`].

use super::*;

impl App {
    /// Kick off walkthrough generation for the selected worktree's branch:
    /// insert the `generating` row, then spawn the headless Claude session.
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
        // handle would silently drop (and orphan) the running `claude` child
        // and strand its branch's row in `generating` forever. Generations
        // for other branches are none of this branch's business — they write
        // disjoint rows, so they run concurrently (see
        // `walkthrough::WalkthroughGenerations`).
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
        let db = crate::review_store::db_path(&self.repo_path);
        let model = self.config.review.walkthrough_model.clone();
        let language = self.config.review.walkthrough_language.clone();
        match crate::walkthrough::spawn_generation(
            &wt_path,
            &db,
            &branch,
            base_ref.as_deref(),
            model.as_deref(),
            language.as_deref(),
        ) {
            Ok(generation) => {
                self.walkthrough_gens.insert(generation);
                // Display-only switch — no `set_focus`, so kicking off a
                // generation from the palette never steals focus from an
                // active terminal input; it just makes the in-progress state
                // visible once the reviewer does look at the Explorer.
                self.viewer_state.explorer.explorer_bottom_view =
                    crate::viewer::ExplorerBottomView::Walkthrough;
                self.set_status(
                    "Generating walkthrough in the background — this takes a few minutes."
                        .to_string(),
                    StatusLevel::Info,
                );
            }
            Err(e) => {
                let msg = e.to_string();
                if let Some(store) = &self.review_store {
                    let _ = store.fail_walkthrough(&branch, &msg);
                }
                self.set_status(
                    format!("Failed to launch walkthrough generation: {msg}"),
                    StatusLevel::Error,
                );
            }
        }
        self.refresh_reviews();
    }

    /// Kill every in-flight walkthrough generation so none outlives the app as
    /// an orphaned headless `claude` process. Called once on shutdown (see
    /// `main.rs`'s main loop, right before it returns on `should_quit`) — a
    /// generation still running at that point would otherwise keep making API
    /// calls with no one polling its outcome.
    pub fn shutdown_walkthrough_generation(&mut self) {
        self.walkthrough_gens.abort_all();
    }

    /// Poll the in-flight walkthrough generations and reconcile each one's
    /// database row with what its process actually did. Called from
    /// [`App::poll_all_background_ops`](Self::poll_all_background_ops).
    pub fn poll_walkthrough_generation(&mut self) {
        if self.walkthrough_gens.is_empty() {
            return;
        }
        let finished = self.walkthrough_gens.take_finished();
        if finished.is_empty() {
            return;
        }
        for outcome in finished {
            let (message, level) = self.reconcile_finished_generation(outcome);
            self.set_status(message, level);
        }
        self.refresh_reviews();
    }

    /// Turn one finished generation into the status message to flash, writing
    /// a `failed` row when the session didn't leave a good one behind.
    ///
    /// Messages name the branch: with several worktrees generating at once,
    /// the one that finishes is often not the one the reviewer is looking at.
    fn reconcile_finished_generation(
        &self,
        finished: crate::walkthrough::FinishedGeneration,
    ) -> (String, StatusLevel) {
        use crate::walkthrough::{GenerationPoll, WalkthroughStatus};
        let crate::walkthrough::FinishedGeneration {
            branch,
            log_path,
            outcome,
        } = finished;
        let fail = |msg: &str| {
            if let Some(store) = &self.review_store {
                let _ = store.fail_walkthrough(&branch, msg);
            }
            (
                format!("Walkthrough failed for '{branch}': {msg}"),
                StatusLevel::Error,
            )
        };
        match outcome {
            GenerationPoll::Running => {
                unreachable!("take_finished only yields generations that stopped")
            }
            GenerationPoll::Exited => {
                // Success is decided by the row the MCP tool wrote, not the
                // exit code: a session that ended without saving is a failure.
                let saved = self
                    .review_store
                    .as_ref()
                    .and_then(|s| s.get_walkthrough(&branch).ok().flatten())
                    .is_some_and(|(w, _)| w.status == WalkthroughStatus::Ready);
                if saved {
                    (
                        format!("Walkthrough ready for '{branch}'."),
                        StatusLevel::Success,
                    )
                } else {
                    fail(&format!(
                        "Claude session ended without saving a walkthrough (log: {})",
                        log_path.display()
                    ))
                }
            }
            GenerationPoll::Failed(msg) => fail(&msg),
            GenerationPoll::TimedOut => fail(&format!(
                "Timed out after {} minutes.",
                crate::walkthrough::GENERATION_TIMEOUT.as_secs() / 60
            )),
        }
    }
}
