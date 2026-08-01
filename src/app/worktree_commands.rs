//! Worktree-management command handlers: create/delete/switch/grab/prune
//! worktrees, merge-to-main, reset-main-to-origin, and cherry-pick — the
//! confirmation-gated entry points invoked from the command palette and
//! keybindings.

use super::{App, StatusLevel, WorktreeInputMode};
use crate::overlay::ActiveOverlay;

impl App {
    pub(super) fn cmd_create_worktree(&mut self) {
        self.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
        self.worktree_mgr.input_buffer.clear();
        self.set_status_info(
            "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string(),
        );
    }

    pub(super) fn cmd_delete_worktree(&mut self) {
        if let Some(wt) = self.worktrees.selected() {
            if wt.is_main {
                self.set_status(
                    "Cannot delete the main worktree.".to_string(),
                    StatusLevel::Warning,
                );
            } else {
                let branch = wt.branch.clone();
                self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingDelete;
                self.set_status_info(format!("Delete worktree '{branch}'? (y/n)"));
            }
        }
    }

    pub(super) fn cmd_switch_branch(&mut self) {
        self.set_status_info("Loading branches...".to_string());
        self.load_switch_branches();
        if !self.overlays.switch_branch.branches.is_empty() {
            self.overlays.active = ActiveOverlay::SwitchBranch;
            self.status_message = None;
        }
    }

    pub(super) fn cmd_grab_branch(&mut self) {
        if self.worktree_mgr.grabbed_branch.is_some() {
            self.set_status(
                "Already grabbing a branch. Ungrab first (G).".to_string(),
                StatusLevel::Warning,
            );
        } else {
            self.load_grab_branches();
            if self.overlays.grab.branches.is_empty() {
                self.set_status_info("No non-main worktrees to grab.".to_string());
            } else {
                self.overlays.active = ActiveOverlay::Grab;
            }
        }
    }

    pub(super) fn cmd_prune_worktrees(&mut self) {
        match crate::git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.find_stale_worktrees() {
                Ok(stale) => {
                    if stale.is_empty() {
                        self.set_status_info("No stale worktrees found.".to_string());
                    } else {
                        self.overlays.prune.stale = stale;
                        self.overlays.active = ActiveOverlay::Prune;
                    }
                }
                Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
            },
            Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
        }
    }

    pub(super) fn cmd_merge_to_main(&mut self) {
        if let Some(wt) = self.worktrees.selected() {
            if wt.is_main {
                self.set_status(
                    "Cannot merge main into itself.".to_string(),
                    StatusLevel::Warning,
                );
            } else {
                let branch = wt.branch.clone();
                let main_branch = self.config.general.main_branch.clone();
                match crate::git_engine::GitEngine::open(&self.repo.path) {
                    Ok(engine) => match engine.merge_into_main(&branch, &main_branch) {
                        Ok(msg) => {
                            self.set_status(msg, StatusLevel::Success);
                            self.refresh_worktrees();
                        }
                        Err(e) => self.set_status(format!("Merge error: {e}"), StatusLevel::Error),
                    },
                    Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
                }
            }
        }
    }

    /// Ask before resetting main — this discards local commits, so it must not
    /// fire on a bare keystroke (`R` sits next to `r` refresh). The actual reset
    /// runs in [`perform_reset_main_to_origin`](Self::perform_reset_main_to_origin)
    /// once confirmed. Both the `R` key and the palette enter through here.
    pub fn cmd_reset_main_to_origin(&mut self) {
        let main_branch = self.config.general.main_branch.clone();
        self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingReset;
        self.set_status_info(format!(
            "Reset '{main_branch}' to origin? Discards local commits on it. (y/n)"
        ));
    }

    /// Perform the hard reset of main to its origin tracking branch. Call only
    /// after the user confirms (see [`cmd_reset_main_to_origin`](Self::cmd_reset_main_to_origin)).
    pub fn perform_reset_main_to_origin(&mut self) {
        let main_branch = self.config.general.main_branch.clone();
        match crate::git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.reset_main_to_origin(&main_branch) {
                Ok(msg) => {
                    self.set_status(msg, StatusLevel::Success);
                    self.refresh_worktrees();
                }
                Err(e) => self.set_status(format!("Reset error: {e}"), StatusLevel::Error),
            },
            Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
        }
    }

    pub(super) fn cmd_cherry_pick(&mut self) {
        let current_branch = self.selected_worktree_branch();
        let source = self
            .worktrees
            .iter()
            .find(|w| w.branch != current_branch)
            .map(|w| w.branch.clone());
        if let Some(branch) = source {
            self.overlays.cherry_pick.source_branch = branch;
            self.load_cherry_pick_commits();
            self.overlays.active = ActiveOverlay::CherryPick;
        } else {
            self.set_status_info("No other worktree branches available.".to_string());
        }
    }
}
