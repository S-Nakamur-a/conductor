//! PR intake and browser hand-off for [`App`].
//!
//! Drives the "Review Pull Request" overlay: fetches PR metadata + git refs
//! and prepares a worktree for review in the background, then applies the
//! result (persisting metadata, switching to the worktree, entering review
//! focus). Also supports opening the selected worktree's PR page directly in
//! the browser.

use super::*;

impl App {
    // ── PR intake (Review Pull Request) ───────────────────────────

    /// Kick off a background PR intake (gh metadata + git fetch + worktree
    /// creation) for the "Review Pull Request" overlay. Safe to call again
    /// for a retry after a failed attempt — the previous `bg_op` is just
    /// replaced.
    pub fn start_pr_intake(&mut self, input: &str) {
        self.overlays.pr_input.loading = true;
        self.overlays.pr_input.error = None;

        let repo_path = self.repo_path.clone();
        let worktree_dir = self.config.general.worktree_dir.clone();
        let input = input.to_string();

        self.overlays.pr_input.bg_op.start(move |tx| {
            let outcome = crate::pr_intake::intake_pr(&repo_path, worktree_dir.as_deref(), &input);
            let _ = tx.send(outcome);
        });
    }

    /// Poll the background PR intake for a result and apply it: persist any
    /// freshly-fetched PR metadata, switch to the worktree, and auto-launch
    /// review mode.
    ///
    /// Applies even if the overlay was dismissed (Esc) while the intake was
    /// still running — the fetch/worktree-creation already succeeded by
    /// then, so it shouldn't be discarded.
    pub fn poll_pr_intake(&mut self) {
        let Some(outcome) = self.overlays.pr_input.bg_op.poll() else {
            return;
        };
        self.overlays.pr_input.loading = false;

        match outcome {
            crate::pr_intake::PrIntakeOutcome::Ready {
                pr_number,
                worktree_path,
                meta,
            } => {
                if let Some(meta) = &meta
                    && let Some(store) = &self.review_store
                {
                    let _ = store.save_worktree_base_branch(&meta.branch, &meta.base_ref);
                    let _ = store.save_pr_review_meta(
                        &meta.branch,
                        Some(pr_number as i64),
                        Some(&meta.url),
                        Some(&meta.title),
                        Some(&meta.base_ref),
                        Some(&meta.head_ref),
                        meta.head_owner_login.as_deref(),
                    );
                }
                self.refresh_worktrees();
                self.select_worktree_by_path(&worktree_path);
                self.overlays.active = crate::overlay::ActiveOverlay::None;
                self.overlays.pr_input.buffer.clear();
                self.viewer_state.explorer.explorer_focus_on_diff_list = true;
                self.set_focus(Focus::Explorer);
                self.set_status(
                    format!(
                        "PR #{pr_number} ready for review — walkthrough: palette › Generate Walkthrough"
                    ),
                    StatusLevel::Success,
                );
            }
            crate::pr_intake::PrIntakeOutcome::Failed { error } => {
                self.overlays.pr_input.error = Some(error.to_string());
            }
        }
    }

    // ── Open PR in browser ───────────────────────────────────────

    /// Open the pull-request page for the selected worktree's branch in the
    /// default web browser.
    pub fn open_pr_in_browser(&mut self) {
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status("No worktree selected.".to_string(), StatusLevel::Warning);
            return;
        }

        match crate::git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => match engine.pr_url_for_branch(&branch) {
                Some(url) => {
                    log::info!("Opening PR URL: {url}");
                    if let Err(e) = open::that(&url) {
                        self.set_status(format!("Failed to open browser: {e}"), StatusLevel::Error);
                    } else {
                        self.set_status(format!("Opened PR for '{branch}'"), StatusLevel::Success);
                    }
                }
                None => {
                    self.set_status(
                        "Could not determine remote URL.".to_string(),
                        StatusLevel::Error,
                    );
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }
}
