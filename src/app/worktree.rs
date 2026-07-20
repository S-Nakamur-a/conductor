//! Worktree switching core for [`App`].
//!
//! Handles selecting a worktree (by index or path), the full
//! `on_worktree_changed` refresh flow (view/session bookkeeping plus
//! dispatching the background file-tree, diff, and branch-details work),
//! polling those background results, and the small helpers (PR url lookup,
//! `gh` availability, worktree-op channel) shared by the other `worktree_*`
//! submodules.

use std::sync::mpsc;

use super::*;

impl App {
    // ── Worktree create / delete helpers ──────────────────────────

    /// Select a worktree by its path and trigger UI updates.
    ///
    /// `pub(super)` — shared with [`super::worktree_grab`] and [`super::worktree_pr`].
    pub(super) fn select_worktree_by_path(&mut self, path: &std::path::Path) {
        if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
            self.selected_worktree = idx;
            self.on_worktree_changed();
        }
    }

    /// Called when the selected worktree changes — refreshes viewer, diff, sessions.
    ///
    /// Heavy operations (file tree walk, diff computation, branch details) are
    /// dispatched to background threads so the UI stays responsive. Results are
    /// applied in `poll_worktree_switch_ops()`.
    /// Switch the selection to the next worktree, wrapping around. Mirrors a
    /// click on the strip (updates views + active sessions via
    /// `on_worktree_changed`, which also makes the strip follow), but keeps the
    /// current panel focus rather than jumping to the terminal. No-op with ≤1
    /// worktree.
    pub fn select_next_worktree(&mut self) {
        let n = self.worktrees.len();
        if n <= 1 {
            return;
        }
        self.selected_worktree = (self.selected_worktree + 1) % n;
        self.on_worktree_changed();
    }

    /// Switch the selection to the previous worktree, wrapping around. See
    /// [`Self::select_next_worktree`].
    pub fn select_prev_worktree(&mut self) {
        let n = self.worktrees.len();
        if n <= 1 {
            return;
        }
        self.selected_worktree = (self.selected_worktree + n - 1) % n;
        self.on_worktree_changed();
    }

    pub fn on_worktree_changed(&mut self) {
        // A reflow transcript belongs to the previous worktree's session;
        // switching worktrees must reset it before new session state loads.
        if self.reflow.active {
            self.close_reflow();
        }

        // An embedded editor belongs to the worktree it was opened on; leaving
        // that worktree would strand it editing the wrong tree, so close it
        // first. The view reload below covers the new worktree.
        self.discard_editor_on_worktree_change();

        // Reveal the newly selected worktree's chip in the bar on the next
        // render (width-dependent panning happens there, where the area is known).
        // This is only safe to set on *user-initiated* selection changes: if a
        // background event ever drives selection while the user is free-scrolling
        // the strip to peek elsewhere, setting this would yank the bar back.
        self.wtbar_reveal_selected = true;

        // Persist the outgoing worktree's view before we wipe it.
        if let Some(outgoing) = self.current_view_branch.clone() {
            self.save_view_for(&outgoing);
        }

        self.viewer_state = ViewerState::default();

        // Track the worktree now being loaded and seed its saved file/scroll
        // so it gets re-opened once the file tree arrives.
        let new_branch = self.selected_worktree_branch();
        self.pending_view_restore = None;
        self.current_view_branch = if new_branch.is_empty() {
            None
        } else {
            Some(new_branch.clone())
        };
        if let Some(store) = &self.review_store {
            if !new_branch.is_empty() {
                let _ = store.set_selected_worktree(&new_branch);
            }
            if let Ok(Some((Some(file), line))) = store.get_view_state(&new_branch) {
                self.pending_view_restore = Some(crate::app::PendingViewRestore {
                    file,
                    scroll: line.max(0) as usize,
                });
            }
        }

        // Clear "new" badge for the worktree the user just selected.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            self.new_worktree_paths.remove(&wt.path);
        }

        // Reviews are fast (SQLite) — keep synchronous.
        self.refresh_reviews();

        // Snapshot baseline so the next poll cycle doesn't trigger a redundant refresh.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            self.last_poll_head_oid = self.worktree_heads.get(&wt.branch).cloned();
            self.last_poll_status = Some((wt.added, wt.modified, wt.deleted));
        }

        // Update active sessions to match the new worktree.
        let wt_name = self.selected_worktree_branch();
        let claude_sessions = self.current_worktree_claude_sessions();
        self.terminal.active_claude_session = claude_sessions.first().map(|(idx, _)| *idx);
        let shell_sessions = self.current_worktree_shell_sessions();
        self.terminal.active_shell_session = shell_sessions.first().map(|(idx, _)| *idx);

        // Activate the PTY sessions.
        if let Some(idx) = self.terminal.active_claude_session {
            self.terminal.pty_manager.activate_session(idx);
        }
        if let Some(idx) = self.terminal.active_shell_session {
            self.terminal.pty_manager.activate_session(idx);
        }

        self.terminal.scroll_claude = 0;
        self.terminal.scroll_shell = 0;
        self.terminal.cache_claude = Default::default();
        self.terminal.cache_shell = Default::default();

        // Dispatch heavy operations to background threads.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let wt_path = wt.path.clone();

            // Background file tree walk.
            {
                let path = wt_path.clone();
                self.bg.file_tree.start(move |tx| {
                    let mut entries = Vec::new();
                    ViewerState::walk_dir(&path, &path, 0, &mut entries);
                    let _ = tx.send(entries);
                });
            }

            // Background diff computation.
            {
                let path = wt_path.clone();
                let base_branch = self.config.general.main_branch.clone();
                let word_diff = self.config.diff.word_diff;
                let tab_width = self.config.viewer.tab_width;
                self.bg.diff.start(move |tx| {
                    let mut result = BgDiffResult {
                        committed: Vec::new(),
                        uncommitted: Vec::new(),
                        error: None,
                    };
                    match DiffState::compute_diff_range_static(
                        &path,
                        &base_branch,
                        true,
                        word_diff,
                        tab_width,
                    ) {
                        Ok(mut files) => {
                            files.sort_by(|a, b| a.path.cmp(&b.path));
                            result.committed = files;
                        }
                        Err(e) => {
                            result.error = Some(format!("{e:#}"));
                            let _ = tx.send(result);
                            return;
                        }
                    }
                    match DiffState::compute_diff_range_static(
                        &path,
                        &base_branch,
                        false,
                        word_diff,
                        tab_width,
                    ) {
                        Ok(mut files) => {
                            files.sort_by(|a, b| a.path.cmp(&b.path));
                            result.uncommitted = files;
                        }
                        Err(e) => {
                            log::warn!("failed to compute uncommitted diff: {e:#}");
                        }
                    }
                    let _ = tx.send(result);
                });
            }

            // Background branch details computation.
            self.start_bg_branch_details();
        }

        self.set_status(
            format!("Switched to worktree: {wt_name}"),
            StatusLevel::Success,
        );
    }

    /// Spawn background branch details computation.
    fn start_bg_branch_details(&mut self) {
        let Some(wt) = self.worktrees.get(self.selected_worktree) else {
            self.branch_details = Default::default();
            return;
        };
        let branch = wt.branch.clone();
        let is_main = wt.is_main;
        let repo_path = self.repo_path.clone();
        let main_branch = self.config.general.main_branch.clone();
        let worktree_branches: Vec<String> = self
            .worktrees
            .iter()
            .filter(|w| !w.is_main && w.branch != branch)
            .map(|w| w.branch.clone())
            .collect();

        // Check DB for cached parent/children before spawning the thread.
        let db_initial_branch = if !is_main {
            self.review_store
                .as_ref()
                .and_then(|store| store.get_worktree_base_branch(&branch).ok().flatten())
        } else {
            None
        };

        let active_branches: std::collections::HashSet<String> =
            self.worktrees.iter().map(|w| w.branch.clone()).collect();
        let db_children: Vec<String> = self
            .review_store
            .as_ref()
            .and_then(|store| store.get_worktree_children(&branch).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| active_branches.contains(c))
            .collect();

        // Reset branch_details and start PR lookup (already async).
        self.branch_details = Default::default();
        if !is_main && self.gh_available {
            self.branch_details.pr_loading = true;
            self.start_pr_url_lookup(&branch);
        }

        self.bg.branch_details.start(move |tx| {
            let mut details = git_engine::BranchDetails::default();

            if !is_main {
                details.initial_branch = db_initial_branch.or_else(|| {
                    git_engine::GitEngine::open(&repo_path)
                        .ok()
                        .and_then(|engine| {
                            engine.detect_parent_branch(&branch, &main_branch, &worktree_branches)
                        })
                });
            }

            if !db_children.is_empty() {
                details.derived_branches = db_children;
            } else if let Ok(engine) = git_engine::GitEngine::open(&repo_path)
                && let Ok(derived) =
                    engine.find_derived_branches(&branch, &main_branch, &worktree_branches)
            {
                details.derived_branches = derived;
            }

            let _ = tx.send(details);
        });
    }

    /// Poll background worktree-switch operations (file tree, diff, branch details).
    pub fn poll_worktree_switch_ops(&mut self) {
        // File tree result.
        if let Some(entries) = self.bg.file_tree.poll() {
            self.viewer_state.tree.file_tree = entries;
            self.viewer_state.invalidate_visible_cache();
            // Restore the previously viewed file + scroll for this worktree now
            // that its file tree is available (one-shot).
            self.consume_pending_view_restore();
            self.rehighlight_viewer();
        }

        // Diff result.
        if let Some(result) = self.bg.diff.poll() {
            if let Some(error) = result.error {
                self.diff_state.committed_files.clear();
                self.diff_state.uncommitted_files.clear();
                self.diff_state.error = Some(error);
            } else {
                self.diff_state.committed_files = result.committed;
                self.diff_state.uncommitted_files = result.uncommitted;
                self.diff_state.error = None;
            }
            self.diff_state.rebuild_display_list();
        }

        // Branch details result.
        if let Some(details) = self.bg.branch_details.poll() {
            // Preserve pr_url and pr_loading from the already-running PR lookup.
            let pr_url = self.branch_details.pr_url.take();
            let pr_loading = self.branch_details.pr_loading;
            self.branch_details = details;
            self.branch_details.pr_url = pr_url;
            self.branch_details.pr_loading = pr_loading;
        }
    }

    // ── Branch details (worktree detail panel) ───────────────────

    /// Check whether the `gh` CLI is available on this system.
    pub(super) fn check_gh_available() -> bool {
        std::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Get (or lazily create) a sender for worktree operation results.
    ///
    /// `pub(super)` — shared with [`super::worktree_crud`] and [`super::worktree_smart`].
    pub(super) fn worktree_op_sender(&mut self) -> mpsc::Sender<WorktreeOpResult> {
        if self.worktree_mgr.bg_worktree_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.worktree_mgr.bg_worktree_tx = Some(tx);
            self.worktree_mgr.bg_worktree_rx = Some(rx);
        }
        self.worktree_mgr.bg_worktree_tx.as_ref().unwrap().clone()
    }

    /// Spawn a background thread to look up the PR URL via `gh pr view`.
    fn start_pr_url_lookup(&mut self, branch: &str) {
        let branch = branch.to_string();
        let repo_path = self.repo_path.clone();

        self.bg.pr_url.start(move |tx| {
            let result = std::process::Command::new("gh")
                .args([
                    "pr", "view", "--head", &branch, "--json", "url", "-q", ".url",
                ])
                .current_dir(&repo_path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if url.is_empty() { None } else { Some(url) }
                    } else {
                        None
                    }
                });
            let _ = tx.send(result);
        });
    }

    /// Poll the background PR URL lookup for a result.
    pub fn poll_pr_url(&mut self) {
        if let Some(result) = self.bg.pr_url.poll() {
            self.branch_details.pr_url = result;
            self.branch_details.pr_loading = false;
        }
    }
}
