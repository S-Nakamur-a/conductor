//! Repository selection and the cached worktree list: switching between known
//! repos, opening an arbitrary path, and refreshing worktree/branch state from
//! git.

use crate::git_engine;
use crate::review_store::{self, ReviewStore};
use crate::viewer::ViewerState;

use super::{App, StatusLevel};

impl App {
    /// Switch to a different repository by index in `repo_list`.
    pub fn switch_repo(&mut self, index: usize) {
        if index >= self.repo_list.len() {
            return;
        }
        // Persist the outgoing repo's view before swapping the store.
        self.persist_view_state();
        self.repo_list_index = index;
        self.repo_path = self.repo_list[index].clone();

        // Re-open the review store for the new repo path.
        let db = review_store::db_path(&self.repo_path);
        self.review_store = match ReviewStore::open(&db) {
            Ok(store) => Some(store),
            Err(e) => {
                log::warn!("failed to open review store for new repo: {e}");
                None
            }
        };

        // Update main repo name for the new repository.
        self.main_repo_name = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|engine| engine.main_worktree_path())
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| {
                self.repo_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| self.repo_path.display().to_string())
            });

        // Refresh worktrees and reviews eagerly; viewer/diff will lazy-load.
        self.selected_worktree = 0;
        self.refresh_worktrees();
        self.viewer_state = ViewerState::default();
        self.diff_state =
            crate::diff_state::DiffState::new(&self.config.general.main_branch, self.diff_state.view_mode);
        // Restore the new repo's last selected worktree + open file/scroll.
        self.restore_selected_worktree_and_view();
        self.refresh_reviews();
        self.terminal.active_claude_session = None;
        self.terminal.active_shell_session = None;

        self.set_status(
            format!("Switched to repository: {}", self.main_repo_name),
            StatusLevel::Success,
        );
    }

    /// Open a repository from an arbitrary filesystem path.
    pub fn open_repo_from_path(&mut self, path: &str) {
        // Expand ~ to home directory.
        let expanded = if let Some(stripped) = path.strip_prefix('~') {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped.strip_prefix('/').unwrap_or(stripped))
            } else {
                std::path::PathBuf::from(path)
            }
        } else {
            std::path::PathBuf::from(path)
        };

        // Canonicalize if possible, otherwise use as-is.
        let canonical = expanded.canonicalize().unwrap_or(expanded);

        if !canonical.is_dir() {
            self.set_status(
                format!("Not a directory: {}", canonical.display()),
                StatusLevel::Error,
            );
            return;
        }

        // Try to discover a git repository at this path.
        match git_engine::GitEngine::open(&canonical) {
            Ok(_engine) => {
                // Valid git repo — switch to it.
                self.repo_path = canonical.clone();

                // Re-open the review store for the new repo path.
                let db = review_store::db_path(&self.repo_path);
                self.review_store = match ReviewStore::open(&db) {
                    Ok(store) => Some(store),
                    Err(e) => {
                        log::warn!("failed to open review store for new repo: {e}");
                        None
                    }
                };

                self.selected_worktree = 0;
                self.refresh_worktrees();
                self.viewer_state = ViewerState::default();
                // This repo gets no view restore, so drop any restore still
                // armed for the *previous* repo — otherwise it could fire here
                // and open a same-named path in the newly opened tree.
                self.pending_view_restore = None;
                self.diff_state =
                    crate::diff_state::DiffState::new(&self.config.general.main_branch, self.diff_state.view_mode);
                self.refresh_reviews();
                self.terminal.active_claude_session = None;
                self.terminal.active_shell_session = None;

                // Add to repo_list if not already present.
                if !self.repo_list.contains(&canonical) {
                    self.repo_list.push(canonical.clone());
                }
                // Update repo_list_index to point to this repo.
                self.repo_list_index = self
                    .repo_list
                    .iter()
                    .position(|p| p == &canonical)
                    .unwrap_or(0);

                let repo_name = canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| canonical.display().to_string());
                self.set_status(
                    format!("Opened repository: {repo_name}"),
                    StatusLevel::Success,
                );
            }
            Err(e) => {
                self.set_status(
                    format!("Not a git repository: {} ({e})", canonical.display()),
                    StatusLevel::Error,
                );
            }
        }
    }

    /// Refresh the cached worktree list from the repository.
    ///
    /// Returns `true` if the worktree list actually changed (different count,
    /// branch names, or status counts), so callers can skip redraws when
    /// nothing is different.
    pub fn refresh_worktrees(&mut self) -> bool {
        let mut changed = false;
        // Remember which branch is selected *before* we replace the list, so we
        // can pin the selection to it by identity afterwards (the list order can
        // shift when worktrees are added/removed).
        let prev_selected_branch = self.selected_worktree_branch();
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_worktrees() {
                    Ok(worktrees) => {
                        // Detect whether the worktree list changed before replacing it.
                        if worktrees.len() != self.worktrees.len() {
                            changed = true;
                        } else {
                            for (old, new) in self.worktrees.iter().zip(worktrees.iter()) {
                                if old.branch != new.branch
                                    || old.added != new.added
                                    || old.modified != new.modified
                                    || old.deleted != new.deleted
                                    || old.is_clean != new.is_clean
                                {
                                    changed = true;
                                    break;
                                }
                            }
                        }
                        self.worktrees = worktrees;
                        // Preserve the selection by *branch identity*, not list
                        // position: indices shift when worktrees are added or
                        // removed. Re-finding the branch keeps the selection
                        // pinned to the same worktree instead of silently sliding
                        // onto a neighbour. Only when the branch is gone (its
                        // worktree was removed) do we fall back to clamping.
                        if let Some(idx) = reselect_worktree_index(
                            &self.worktrees,
                            &prev_selected_branch,
                            self.selected_worktree,
                        ) {
                            self.selected_worktree = idx;
                        }
                        // Detect commits by HEAD oid changes. The oid was
                        // captured while `list_worktrees` had each repo open,
                        // so no second `Repository::open` per worktree here.
                        let head_updates: Vec<(String, String)> = self
                            .worktrees
                            .iter()
                            .filter_map(|wt| {
                                wt.head_oid
                                    .clone()
                                    .map(|oid| (wt.branch.clone(), oid))
                            })
                            .collect();
                        for (branch, head_oid) in head_updates {
                            if let Some(old) = self.worktree_heads.get(&branch)
                                && old != &head_oid
                            {
                                self.record_stat("commits_made");
                                changed = true;
                            }
                            self.worktree_heads.insert(branch, head_oid);
                        }
                    }
                    Err(e) => {
                        log::warn!("failed to list worktrees: {e}");
                    }
                }
                // Refresh local branches for the detail zone.
                if let Ok(branches) = engine.list_local_branches() {
                    if branches != self.worktree_mgr.local_branches {
                        changed = true;
                    }
                    self.worktree_mgr.local_branches = branches;
                }
            }
            Err(e) => {
                log::warn!("failed to open git repository: {e}");
            }
        }
        self.rebuild_worktree_list_rows();
        // If the selected worktree's branch changed out from under us (its
        // worktree was removed, so the selection fell back to another branch —
        // often the main worktree), reload the review state. Otherwise the
        // previous branch's change summary and comments linger and get shown
        // against the wrong branch (e.g. a merged PR's summary on `main`).
        //
        // An *empty* branch is excluded: `list_worktrees` logs and skips a
        // worktree it fails to inspect (see `git_engine::worktree_ops`), so a
        // transient git error can empty the list for one poll. That is a failed
        // read, not a selection change, and reloading reviews against `""`
        // would blank out the panel every few seconds until it recovered.
        let new_branch = self.selected_worktree_branch();
        if !new_branch.is_empty() && new_branch != prev_selected_branch {
            self.refresh_reviews();
        }
        changed
    }

    /// Advance the decoration animation by one tick. Returns `true` when
    /// an animation was actually updated (i.e. mode is not `None`).
    pub fn tick_decoration(&mut self, width: u16, height: u16) -> bool {
        use crate::ui::decoration::{DecorationActivity, DecorationMode};
        let mode = DecorationMode::from_str(&self.config.general.decoration);
        if !mode.has_animation() {
            return false;
        }
        self.decoration_tick = self.decoration_tick.wrapping_add(1);
        let activity = if self.terminal.cc_waiting_worktrees.is_empty() {
            DecorationActivity::Calm
        } else {
            DecorationActivity::Active
        };
        crate::ui::decoration::tick_decoration(
            &mut self.decoration_states,
            self.decoration_tick,
            width,
            height,
            activity,
            mode,
        );
        true
    }

    /// Record a stat event for both the current session and daily totals.
    pub(super) fn record_stat(&self, field: &str) {
        if let Some(store) = &self.review_store {
            let _ = store.increment_daily_stat(field);
            if let Some(ref sid) = self.stats_session_id {
                let _ = store.increment_session_stat(sid, field);
            }
        }
    }

    /// Return `(worktree_name, working_dir)` for the currently selected worktree.
    pub(super) fn selected_worktree_info(&self) -> (String, std::path::PathBuf) {
        self.worktrees
            .get(self.selected_worktree)
            .map(|w| (w.branch.clone(), w.path.clone()))
            .unwrap_or_else(|| ("default".to_string(), self.repo_path.clone()))
    }
}

