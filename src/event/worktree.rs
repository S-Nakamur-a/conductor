//! Worktree パネルのキー処理。

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus, StatusLevel, WorktreeListRow};
use crate::git_engine;
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;

/// Worktree パネルがフォーカスされている間のキーを処理する。
pub(super) fn handle_worktree_key(app: &mut App, key: KeyEvent) {
    // Esc は保留中のスマート worktree 作成があればそれをキャンセルする。
    // そうでなければ worktree 切り替えモーダルを閉じる (このハンドラは
    // 今そのモーダルの裏方も兼ねている)。
    if key.code == KeyCode::Esc {
        if app.cancel_smart_worktrees() {
            return;
        }
        app.overlays.active = ActiveOverlay::None;
        return;
    }

    let action = app.keymap.resolve(&key, KeyContext::Worktree);
    match action {
        Some(Action::NavigateDown) if !app.worktrees.rows.is_empty() => {
            let prev_wt = app.worktrees.selected_index();
            app.worktrees.row_selected =
                (app.worktrees.row_selected + 1) % app.worktrees.rows.len();
            app.sync_selected_worktree();
            if app.worktrees.selected_index() != prev_wt {
                app.on_worktree_changed();
            }
        }
        Some(Action::NavigateUp) if !app.worktrees.rows.is_empty() => {
            let prev_wt = app.worktrees.selected_index();
            app.worktrees.row_selected = if app.worktrees.row_selected == 0 {
                app.worktrees.rows.len() - 1
            } else {
                app.worktrees.row_selected - 1
            };
            app.sync_selected_worktree();
            if app.worktrees.selected_index() != prev_wt {
                app.on_worktree_changed();
            }
        }
        Some(Action::Select) => {
            // 選択を確定して切り替えモーダルを閉じる。
            app.overlays.active = ActiveOverlay::None;
            match app.worktrees.rows.get(app.worktrees.row_selected).copied() {
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
            if let Some(wt) = app.worktrees.selected() {
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
                    // 確定すると worktree とそのブランチの両方を削除する
                    // (force)。反射的な y で作業を黙って失わせないよう、
                    // 何が失われるかを明示する: 未コミットの変更はディレクトリ
                    // ごと消え、main にマージされていないコミットはブランチ
                    // ごと到達不能になる。
                    let branch = wt.branch.clone();
                    let dirty_count = wt.added + wt.modified + wt.deleted;
                    let is_clean = wt.is_clean;
                    let main_branch = app
                        .worktrees
                        .iter()
                        .find(|w| w.is_main)
                        .map(|w| w.branch.clone());

                    let mut warnings: Vec<String> = Vec::new();
                    if !is_clean {
                        warnings.push(format!("{dirty_count} uncommitted change(s) will be LOST"));
                    }
                    if let Some(main_branch) = main_branch.filter(|m| *m != branch) {
                        match git_engine::GitEngine::open(&app.repo.path)
                            .and_then(|e| e.is_branch_merged_into(&branch, &main_branch))
                        {
                            Ok(false) => warnings.push(format!(
                                "commits not merged into '{main_branch}' will be lost with the branch"
                            )),
                            Ok(true) => {}
                            // 確認できない場合 (例: ブランチ名変更後) は削除を
                            // ブロックしないが、安全だとも主張しない。
                            Err(e) => {
                                log::warn!("merged-into-main check failed for '{branch}': {e}");
                            }
                        }
                    }

                    app.worktree_mgr.input_mode = crate::app::WorktreeInputMode::ConfirmingDelete;
                    let prompt = if warnings.is_empty() {
                        format!("Delete worktree '{branch}' and its branch? (y/n)")
                    } else {
                        format!(
                            "Delete worktree '{branch}'? WARNING: {} (y/n)",
                            warnings.join("; ")
                        )
                    };
                    app.set_status(prompt, StatusLevel::Warning);
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
        Some(Action::PruneWorktrees) => match git_engine::GitEngine::open(&app.repo.path) {
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
            if let Some(wt) = app.worktrees.selected() {
                if wt.is_main {
                    app.set_status(
                        "Cannot merge main into itself.".to_string(),
                        StatusLevel::Error,
                    );
                } else {
                    let branch = wt.branch.clone();
                    let main_branch = app.config.general.main_branch.clone();
                    match git_engine::GitEngine::open(&app.repo.path) {
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
            // 先に確認する — これはローカルのコミットを破棄する。
            app.cmd_reset_main_to_origin();
        }
        Some(Action::OpenPullRequest) => {
            app.open_pr_in_browser();
        }
        Some(Action::CherryPick) => {
            let current_branch = app
                .worktrees
                .get(app.worktrees.selected_index())
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
