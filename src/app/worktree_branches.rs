//! Branch/commit auxiliary operations for [`App`]: switch/base/grab branch
//! listing and filtering, worktree pull, stale-worktree pruning, and
//! cherry-pick.

use super::*;

impl App {
    /// Prune all stale worktrees.
    pub fn execute_prune(&mut self) {
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => {
                let mut pruned = 0;
                for name in &self.overlays.prune.stale {
                    match engine.prune_stale_worktree(name) {
                        Ok(()) => pruned += 1,
                        Err(e) => {
                            log::warn!("failed to prune worktree '{name}': {e}");
                        }
                    }
                }
                self.set_status(
                    format!("Pruned {pruned} stale worktree(s)."),
                    StatusLevel::Success,
                );
                self.overlays.prune.stale.clear();
                self.refresh_worktrees();
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Load remote branches for the switch overlay.
    ///
    /// Immediately populates the list from cached refs, then kicks off a
    /// background fetch. When the fetch completes, `poll_bg_branches()`
    /// picks up the refreshed list so the overlay updates without blocking.
    pub fn load_switch_branches(&mut self) {
        // Show cached refs instantly.
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.list_remote_branches() {
                Ok(branches) => {
                    self.overlays.switch_branch.branches = branches;
                    self.overlays.switch_branch.selected = 0;
                    self.overlays.switch_branch.filter.clear();
                }
                Err(e) => {
                    self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                    self.overlays.switch_branch.branches.clear();
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
                return;
            }
        }

        // Fetch in background and send updated branch list back.
        let repo_path = self.repo.path.clone();
        self.bg.branch.start(move |tx| {
            let engine = match git_engine::GitEngine::open(&repo_path) {
                Ok(e) => e,
                Err(err) => {
                    log::warn!("bg fetch: failed to open repo: {err}");
                    return;
                }
            };
            if let Err(e) = engine.fetch_origin() {
                log::warn!("bg fetch failed: {e}");
            }
            match engine.list_remote_branches() {
                Ok(branches) => {
                    let _ = tx.send(branches);
                }
                Err(e) => {
                    log::warn!("bg list_remote_branches failed: {e}");
                }
            }
        });
    }

    /// Check whether the background fetch has finished and update the
    /// switch-branch list if new data is available. Non-blocking.
    pub fn poll_bg_branches(&mut self) {
        if let Some(branches) = self.bg.branch.poll() {
            // Preserve the user's current filter/selection as best we can.
            let prev_selected_name = self
                .filtered_switch_branches()
                .get(self.overlays.switch_branch.selected)
                .map(|(_, name)| (*name).clone());
            self.overlays.switch_branch.branches = branches;
            // Try to restore selection by name.
            if let Some(name) = prev_selected_name
                && let Some(pos) = self
                    .filtered_switch_branches()
                    .iter()
                    .position(|(_, b)| **b == name)
            {
                self.overlays.switch_branch.selected = pos;
            }
            self.bg.branch.clear();
        }
    }

    // ── Pull worktree (fetch + fast-forward) ──────────────────────────

    /// Start a background pull (fetch + fast-forward) for the selected worktree.
    pub fn start_pull_worktree(&mut self) {
        if self.bg.pull.is_running() {
            self.set_status(
                "A pull is already in progress.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }

        let wt = match self.worktrees.selected() {
            Some(wt) => wt,
            None => return,
        };

        let branch = wt.branch.clone();
        let wt_path = wt.path.clone();
        let repo_path = self.repo.path.clone();

        self.set_status(format!("Pulling '{branch}'..."), StatusLevel::Info);

        self.bg.pull.start(move |tx| {
            let result = (|| -> Result<String, String> {
                let engine = git_engine::GitEngine::open(&repo_path)
                    .map_err(|e| format!("Failed to open repo: {e}"))?;
                engine.pull_worktree(&wt_path).map_err(|e| format!("{e}"))
            })();
            let _ = tx.send(result);
        });
    }

    /// Poll the background pull channel. Non-blocking.
    pub fn poll_bg_pull(&mut self) {
        if let Some(result) = self.bg.pull.poll() {
            match result {
                Ok(msg) => {
                    let level = if msg.contains("up-to-date") {
                        StatusLevel::Info
                    } else if msg.contains("fast-forward") {
                        StatusLevel::Success
                    } else {
                        StatusLevel::Warning
                    };
                    self.set_status(msg, level);
                    self.refresh_worktrees();
                }
                Err(err) => {
                    self.set_status(format!("Pull failed: {err}"), StatusLevel::Error);
                }
            }
        }
    }

    /// Return the filtered list of switch branches based on the current filter.
    pub fn filtered_switch_branches(&self) -> Vec<(usize, &String)> {
        if self.overlays.switch_branch.filter.is_empty() {
            self.overlays
                .switch_branch
                .branches
                .iter()
                .enumerate()
                .collect()
        } else {
            let filter_lower = self.overlays.switch_branch.filter.to_lowercase();
            self.overlays
                .switch_branch
                .branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    pub fn filtered_grab_branches(&self) -> Vec<(usize, &String)> {
        if self.overlays.grab.filter.is_empty() {
            self.overlays.grab.branches.iter().enumerate().collect()
        } else {
            let filter_lower = self.overlays.grab.filter.to_lowercase();
            self.overlays
                .grab
                .branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// Load branches available as base for worktree creation.
    /// Lists remote branches and pre-selects `origin/<main_branch>`.
    pub fn load_base_branches(&mut self) {
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => {
                // Prefer remote-tracking branches; fall back to local branches
                // when the repo has no remote (e.g. a local-only project),
                // otherwise the picker would be empty and nothing is selectable.
                let branches = match engine.list_remote_branches() {
                    Ok(remote) if !remote.is_empty() => Ok(remote),
                    Ok(_) => engine.list_local_branches(),
                    Err(e) => Err(e),
                };
                match branches {
                    Ok(branches) => {
                        self.worktree_mgr.base_branch_list = branches;
                        self.worktree_mgr.base_branch_selected = 0;
                        self.worktree_mgr.base_branch_filter.clear();
                        // Pre-select origin/<main_branch>, or the local
                        // <main_branch> when there is no remote.
                        let main_branch = self.config.general.main_branch.clone();
                        let remote_base = format!("origin/{main_branch}");
                        if let Some(pos) = self
                            .worktree_mgr
                            .base_branch_list
                            .iter()
                            .position(|b| b == &remote_base || b == &main_branch)
                        {
                            self.worktree_mgr.base_branch_selected = pos;
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.worktree_mgr.base_branch_list.clear();
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Return the filtered list of base branches based on the current filter.
    pub fn filtered_base_branches(&self) -> Vec<(usize, &String)> {
        if self.worktree_mgr.base_branch_filter.is_empty() {
            self.worktree_mgr
                .base_branch_list
                .iter()
                .enumerate()
                .collect()
        } else {
            let filter_lower = self.worktree_mgr.base_branch_filter.to_lowercase();
            self.worktree_mgr
                .base_branch_list
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// Load grab branch candidates (non-main worktree branches).
    pub fn load_grab_branches(&mut self) {
        self.overlays.grab.branches = self
            .worktrees
            .iter()
            .filter(|w| !w.is_main)
            .map(|w| w.branch.clone())
            .collect();
        self.overlays.grab.selected = 0;
    }

    // ── Cherry-pick helpers ────────────────────────────────────────────

    pub fn load_cherry_pick_commits(&mut self) {
        let branch = self.overlays.cherry_pick.source_branch.clone();
        if branch.is_empty() {
            self.overlays.cherry_pick.commits.clear();
            return;
        }
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.list_branch_commits(&branch, 20) {
                Ok(commits) => {
                    self.overlays.cherry_pick.commits = commits;
                    self.overlays.cherry_pick.selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to list commits for branch '{branch}': {e}");
                    self.overlays.cherry_pick.commits.clear();
                }
            },
            Err(e) => {
                log::warn!("failed to open git repository for cherry-pick: {e}");
                self.overlays.cherry_pick.commits.clear();
            }
        }
    }

    pub fn execute_cherry_pick(&mut self) {
        let commit = match self
            .overlays
            .cherry_pick
            .commits
            .get(self.overlays.cherry_pick.selected)
        {
            Some(c) => c.clone(),
            None => {
                self.set_status("No commit selected.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let wt_path = match self.worktrees.selected() {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("No worktree selected.".to_string(), StatusLevel::Error);
                return;
            }
        };

        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.cherry_pick_to_worktree(&wt_path, &commit.oid) {
                Ok(msg) => {
                    self.set_status(msg, StatusLevel::Success);
                    self.refresh_worktrees();
                }
                Err(e) => {
                    self.set_status(format!("Cherry-pick error: {e}"), StatusLevel::Error);
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }
}