/// Pick the worktree index to keep selected after the worktree list is
/// refreshed.
///
/// The list order is not stable across refreshes — adding or removing a
/// worktree shifts every index after it. Selecting purely by the old index
/// would silently re-point the selection at a *different* branch, which then
/// shows that branch's review data (including the change summary) against the
/// wrong worktree. So we re-pin by branch identity first; only when the
/// previously selected branch is gone do we clamp the old index into range.
///
/// Returns `None` when there are no worktrees (nothing to select).
fn reselect_worktree_index(
    worktrees: &[git_engine::WorktreeInfo],
    prev_branch: &str,
    old_index: usize,
) -> Option<usize> {
    if worktrees.is_empty() {
        return None;
    }
    if !prev_branch.is_empty()
        && let Some(idx) = worktrees.iter().position(|w| w.branch == prev_branch)
    {
        return Some(idx);
    }
    Some(old_index.min(worktrees.len() - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(branch: &str) -> git_engine::WorktreeInfo {
        git_engine::WorktreeInfo {
            path: std::path::PathBuf::from(format!("/tmp/{branch}")),
            branch: branch.to_string(),
            is_main: branch == "main",
            added: 0,
            modified: 0,
            deleted: 0,
            is_clean: true,
            ahead: None,
            behind: None,
            head_oid: None,
        }
    }

    #[test]
    fn reselect_pins_to_branch_when_order_shifts() {
        // Selection points at "feat-b" (index 2). A new worktree inserted
        // earlier shifts indices; the selection must follow "feat-b", not stay
        // at index 2 (which now holds a different branch).
        let after = [wt("main"), wt("feat-a"), wt("feat-aa"), wt("feat-b")];
        assert_eq!(reselect_worktree_index(&after, "feat-b", 2), Some(3));
    }

    #[test]
    fn reselect_falls_back_when_branch_removed() {
        // "feat-a" (index 1) was removed; only "main" remains. The stale index 1
        // is out of range and must clamp to the last valid index (main).
        let after = [wt("main")];
        assert_eq!(reselect_worktree_index(&after, "feat-a", 1), Some(0));
    }

    #[test]
    fn reselect_keeps_index_when_branch_unchanged() {
        let after = [wt("main"), wt("feat-a")];
        assert_eq!(reselect_worktree_index(&after, "feat-a", 1), Some(1));
    }

    #[test]
    fn reselect_returns_none_for_empty_list() {
        assert_eq!(reselect_worktree_index(&[], "main", 0), None);
    }

    #[test]
    fn reselect_clamps_when_prev_branch_empty() {
        // No previously selected branch (e.g. first load): just keep the index
        // in range.
        let after = [wt("main"), wt("feat-a")];
        assert_eq!(reselect_worktree_index(&after, "", 5), Some(1));
    }
}
