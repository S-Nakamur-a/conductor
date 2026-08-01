//! Claude Code session resume for [`App`].
//!
//! Loading and filtering resumable sessions from Claude's on-disk history,
//! resuming a chosen session by ID, and the startup auto-resume flow that
//! reattaches the previous session for each worktree.

use std::path::PathBuf;

use super::*;

impl App {
    /// Load resumable Claude Code sessions from Claude's history.
    pub fn load_resume_sessions(&mut self) {
        let filter = if self.overlays.resume_session.all_projects {
            None
        } else {
            Some(self.repo.path.as_path())
        };
        match crate::claude_sessions::load_resumable_sessions(filter) {
            Ok(sessions) => {
                self.overlays.resume_session.sessions = sessions;
                self.overlays.resume_session.selected = 0;
                self.overlays.resume_session.filter.clear();
            }
            Err(e) => {
                log::warn!("failed to load resumable sessions: {e}");
                self.overlays.resume_session.sessions.clear();
                self.set_status(format!("Error loading sessions: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Return the filtered list of resume sessions based on the current filter string.
    pub fn filtered_resume_sessions(
        &self,
    ) -> Vec<(usize, &crate::claude_sessions::ResumableSession)> {
        if self.overlays.resume_session.filter.is_empty() {
            self.overlays
                .resume_session
                .sessions
                .iter()
                .enumerate()
                .collect()
        } else {
            let filter_lower = self.overlays.resume_session.filter.to_lowercase();
            self.overlays
                .resume_session
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.display.to_lowercase().contains(&filter_lower)
                        || s.session_id.to_lowercase().contains(&filter_lower)
                        || s.project_name.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Resume a Claude Code session by its session ID.
    pub fn resume_claude_session(
        &mut self,
        session_id: &str,
        display: &str,
    ) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let label: String = display.chars().take(40).collect();
        let label = if label.is_empty() {
            format!("Resume:{}", &session_id[..8.min(session_id.len())])
        } else {
            label
        };
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
            &self.repo.path,
            None,
        )?;
        self.switch_claude_session(idx);
        Ok(idx)
    }

    /// Automatically resume Claude Code sessions for all worktrees that had a
    /// previous session. Called once after the first frame render.
    pub fn perform_auto_resume(&mut self) {
        if !self.pending_auto_resume {
            return;
        }
        self.pending_auto_resume = false;

        let paths: Vec<PathBuf> = self.worktrees.iter().map(|w| w.path.clone()).collect();
        if paths.is_empty() {
            return;
        }

        let sessions = match crate::claude_sessions::find_latest_sessions_for_paths(&paths) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("auto-resume: failed to find sessions: {e}");
                return;
            }
        };

        if sessions.is_empty() {
            return;
        }

        // If we have a grabbed branch with a session ID, use it for the main worktree
        // instead of whatever auto-resume would normally find (since the session was
        // created in the source worktree, not the main worktree).
        let grabbed_session_for_main = self
            .worktree_mgr
            .grabbed_branch
            .as_ref()
            .and_then(|g| g.claude_session_id.clone());

        let selected_wt_path = self.selected_worktree_path();
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let repo_path = self.repo.path.clone();
        let mut resumed_count = 0;

        for wt in &self.worktrees.to_vec() {
            let canonical = std::fs::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());

            // For main worktree with a grabbed session, prefer the grabbed session ID.
            if wt.is_main
                && let Some(ref grabbed_id) = grabbed_session_for_main
            {
                let label = format!("Resume:{}", &grabbed_id[..8.min(grabbed_id.len())]);
                match self.terminal.pty_manager.spawn_session(
                    pty_manager::SessionKind::ClaudeCode,
                    &wt.branch,
                    &label,
                    &shell,
                    &wt.path,
                    rows,
                    cols,
                    Some(grabbed_id),
                    &repo_path,
                    None,
                ) {
                    Ok(idx) => {
                        resumed_count += 1;
                        if wt.path == selected_wt_path {
                            self.switch_claude_session(idx);
                        }
                    }
                    Err(e) => {
                        log::warn!("auto-resume: failed to resume grabbed session for main: {e}");
                    }
                }
                continue;
            }

            // Skip normal auto-resume for the main worktree unless explicitly
            // opted in.  Grabbed sessions (handled above) are always resumed.
            if wt.is_main && !self.config.general.auto_resume_main {
                continue;
            }

            let session = match sessions.get(&canonical) {
                Some(s) => s,
                None => continue,
            };

            let label: String = session.display.chars().take(40).collect();
            let label = if label.is_empty() {
                format!(
                    "Resume:{}",
                    &session.session_id[..8.min(session.session_id.len())]
                )
            } else {
                label
            };

            match self.terminal.pty_manager.spawn_session(
                pty_manager::SessionKind::ClaudeCode,
                &wt.branch,
                &label,
                &shell,
                &wt.path,
                rows,
                cols,
                Some(&session.session_id),
                &repo_path,
                None,
            ) {
                Ok(idx) => {
                    resumed_count += 1;
                    // Only switch to this session for the currently selected worktree.
                    if wt.path == selected_wt_path {
                        self.switch_claude_session(idx);
                    }
                }
                Err(e) => {
                    log::warn!(
                        "auto-resume: failed to spawn session for {}: {e}",
                        wt.branch
                    );
                }
            }
        }

        if resumed_count > 0 {
            self.set_status(
                format!("Auto-resumed {resumed_count} Claude session(s)"),
                StatusLevel::Success,
            );
        }
    }
}
