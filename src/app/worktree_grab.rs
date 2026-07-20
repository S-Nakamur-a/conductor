//! Grab/ungrab flow for [`App`].
//!
//! "Grab" checks out a worktree's branch onto the main worktree (so it can be
//! driven from the primary working copy) and, if the source worktree has a
//! Claude Code session, migrates and auto-resumes it there too. "Ungrab"
//! reverses both steps.

use super::*;

impl App {
    /// Execute grab: checkout main to the selected worktree's branch.
    ///
    /// Also looks up the source worktree's latest Claude Code session and,
    /// if found, auto-resumes it on the main worktree after grabbing.
    pub fn execute_grab(&mut self, branch_name: &str) {
        // Pre-check: already grabbing another branch
        if let Some(ref grabbed) = self.worktree_mgr.grabbed_branch {
            self.set_status(
                format!("Already grabbed: {}. Ungrab first (Y).", grabbed.branch),
                StatusLevel::Warning,
            );
            return;
        }

        let main_path = match self.worktrees.iter().find(|w| w.is_main) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("Main worktree not found.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let source_path = match self.worktrees.iter().find(|w| w.branch == branch_name) {
            Some(w) => w.path.clone(),
            None => {
                self.set_status(
                    format!("Worktree for '{branch_name}' not found."),
                    StatusLevel::Error,
                );
                return;
            }
        };

        // Look up the latest Claude Code session for the source worktree.
        log::info!(
            "grab: looking up session for source_path={}",
            source_path.display()
        );
        let claude_session = crate::claude_sessions::find_latest_sessions_for_paths(
            std::slice::from_ref(&source_path),
        )
        .ok()
        .and_then(|mut map| {
            log::info!(
                "grab: session map has {} entries: {:?}",
                map.len(),
                map.keys().collect::<Vec<_>>()
            );
            let canonical =
                std::fs::canonicalize(&source_path).unwrap_or_else(|_| source_path.clone());
            log::info!("grab: canonical source_path={}", canonical.display());
            map.remove(&canonical)
        });
        let session_id = claude_session.as_ref().map(|s| s.session_id.as_str());
        log::info!("grab: found session={:?}", session_id);

        let selected_path = self
            .worktrees
            .get(self.selected_worktree)
            .map(|w| w.path.clone());
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.grab_branch(&main_path, &source_path, branch_name, session_id) {
                    Ok(()) => {
                        let claude_session_id =
                            claude_session.as_ref().map(|s| s.session_id.clone());
                        self.worktree_mgr.grabbed_branch = Some(GrabbedBranch {
                            branch: branch_name.to_string(),
                            source_worktree: source_path.clone(),
                            claude_session_id: claude_session_id.clone(),
                        });

                        // Migrate session files so `claude --resume` works from main cwd.
                        if let Some(ref session) = claude_session
                            && let Err(e) = crate::claude_sessions::migrate_session(
                                &session.session_id,
                                &source_path,
                                &main_path,
                                &session.display,
                            )
                        {
                            log::warn!("grab: session migration failed: {e}");
                        }

                        // Auto-resume the Claude Code session on main worktree.
                        let resume_msg = if let Some(ref session) = claude_session {
                            match self.resume_claude_session_on_main(&session.session_id) {
                                Ok(_) => format!(
                                    "Grabbed '{branch_name}' + resumed session {}. Press Y to ungrab.",
                                    &session.session_id[..8.min(session.session_id.len())]
                                ),
                                Err(e) => {
                                    log::warn!("grab: failed to resume session: {e}");
                                    format!(
                                        "Grabbed '{branch_name}' (session resume failed). Press Y to ungrab."
                                    )
                                }
                            }
                        } else {
                            format!(
                                "Grabbed '{branch_name}' — main is now on this branch. Press Y to ungrab."
                            )
                        };
                        self.set_status(resume_msg, StatusLevel::Success);

                        self.refresh_worktrees();
                        if let Some(path) = selected_path {
                            self.select_worktree_by_path(&path);
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Grab error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Resume a Claude Code session on the main worktree.
    fn resume_claude_session_on_main(&mut self, session_id: &str) -> anyhow::Result<usize> {
        let main_wt = self.worktrees.iter().find(|w| w.is_main);
        let (worktree_name, working_dir) = match main_wt {
            Some(w) => (w.branch.clone(), w.path.clone()),
            None => anyhow::bail!("main worktree not found"),
        };

        // Use a short numbered label consistent with spawn_claude_code.
        let cc_count = self
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| {
                s.working_dir == working_dir && s.kind == pty_manager::SessionKind::ClaudeCode
            })
            .count();
        let label = format!("CC:{}", cc_count + 1);
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::ClaudeCode,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            Some(session_id),
            &self.repo_path,
            None,
        )?;
        self.switch_claude_session(idx);
        Ok(idx)
    }

    /// Execute ungrab: return main to main branch, restore worktree to original branch.
    pub fn execute_ungrab(&mut self) {
        let grabbed = match self.worktree_mgr.grabbed_branch.clone() {
            Some(g) => g,
            None => {
                self.set_status("Not grabbing any branch.".to_string(), StatusLevel::Warning);
                return;
            }
        };
        let main_path = match self.worktrees.iter().find(|w| w.is_main) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("Main worktree not found.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let selected_path = self
            .worktrees
            .get(self.selected_worktree)
            .map(|w| w.path.clone());
        let main_branch = self.config.general.main_branch.clone();
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.ungrab_branch(
                    &main_path,
                    &grabbed.source_worktree,
                    &grabbed.branch,
                    &main_branch,
                ) {
                    Ok(()) => {
                        // Clean up migrated session files and copy back any
                        // conversation data that Claude Code wrote as real files.
                        if let Some(ref sid) = grabbed.claude_session_id
                            && let Err(e) = crate::claude_sessions::unmigrate_session(
                                sid,
                                &grabbed.source_worktree,
                                &main_path,
                            )
                        {
                            log::warn!("ungrab: session unmigration failed: {e}");
                        }

                        let branch = grabbed.branch.clone();
                        self.worktree_mgr.grabbed_branch = None;
                        self.set_status(
                            format!("Ungrabbed '{branch}' — main restored."),
                            StatusLevel::Success,
                        );
                        self.refresh_worktrees();
                        if let Some(path) = selected_path {
                            self.select_worktree_by_path(&path);
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Ungrab error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }
}
