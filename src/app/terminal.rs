//! Terminal / PTY / Permission methods for [`App`].
//!
//! This module contains methods for spawning and managing Claude Code and Shell
//! PTY sessions, permission auto-response, and related helpers.

use std::collections::HashSet;
use std::path::PathBuf;
use super::*;

const SESSION_ICONS: &[&str] = &[
    "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

impl App {
    /// Spawn a new Claude Code PTY session for the currently selected worktree.
    pub fn spawn_claude_code(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let used_ids: Vec<&str> = self
            .terminal.pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::ClaudeCode)
            .filter_map(|s| s.label.strip_prefix("CC:"))
            .collect();
        let id = SESSION_ICONS
            .iter()
            .find(|e| !used_ids.contains(e))
            .unwrap_or(&SESSION_ICONS[used_ids.len() % SESSION_ICONS.len()]);
        let label = format!("CC:{id}");
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
            None,
            &self.repo_path,
        )?;
        self.terminal.pty_manager.activate_session(idx);
        self.terminal.active_claude_session = Some(idx);
        self.rebuild_worktree_list_rows();
        Ok(idx)
    }

    /// Spawn a new interactive shell PTY session for the currently selected worktree.
    pub fn spawn_shell(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let sh_count = self
            .terminal.pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::Shell)
            .count();
        let label = format!("SH:{}", sh_count + 1);
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_shell;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::Shell,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            None,
            &self.repo_path,
        )?;
        self.terminal.pty_manager.activate_session(idx);
        self.terminal.active_shell_session = Some(idx);
        Ok(idx)
    }

    /// Close (kill + remove) a terminal session by its global index.
    ///
    /// Adjusts `active_claude_session` and `active_shell_session` indices
    /// and falls back to the next available session for the current worktree.
    pub fn close_terminal_session(&mut self, global_idx: usize) {
        // Kill and remove the session.
        let _ = self.terminal.pty_manager.kill_session(global_idx);
        self.terminal.pty_manager.remove_session(global_idx);

        // Adjust deferred prompts: remove the closed session, shift higher indices.
        self.terminal.deferred_prompts.remove(&global_idx);
        let shifted: Vec<(usize, String)> = self
            .terminal
            .deferred_prompts
            .drain()
            .map(|(k, v)| (if k > global_idx { k - 1 } else { k }, v))
            .collect();
        self.terminal.deferred_prompts.extend(shifted);

        // Adjust active session indices.
        for a in [&mut self.terminal.active_claude_session, &mut self.terminal.active_shell_session]
            .into_iter()
            .flatten()
        {
            if *a == global_idx {
                *a = usize::MAX; // mark for clear
            } else if *a > global_idx {
                *a -= 1;
            }
        }

        // Clear invalidated indices and fall back to next available session.
        if self.terminal.active_claude_session == Some(usize::MAX) {
            self.terminal.active_claude_session = self
                .current_worktree_claude_sessions()
                .first()
                .map(|(idx, _)| *idx);
        }
        if self.terminal.active_shell_session == Some(usize::MAX) {
            self.terminal.active_shell_session = self
                .current_worktree_shell_sessions()
                .first()
                .map(|(idx, _)| *idx);
        }
        self.rebuild_worktree_list_rows();
    }

    /// Remove PTY sessions whose child processes have exited.
    ///
    /// Iterates in reverse to preserve indices of earlier sessions while
    /// removing later ones. Adjusts `active_claude_session` and
    /// `active_shell_session` indices after removal.
    pub fn cleanup_dead_sessions(&mut self) -> bool {
        let count = self.terminal.pty_manager.session_count();
        let mut removed_any = false;

        // Walk backwards so removals don't shift indices we haven't checked yet.
        for idx in (0..count).rev() {
            if !self.terminal.pty_manager.is_session_alive(idx) {
                log::info!("removing dead PTY session at index {idx}");
                self.terminal.pty_manager.remove_session(idx);
                removed_any = true;

                // Adjust deferred prompts.
                self.terminal.deferred_prompts.remove(&idx);
                let shifted: Vec<(usize, String)> = self
                    .terminal
                    .deferred_prompts
                    .drain()
                    .map(|(k, v)| (if k > idx { k - 1 } else { k }, v))
                    .collect();
                self.terminal.deferred_prompts.extend(shifted);

                // Adjust active session indices.
                for a in [&mut self.terminal.active_claude_session, &mut self.terminal.active_shell_session].into_iter().flatten() {
                    if *a == idx {
                        *a = usize::MAX; // mark for clear
                    } else if *a > idx {
                        *a -= 1;
                    }
                }
            }
        }

        if removed_any {
            // Clear any indices that were pointing at removed sessions.
            if self.terminal.active_claude_session == Some(usize::MAX) {
                self.terminal.active_claude_session = None;
            }
            if self.terminal.active_shell_session == Some(usize::MAX) {
                self.terminal.active_shell_session = None;
            }
        }

        removed_any
    }

    /// Load resumable Claude Code sessions from Claude's history.
    pub fn load_resume_sessions(&mut self) {
        let filter = if self.overlays.resume_session.all_projects {
            None
        } else {
            Some(self.repo_path.as_path())
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
    pub fn filtered_resume_sessions(&self) -> Vec<(usize, &crate::claude_sessions::ResumableSession)> {
        if self.overlays.resume_session.filter.is_empty() {
            self.overlays.resume_session.sessions.iter().enumerate().collect()
        } else {
            let filter_lower = self.overlays.resume_session.filter.to_lowercase();
            self.overlays.resume_session.sessions
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
    pub fn resume_claude_session(&mut self, session_id: &str, display: &str) -> anyhow::Result<usize> {
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
            &self.repo_path,
        )?;
        self.terminal.pty_manager.activate_session(idx);
        self.terminal.active_claude_session = Some(idx);
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
        let grabbed_session_for_main = self.worktree_mgr.grabbed_branch.as_ref()
            .and_then(|g| g.claude_session_id.clone());

        let selected_wt_path = self.selected_worktree_path();
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let repo_path = self.repo_path.clone();
        let mut resumed_count = 0;

        for wt in &self.worktrees.clone() {
            let canonical = std::fs::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());

            // For main worktree with a grabbed session, prefer the grabbed session ID.
            if wt.is_main {
                if let Some(ref grabbed_id) = grabbed_session_for_main {
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
                    ) {
                        Ok(idx) => {
                            resumed_count += 1;
                            if wt.path == selected_wt_path {
                                self.terminal.pty_manager.activate_session(idx);
                                self.terminal.active_claude_session = Some(idx);
                            }
                        }
                        Err(e) => {
                            log::warn!("auto-resume: failed to resume grabbed session for main: {e}");
                        }
                    }
                    continue;
                }
            }

            let session = match sessions.get(&canonical) {
                Some(s) => s,
                None => continue,
            };

            let label: String = session.display.chars().take(40).collect();
            let label = if label.is_empty() {
                format!("Resume:{}", &session.session_id[..8.min(session.session_id.len())])
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
            ) {
                Ok(idx) => {
                    resumed_count += 1;
                    // Only activate + set active_claude_session for the currently selected worktree.
                    if wt.path == selected_wt_path {
                        self.terminal.pty_manager.activate_session(idx);
                        self.terminal.active_claude_session = Some(idx);
                    }
                }
                Err(e) => {
                    log::warn!("auto-resume: failed to spawn session for {}: {e}", wt.branch);
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

    /// Return `(index_in_pty_manager, &PtySession)` pairs for Claude Code sessions
    /// belonging to the currently selected worktree.
    pub fn current_worktree_claude_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal.pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == pty_manager::SessionKind::ClaudeCode)
            .collect()
    }

    /// Return `(index_in_pty_manager, &PtySession)` pairs for Shell sessions
    /// belonging to the currently selected worktree.
    pub fn current_worktree_shell_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal.pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell)
            .collect()
    }

    /// Sync PTY session sizes with cached layout dimensions.
    /// Only resizes when dimensions actually changed.
    pub fn sync_pty_sizes(
        &mut self,
        last_claude_size: &mut (u16, u16),
        last_shell_size: &mut (u16, u16),
    ) {
        let cols = &self.layout_cache.columns;
        let is_terminal_expanded = matches!(
            self.expanded_panel,
            Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
        );
        let border_cols: u16 = if is_terminal_expanded { 0 } else { 2 };
        let border_rows: u16 = if is_terminal_expanded { 1 } else { 2 };
        let right_w = cols[3].width;
        if right_w > border_cols {
            let right_cols = right_w.saturating_sub(border_cols);
            let claude_pty_rows = self.layout_cache.terminal_split[0].height.saturating_sub(border_rows);
            let shell_pty_rows = self.layout_cache.terminal_split[1].height.saturating_sub(border_rows);

            if (claude_pty_rows, right_cols) != *last_claude_size && claude_pty_rows > 0 && right_cols > 0 {
                *last_claude_size = (claude_pty_rows, right_cols);
                self.update_claude_terminal_size(claude_pty_rows, right_cols);
            }
            if (shell_pty_rows, right_cols) != *last_shell_size && shell_pty_rows > 0 && right_cols > 0 {
                *last_shell_size = (shell_pty_rows, right_cols);
                self.update_shell_terminal_size(shell_pty_rows, right_cols);
            }
        }
    }

    /// Update the terminal content area size for Claude PTY sessions and resize them.
    pub fn update_claude_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_claude = (rows, cols);
        let wt_path = self.selected_worktree_path();
        let count = self.terminal.pty_manager.session_count();
        for idx in 0..count {
            let s = &self.terminal.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == pty_manager::SessionKind::ClaudeCode {
                self.terminal.pty_manager.resize_session(idx, rows, cols);
            }
        }
    }

    /// Update the terminal content area size for Shell PTY sessions and resize them.
    pub fn update_shell_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_shell = (rows, cols);
        let wt_path = self.selected_worktree_path();
        let count = self.terminal.pty_manager.session_count();
        for idx in 0..count {
            let s = &self.terminal.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell {
                self.terminal.pty_manager.resize_session(idx, rows, cols);
            }
        }
    }

    // ── Lightweight change-detection polling ─────────────────────────────

    /// Check whether the diff and viewer panels need refreshing by comparing
    /// the current worktree's HEAD oid and status counts against the last
    /// known values.  Only triggers the expensive `refresh_diff()` and
    /// `refresh_viewer()` when an actual change is detected.
    ///
    /// Called after `refresh_worktrees()` in the polling loop, which already
    /// fetches HEAD oids and status counts as a side effect.
    pub fn check_diff_viewer_staleness(&mut self) {
        let wt = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt,
            None => return,
        };

        let current_head = self.worktree_heads.get(&wt.branch).cloned();
        let current_status = (wt.added, wt.modified, wt.deleted);

        let head_changed = self.last_poll_head_oid.as_ref() != current_head.as_ref();
        let status_changed = self.last_poll_status != Some(current_status);

        if head_changed || status_changed {
            log::debug!(
                "Change detected for worktree '{}': head_changed={}, status_changed={}",
                wt.branch, head_changed, status_changed,
            );
            self.refresh_diff();
            self.refresh_viewer();
        }

        self.last_poll_head_oid = current_head;
        self.last_poll_status = Some(current_status);
    }

    // ── Claude Code input-waiting detection ────────────────────────────

    /// Handle a single CC state notification received via the Unix socket.
    pub fn handle_cc_notify(&mut self, event: crate::cc_notify::CcNotifyEvent) {
        // Normalize the cwd and match against known worktrees.
        let event_normalized: PathBuf = event.cwd.components().collect();
        let wt_path = self
            .worktrees
            .iter()
            .find(|wt| {
                let wt_normalized: PathBuf = wt.path.components().collect();
                wt_normalized == event_normalized
            })
            .map(|wt| wt.path.clone());

        let wt_path = match wt_path {
            Some(p) => p,
            None => return, // Unknown worktree — ignore.
        };

        // Verify a CC session exists for this worktree.
        let has_session = self.terminal.pty_manager.sessions().iter().any(|s| {
            s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == wt_path
        });
        if !has_session {
            return;
        }

        match event.kind {
            crate::cc_notify::CcNotifyKind::Waiting => {
                self.terminal.cc_active_worktrees.remove(&wt_path);

                // Check ack suppression.
                if let Some(&ack_time) = self.terminal.cc_waiting_ack_time.get(&wt_path) {
                    if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                        s.kind == pty_manager::SessionKind::ClaudeCode
                            && s.working_dir == wt_path
                    }) {
                        let current = *session
                            .last_output_time
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if current == ack_time {
                            return; // Suppressed — no new output since ack.
                        }
                        self.terminal.cc_waiting_ack_time.remove(&wt_path);
                    }
                }

                // Focus suppression: if user is focused on this terminal, auto-ack.
                let is_focused = matches!(self.focus, Focus::TerminalClaude)
                    && self.selected_worktree_path() == wt_path;
                if is_focused {
                    return;
                }

                let is_new = self.terminal.cc_waiting_worktrees.insert(wt_path.clone());
                if is_new {
                    let display_name = self
                        .worktrees
                        .iter()
                        .find(|w| w.path == wt_path)
                        .map(|w| w.branch.clone())
                        .unwrap_or_else(|| "?".to_string());
                    self.set_status(
                        format!("CC waiting for input: {display_name}"),
                        StatusLevel::Info,
                    );
                }
            }
            crate::cc_notify::CcNotifyKind::Active => {
                self.terminal.cc_waiting_worktrees.remove(&wt_path);
                self.terminal.cc_active_worktrees.insert(wt_path);
            }
        }
    }

    /// Scan hook signal files and update `cc_waiting_worktrees` and
    /// `cc_active_worktrees`.
    ///
    /// Reads signal files from `.conductor/cc-waiting/` and
    /// `.conductor/cc-active/` directories written by plugin hooks.
    ///
    /// If a worktree newly enters the waiting state and the user is not
    /// currently focused on that worktree's terminal, a status message is
    /// shown as a notification.
    pub fn check_cc_waiting_state(&mut self) -> bool {
        let old_waiting = self.terminal.cc_waiting_worktrees.clone();
        let old_active = self.terminal.cc_active_worktrees.clone();

        // Resolve the main repo root so we look in the right place even
        // when Conductor was launched from a linked worktree.
        let conductor_dir = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone())
            .join(".conductor");

        // Helper: scan a signal directory and collect matching worktree paths.
        let scan_signal_dir = |dir_name: &str, worktrees: &[crate::git_engine::WorktreeInfo]| -> HashSet<PathBuf> {
            let mut result = HashSet::new();
            let signal_dir = conductor_dir.join(dir_name);
            if let Ok(entries) = std::fs::read_dir(&signal_dir) {
                for entry in entries.flatten() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    let signal_path: PathBuf = PathBuf::from(filename.replace("__", "/"));
                    let signal_normalized: PathBuf = signal_path.components().collect();
                    for wt in worktrees {
                        let wt_normalized: PathBuf = wt.path.components().collect();
                        if wt_normalized == signal_normalized {
                            result.insert(wt.path.clone());
                        }
                    }
                }
            }
            result
        };

        let mut new_waiting = scan_signal_dir("cc-waiting", &self.worktrees);
        let mut new_active = scan_signal_dir("cc-active", &self.worktrees);

        // Ignore states for worktrees that have no CC session open.
        // Signal files may persist after a session has exited; without this
        // filter the UI would animate for a non-existent panel.
        let has_cc_session = |wt_path: &PathBuf| -> bool {
            self.terminal.pty_manager.sessions().iter().any(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == *wt_path
            })
        };
        new_waiting.retain(&has_cc_session);
        new_active.retain(has_cc_session);

        // Detect worktrees that newly entered waiting state.
        let current_wt_path = self.selected_worktree_path();
        let is_terminal_focused = matches!(self.focus, Focus::TerminalClaude);

        // When the user is focused on a CC terminal, treat the waiting state
        // as acknowledged — remove it so the notification bar and worktree
        // animation are fully cleared (not just pulse-suppressed).
        if is_terminal_focused && new_waiting.remove(&current_wt_path) {
            // Record ack so the notification is not re-triggered by the
            // PTY pattern-match source until new output arrives.
            if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode
                    && s.working_dir == current_wt_path
            }) {
                let t = *session
                    .last_output_time
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                self.terminal.cc_waiting_ack_time.insert(current_wt_path.clone(), t);
            }
        }

        // Suppress re-triggering for worktrees the user already acknowledged
        // if the PTY has not produced any new output since that acknowledgment.
        let mut ack_expired: Vec<PathBuf> = Vec::new();
        new_waiting.retain(|wt_path| {
            if let Some(&ack_time) = self.terminal.cc_waiting_ack_time.get(wt_path) {
                if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                    s.kind == pty_manager::SessionKind::ClaudeCode
                        && s.working_dir == *wt_path
                }) {
                    let current = *session
                        .last_output_time
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if current == ack_time {
                        return false; // no new output — suppress
                    }
                }
                // New output arrived or session gone — ack is stale.
                ack_expired.push(wt_path.clone());
            }
            true
        });
        for p in ack_expired {
            self.terminal.cc_waiting_ack_time.remove(&p);
        }

        for wt_path in &new_waiting {
            if !self.terminal.cc_waiting_worktrees.contains(wt_path) {
                // Resolve display name from worktree list.
                let display_name = self.worktrees.iter()
                    .find(|w| &w.path == wt_path)
                    .map(|w| w.branch.clone())
                    .unwrap_or_else(|| "?".to_string());
                // Newly waiting — notify if user is not focused on that terminal.
                let skip_notify = is_terminal_focused && *wt_path == current_wt_path;
                if !skip_notify {
                    self.set_status(format!("CC waiting for input: {display_name}"), StatusLevel::Info);
                }
            }
        }

        self.terminal.cc_waiting_worktrees = new_waiting;
        self.terminal.cc_active_worktrees = new_active;

        self.terminal.cc_waiting_worktrees != old_waiting
            || self.terminal.cc_active_worktrees != old_active
    }

    /// Flush deferred prompts for CC sessions that are now ready for input.
    ///
    /// Checks two conditions (either is sufficient):
    /// 1. `is_waiting_for_input` — the session is idle with a "> " prompt
    ///    (reliable for normal operation).
    /// 2. `session_has_visible_output` — the session has rendered anything
    ///    (faster for freshly spawned sessions that haven't reached idle yet).
    pub fn flush_deferred_prompts(&mut self) {
        let ready: Vec<usize> = self
            .terminal
            .deferred_prompts
            .keys()
            .copied()
            .filter(|&idx| {
                self.terminal.pty_manager.is_waiting_for_input(idx)
                    || self.terminal.pty_manager.session_has_visible_output(idx)
            })
            .collect();
        for idx in ready {
            if let Some(prompt) = self.terminal.deferred_prompts.remove(&idx) {
                let _ = self.terminal.pty_manager.write_chunked_to_session(idx, &prompt);
            }
        }
    }

    /// Remove the hook signal file for a given session and clear its
    /// waiting state. Called when user sends input to a CC terminal.
    pub fn clear_cc_waiting_signal(&mut self, session_idx: usize) {
        let session = match self.terminal.pty_manager.sessions().get(session_idx) {
            Some(s) => s,
            None => return,
        };
        if session.kind != pty_manager::SessionKind::ClaudeCode {
            return;
        }
        // Record the PTY output timestamp so that the periodic scan does not
        // re-trigger the notification until new output actually arrives.
        let last_output = *session
            .last_output_time
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let working_dir = session.working_dir.clone();
        self.terminal.cc_waiting_ack_time.insert(working_dir.clone(), last_output);

        let conductor_dir = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone())
            .join(".conductor");
        // Normalize the path (strip trailing slash) to match the shell's $PWD encoding.
        let normalized: PathBuf = session.working_dir.components().collect();
        let sanitized = normalized.display().to_string().replace('/', "__");
        let _ = std::fs::remove_file(conductor_dir.join("cc-waiting").join(&sanitized));
        let _ = std::fs::remove_file(conductor_dir.join("cc-active").join(&sanitized));
        self.terminal.cc_waiting_worktrees.remove(&working_dir);
        self.terminal.cc_active_worktrees.remove(&working_dir);
    }

    // ── Permission handling ─────────────────────────────────────────
    // NOTE: Permission handling functions (start_permission_server,
    // process_permission_judgments, respond_permission_request, etc.)
    // have been moved to the plugin's hooks.
}

// NOTE: Permission handling functions (judge_permission, ask_user_permission,
// apply_suggestion_to_settings, etc.) have been moved to the plugin's Python
// script at plugins/conductor/hooks/scripts/permission-handler.py.
