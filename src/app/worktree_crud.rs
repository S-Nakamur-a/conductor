//! Worktree create/delete lifecycle for [`App`].
//!
//! Owns the background-thread create/delete flow: kicking off the git
//! operation on a worker thread, tracking [`PendingWorktree`] entries while
//! it runs, and applying the result (or timeout) once it completes.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use super::*;

impl App {
    /// Create a worktree from a base ref (2-step flow) — runs in a background thread.
    pub fn create_worktree_from_base(&mut self, branch_name: &str, base_ref: &str) {
        let base = if base_ref.is_empty() {
            "origin/main"
        } else {
            base_ref
        };

        let pending = PendingWorktree {
            branch: branch_name.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: base.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            session_name: None,
            delete_branch_after: false,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(
            format!("Creating worktree '{branch_name}'..."),
            StatusLevel::Info,
        );

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();
        let branch = branch_name.to_string();
        let base_owned = base.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path).and_then(|engine| {
                engine.create_worktree_from_base(&branch, &base_owned, wt_dir.as_deref())
            });
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed {
                    error: format!("{e}"),
                    pending,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// Create a worktree from a remote branch — runs in a background thread.
    pub fn create_worktree_from_remote(&mut self, remote_branch: &str) {
        let local_branch = remote_branch
            .strip_prefix("origin/")
            .unwrap_or(remote_branch);

        let pending = PendingWorktree {
            branch: local_branch.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: remote_branch.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            session_name: None,
            delete_branch_after: false,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(
            format!("Creating worktree '{local_branch}'..."),
            StatusLevel::Info,
        );

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();
        let remote = remote_branch.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.create_worktree_from_remote(&remote, wt_dir.as_deref()));
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed {
                    error: format!("{e}"),
                    pending,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// Delete a branch (optionally force).
    pub fn delete_branch(&mut self, name: &str, force: bool) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => match engine.delete_branch(name, force) {
                Ok(()) => {
                    let mode = if force { "force-deleted" } else { "deleted" };
                    self.set_status(format!("Branch {mode}: {name}"), StatusLevel::Success);
                }
                Err(e) => {
                    self.set_status(format!("Branch delete error: {e}"), StatusLevel::Error);
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    pub fn delete_selected_worktree(&mut self, delete_branch_after: bool) {
        let wt = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt,
            None => return,
        };

        if wt.is_main {
            self.set_status(
                "Cannot delete the main worktree.".to_string(),
                StatusLevel::Error,
            );
            return;
        }

        let wt_path = wt.path.clone();
        let branch = wt.branch.clone();

        // Kill all PTY sessions (Claude Code + Shell) associated with this worktree
        // before removing the worktree directory. Walk backwards so removals don't
        // shift indices we haven't processed yet.
        let session_indices: Vec<usize> = self
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path)
            .map(|(idx, _)| idx)
            .collect();
        for &idx in session_indices.iter().rev() {
            log::info!("killing PTY session {idx} for deleted worktree '{branch}'");
            self.close_terminal_session(idx);
        }

        // Add pending entry and run git removal in a background thread.
        let pending = PendingWorktree {
            branch: branch.clone(),
            op: PendingWorktreeOp::Deleting,
            base_ref: String::new(),
            worktree_path: Some(wt_path.clone()),
            auto_spawn: false,
            smart_prompt: String::new(),
            session_name: None,
            delete_branch_after,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending);
        self.set_status(
            format!("Deleting worktree '{branch}'..."),
            StatusLevel::Info,
        );

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.remove_worktree(&wt_path));
            let msg = match result {
                Ok(()) => WorktreeOpResult::Deleted { branch },
                Err(e) => WorktreeOpResult::DeleteFailed {
                    error: format!("{e}"),
                    branch,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// Check if a worktree at the given path is pending deletion.
    pub fn is_worktree_pending_delete(&self, path: &std::path::Path) -> bool {
        self.worktree_mgr.pending_worktrees.iter().any(|p| {
            p.op == PendingWorktreeOp::Deleting && p.worktree_path.as_deref() == Some(path)
        })
    }

    /// Poll for completed background worktree create/delete results.
    pub fn poll_worktree_ops(&mut self) {
        let rx = match self.worktree_mgr.bg_worktree_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        let mut results = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(result) => results.push(result),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worktree_mgr.bg_worktree_rx = None;
                    self.worktree_mgr.bg_worktree_tx = None;
                    // Clean up any pending create/smart-create entries that will never complete.
                    let orphaned: Vec<_> = self
                        .worktree_mgr
                        .pending_worktrees
                        .iter()
                        .filter(|p| {
                            matches!(
                                p.op,
                                PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                            )
                        })
                        .map(|p| p.description.clone())
                        .collect();
                    if !orphaned.is_empty() {
                        self.worktree_mgr.pending_worktrees.retain(|p| {
                            !matches!(
                                p.op,
                                PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                            )
                        });
                        log::warn!(
                            "Cleaned up {} orphaned pending worktrees on channel disconnect",
                            orphaned.len()
                        );
                        self.set_status(
                            "Worktree creation interrupted (channel disconnected)".to_string(),
                            StatusLevel::Error,
                        );
                    }
                    break;
                }
            }
        }
        for result in results {
            self.handle_worktree_op_result(result);
        }

        // Timeout detection: warn if any pending create/smart-create has been running too long.
        const TIMEOUT_SECS: u64 = 120;
        let now = std::time::Instant::now();
        let timed_out: Vec<_> = self
            .worktree_mgr
            .pending_worktrees
            .iter()
            .filter(|p| {
                matches!(
                    p.op,
                    PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                ) && now.duration_since(p.created_at).as_secs() >= TIMEOUT_SECS
            })
            .map(|p| {
                if p.description.is_empty() {
                    p.branch.clone()
                } else {
                    p.description.clone()
                }
            })
            .collect();
        if !timed_out.is_empty() {
            self.worktree_mgr.pending_worktrees.retain(|p| {
                !(matches!(
                    p.op,
                    PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                ) && now.duration_since(p.created_at).as_secs() >= TIMEOUT_SECS)
            });
            let names = timed_out.join(", ");
            log::warn!("Timed out pending worktrees: {names}");
            self.set_status(
                format!("Worktree creation timed out: {names}"),
                StatusLevel::Error,
            );
        }
    }

    fn handle_worktree_op_result(&mut self, result: WorktreeOpResult) {
        match result {
            WorktreeOpResult::Created { path, pending } => {
                // Remove from pending list (matches both Creating and SmartCreating).
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating
                        || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });

                self.new_worktree_paths.insert(path.clone());
                self.record_stat("branches_created");
                if let Some(store) = &self.review_store {
                    let _ = store.save_worktree_base_branch(&pending.branch, &pending.base_ref);
                }
                self.refresh_worktrees();
                // Preserve the current focus and selected worktree — don't
                // switch the user's view to the newly created worktree.
                let prev_selected = self.selected_worktree;
                let prev_focus = self.focus;
                self.set_status(
                    format!(
                        "Created worktree: {} (from {})",
                        path.display(),
                        pending.base_ref
                    ),
                    StatusLevel::Success,
                );

                // Smart Worktree: auto-spawn Claude Code and defer prompt
                // until the session is ready for input.
                if pending.auto_spawn {
                    // Temporarily select the new worktree so spawn_claude_code
                    // picks up the correct working directory.
                    // Use direct index assignment instead of select_worktree_by_path
                    // to avoid on_worktree_changed() clearing the 🌱 new-worktree badge.
                    if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
                        self.selected_worktree = idx;
                    }
                    match self.spawn_claude_code_with_name(pending.session_name.as_deref()) {
                        Ok(idx) => {
                            if !pending.smart_prompt.is_empty() {
                                self.terminal
                                    .deferred_prompts
                                    .insert(idx, pending.smart_prompt.clone());
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to auto-spawn Claude Code: {e}");
                        }
                    }
                    // Restore the previous worktree selection and focus.
                    self.selected_worktree = prev_selected;
                    self.on_worktree_changed();
                    self.focus = prev_focus;
                }
            }
            WorktreeOpResult::CreateFailed { error, pending } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating
                        || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::Deleted { ref branch } => {
                let delete_branch_after = self.worktree_mgr.pending_worktrees.iter().any(|p| {
                    p.op == PendingWorktreeOp::Deleting
                        && p.branch == *branch
                        && p.delete_branch_after
                });
                self.worktree_mgr
                    .pending_worktrees
                    .retain(|p| !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch));
                self.refresh_worktrees();
                // If the worktree currently shown in the Explorer/Claude/Shell
                // panels was the one removed, `refresh_worktrees` has slid the
                // selection onto a surviving worktree (e.g. main) — but the
                // panels still point at the gone worktree and render blank.
                // Reload them to match the new selection, exactly as a normal
                // switch would. (Deleting some *other* worktree leaves the
                // selection intact, so this is a no-op then.)
                let selected_branch = self.selected_worktree_branch();
                let view_branch = self.current_view_branch.clone().unwrap_or_default();
                if selected_branch != view_branch {
                    self.on_worktree_changed();
                }
                self.set_status(format!("Deleted worktree: {branch}"), StatusLevel::Success);

                if delete_branch_after {
                    self.delete_branch(branch, true);
                }
            }
            WorktreeOpResult::DeleteFailed { error, ref branch } => {
                self.worktree_mgr
                    .pending_worktrees
                    .retain(|p| !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch));
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::Skipped {
                ref branch,
                ref reason,
            } => {
                self.worktree_mgr
                    .pending_worktrees
                    .retain(|p| p.branch != *branch);
                self.worktree_mgr.skip_reason = Some(reason.clone());
            }
            WorktreeOpResult::SmartBranchResolved {
                ref description,
                ref branch,
                ref prompt,
                ref session_name,
            } => {
                // Update the pending entry: set branch name, prompt, and session name.
                for p in &mut self.worktree_mgr.pending_worktrees {
                    if p.op == PendingWorktreeOp::SmartCreating && p.description == *description {
                        p.branch = branch.clone();
                        p.smart_prompt = prompt.clone();
                        p.session_name = session_name.clone();
                        break;
                    }
                }
                self.set_status(
                    format!("Smart worktree: creating '{branch}'... (Esc to cancel)"),
                    StatusLevel::Info,
                );
            }
            WorktreeOpResult::SmartFailed {
                ref description,
                ref error,
            } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::SmartCreating && p.description == *description)
                });
                // Suppress error message if the operation was cancelled by user.
                if error == "Cancelled" {
                    log::info!("Smart worktree cancelled for: {description}");
                } else {
                    log::warn!("Smart worktree failed: {error}");
                    self.set_status(
                        format!("Smart worktree failed: {error}"),
                        StatusLevel::Error,
                    );
                }
            }
        }
    }
}
