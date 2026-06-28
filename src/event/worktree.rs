//! Worktree panel key handling.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus, StatusLevel, WorktreeListRow};
use crate::git_engine;
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;

/// Handle keys when the Worktree panel is focused.
pub(super) fn handle_worktree_key(app: &mut App, key: KeyEvent) {
    // Esc cancels any pending smart worktree creation; otherwise it closes the
    // worktree switcher modal (this handler now backs that modal).
    if key.code == KeyCode::Esc {
        if app.cancel_smart_worktrees() {
            return;
        }
        app.overlays.active = ActiveOverlay::None;
        return;
    }

    let action = app.keymap.resolve(&key, KeyContext::Worktree);
    match action {
        Some(Action::NavigateDown) if !app.worktree_list_rows.is_empty() => {
            let prev_wt = app.selected_worktree;
            app.worktree_list_selected =
                (app.worktree_list_selected + 1) % app.worktree_list_rows.len();
            app.sync_selected_worktree();
            if app.selected_worktree != prev_wt {
                app.on_worktree_changed();
            }
        }
        Some(Action::NavigateUp) if !app.worktree_list_rows.is_empty() => {
            let prev_wt = app.selected_worktree;
            app.worktree_list_selected = if app.worktree_list_selected == 0 {
                app.worktree_list_rows.len() - 1
            } else {
                app.worktree_list_selected - 1
            };
            app.sync_selected_worktree();
            if app.selected_worktree != prev_wt {
                app.on_worktree_changed();
            }
        }
        Some(Action::Select) => {
            // Selecting commits the choice and closes the switcher modal.
            app.overlays.active = ActiveOverlay::None;
            match app
                .worktree_list_rows
                .get(app.worktree_list_selected)
                .copied()
            {
                Some(WorktreeListRow::Session { pty_idx, .. }) => {
                    app.switch_claude_session(pty_idx);
                    app.set_focus(Focus::TerminalClaude);
                }
                Some(WorktreeListRow::Worktree(_)) | None => {
                    app.on_worktree_changed();
                    app.set_focus(Focus::Explorer);
                }
            }
        }
        Some(Action::CreateWorktree) => {
            app.worktree_mgr.input_mode = crate::app::WorktreeInputMode::CreatingWorktree;
            app.worktree_mgr.input_buffer.clear();
            app.set_status(
                "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string(),
                StatusLevel::Info,
            );
        }
        Some(Action::DeleteWorktree) => {
            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                if wt.is_main {
                    app.set_status(
                        "Cannot delete the main worktree.".to_string(),
                        StatusLevel::Error,
                    );
                } else if app.is_worktree_pending_delete(&wt.path) {
                    app.set_status(
                        "Worktree is already being deleted.".to_string(),
                        StatusLevel::Warning,
                    );
                } else {
                    app.worktree_mgr.input_mode = crate::app::WorktreeInputMode::ConfirmingDelete;
                    app.set_status(
                        format!("Delete worktree '{}'? (y/n)", wt.branch),
                        StatusLevel::Warning,
                    );
                }
            }
        }
        Some(Action::SwitchBranch) => {
            app.set_status("Loading branches...".to_string(), StatusLevel::Info);
            app.load_switch_branches();
            if !app.overlays.switch_branch.branches.is_empty() {
                app.overlays.active = ActiveOverlay::SwitchBranch;
                app.status_message = None;
            } else if app
                .status_message
                .as_ref()
                .is_some_and(|m| m.text == "Loading branches...")
            {
                app.set_status(
                    "No remote branches found.".to_string(),
                    StatusLevel::Warning,
                );
            }
        }
        Some(Action::GrabBranch) => {
            if app.worktree_mgr.grabbed_branch.is_some() {
                app.set_status(
                    "Already grabbing a branch. Ungrab first (G).".to_string(),
                    StatusLevel::Warning,
                );
            } else {
                app.load_grab_branches();
                if app.overlays.grab.branches.is_empty() {
                    app.set_status(
                        "No non-main worktrees to grab.".to_string(),
                        StatusLevel::Warning,
                    );
                } else {
                    app.overlays.active = ActiveOverlay::Grab;
                }
            }
        }
        Some(Action::UngrabBranch) => {
            if app.worktree_mgr.grabbed_branch.is_none() {
                app.set_status(
                    "Not grabbing — nothing to ungrab.".to_string(),
                    StatusLevel::Warning,
                );
            } else {
                app.worktree_mgr.input_mode = crate::app::WorktreeInputMode::ConfirmingUngrab;
                app.set_status(
                    "Ungrab? Main will return to main branch. (y/n)".to_string(),
                    StatusLevel::Warning,
                );
            }
        }
        Some(Action::PruneWorktrees) => match git_engine::GitEngine::open(&app.repo_path) {
            Ok(engine) => match engine.find_stale_worktrees() {
                Ok(stale) => {
                    if stale.is_empty() {
                        app.set_status("No stale worktrees found.".to_string(), StatusLevel::Info);
                    } else {
                        app.overlays.prune.stale = stale;
                        app.overlays.active = ActiveOverlay::Prune;
                    }
                }
                Err(e) => {
                    app.set_status(format!("Error: {e}"), StatusLevel::Error);
                }
            },
            Err(e) => {
                app.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        },
        Some(Action::MergeToMain) => {
            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                if wt.is_main {
                    app.set_status(
                        "Cannot merge main into itself.".to_string(),
                        StatusLevel::Error,
                    );
                } else {
                    let branch = wt.branch.clone();
                    let main_branch = app.config.general.main_branch.clone();
                    match git_engine::GitEngine::open(&app.repo_path) {
                        Ok(engine) => match engine.merge_into_main(&branch, &main_branch) {
                            Ok(msg) => {
                                app.set_status(msg, StatusLevel::Success);
                                app.refresh_worktrees();
                            }
                            Err(e) => {
                                app.set_status(format!("Merge error: {e}"), StatusLevel::Error);
                            }
                        },
                        Err(e) => {
                            app.set_status(format!("Error: {e}"), StatusLevel::Error);
                        }
                    }
                }
            }
        }
        Some(Action::PullWorktree) => {
            app.start_pull_worktree();
        }
        Some(Action::SessionHistory) => {
            app.overlays.active = ActiveOverlay::History;
            app.load_session_history();
        }
        Some(Action::RefreshWorktrees) => {
            app.refresh_worktrees();
        }
        Some(Action::ResetMainToOrigin) => {
            // Confirm first — this discards local commits.
            app.cmd_reset_main_to_origin();
        }
        Some(Action::OpenPullRequest) => {
            app.open_pr_in_browser();
        }
        Some(Action::CherryPick) => {
            let current_branch = app
                .worktrees
                .get(app.selected_worktree)
                .map(|w| w.branch.clone())
                .unwrap_or_default();
            let source = app
                .worktrees
                .iter()
                .find(|w| w.branch != current_branch)
                .map(|w| w.branch.clone());
            if let Some(branch) = source {
                app.overlays.cherry_pick.source_branch = branch;
                app.load_cherry_pick_commits();
                app.overlays.active = ActiveOverlay::CherryPick;
            } else {
                app.set_status(
                    "No other worktree branches available.".to_string(),
                    StatusLevel::Warning,
                );
            }
        }
        _ => {}
    }
}
