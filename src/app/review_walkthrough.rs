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
    /// generation is already in flight it's a no-op with a status hint.
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
        // Only one generation may be in flight per app instance: replacing
        // the handle would silently drop (and orphan) the running `claude`
        // child and strand its branch's row in `generating` forever.
        if let Some(g) = &self.walkthrough_gen {
            let msg = if g.branch == branch {
                "A walkthrough is already being generated for this branch.".to_string()
            } else {
                format!(
                    "A walkthrough is already being generated for '{}' — wait for it to finish.",
                    g.branch
                )
            };
            self.set_status(msg, StatusLevel::Warning);
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
            &self.repo_path,
            &wt_path,
            &db,
            &branch,
            base_ref.as_deref(),
            model.as_deref(),
            language.as_deref(),
        ) {
            Ok(generation) => {
                self.walkthrough_gen = Some(generation);
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

    /// Kill an in-flight walkthrough generation, if any, so it doesn't
    /// outlive the app as an orphaned headless `claude` process. Called once
    /// on shutdown (see `main.rs`'s main loop, right before it returns on
    /// `should_quit`) — a generation still running at that point would
    /// otherwise keep making API calls with no one polling its outcome.
    pub fn shutdown_walkthrough_generation(&mut self) {
        if let Some(mut generation) = self.walkthrough_gen.take() {
            generation.abort();
        }
    }

    /// Poll the in-flight walkthrough generation (if any) and reconcile the
    /// database row with what the process actually did. Called from
    /// [`App::poll_all_background_ops`](Self::poll_all_background_ops).
    pub fn poll_walkthrough_generation(&mut self) {
        let Some(generation) = &mut self.walkthrough_gen else {
            return;
        };
        use crate::walkthrough::{GenerationPoll, WalkthroughStatus};
        let outcome = generation.poll();
        if matches!(outcome, GenerationPoll::Running) {
            return;
        }
        let branch = generation.branch.clone();
        let log_path = generation.log_path.clone();
        self.walkthrough_gen = None;

        let (message, level) = match outcome {
            GenerationPoll::Running => unreachable!("handled above"),
            GenerationPoll::Exited => {
                // Success is decided by the row the MCP tool wrote, not the
                // exit code: a session that ended without saving is a failure.
                let saved = self
                    .review_store
                    .as_ref()
                    .and_then(|s| s.get_walkthrough(&branch).ok().flatten())
                    .is_some_and(|(w, _)| w.status == WalkthroughStatus::Ready);
                if saved {
                    ("Walkthrough ready.".to_string(), StatusLevel::Success)
                } else {
                    let msg = format!(
                        "Claude session ended without saving a walkthrough (log: {})",
                        log_path.display()
                    );
                    if let Some(store) = &self.review_store {
                        let _ = store.fail_walkthrough(&branch, &msg);
                    }
                    (format!("Walkthrough failed: {msg}"), StatusLevel::Error)
                }
            }
            GenerationPoll::Failed(msg) => {
                if let Some(store) = &self.review_store {
                    let _ = store.fail_walkthrough(&branch, &msg);
                }
                (format!("Walkthrough failed: {msg}"), StatusLevel::Error)
            }
            GenerationPoll::TimedOut => {
                let msg = format!(
                    "Timed out after {} minutes.",
                    crate::walkthrough::GENERATION_TIMEOUT.as_secs() / 60
                );
                if let Some(store) = &self.review_store {
                    let _ = store.fail_walkthrough(&branch, &msg);
                }
                (format!("Walkthrough failed: {msg}"), StatusLevel::Error)
            }
        };
        self.set_status(message, level);
        self.refresh_reviews();
    }
}
