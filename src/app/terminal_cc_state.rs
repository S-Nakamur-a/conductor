//! Claude Code waiting/active state tracking for [`App`].
//!
//! Consumes CC state notifications (both the direct Unix-socket event and the
//! filesystem hook-signal fallback) to maintain `cc_waiting_worktrees` /
//! `cc_active_worktrees`, and flushes prompts that were deferred until a
//! session became ready for input.

use std::collections::HashSet;
use std::path::PathBuf;

use super::*;

impl App {
    // ── Claude Code input-waiting detection ────────────────────────────

    /// Handle a single CC notification received via the Unix socket.
    pub fn handle_cc_notify(&mut self, event: crate::cc_notify::CcNotifyEvent) {
        let (kind, cwd) = match event {
            crate::cc_notify::CcNotifyEvent::State { kind, cwd } => (kind, cwd),
            // `/clear` や `/resume` でこのパネルのログが別 id に移った。
            // スクロールバックが読むファイルを差し替えるだけで、waiting/active
            // の状態には関係しない。
            crate::cc_notify::CcNotifyEvent::SessionRotated {
                panel_id,
                session_id,
            } => {
                if self
                    .terminal
                    .pty_manager
                    .set_claude_session_id(&panel_id, session_id)
                {
                    // 開きっぱなしのトランスクリプトは古いログを指したままなので
                    // 畳む。次に開いたときに新しいログから読み直される。
                    if self.reflow.active {
                        self.close_reflow();
                    }
                }
                return;
            }
        };

        // Normalize the cwd and match against known worktrees.
        let event_normalized: PathBuf = cwd.components().collect();
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
        let has_session =
            self.terminal.pty_manager.sessions().iter().any(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == wt_path
            });
        if !has_session {
            return;
        }

        match kind {
            crate::cc_notify::CcNotifyKind::Waiting => {
                self.terminal.cc_active_worktrees.remove(&wt_path);

                // Check ack suppression.
                if let Some(&ack_time) = self.terminal.cc_waiting_ack_time.get(&wt_path)
                    && let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                        s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == wt_path
                    })
                {
                    let current = *session
                        .last_output_time
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if current == ack_time {
                        return; // Suppressed — no new output since ack.
                    }
                    self.terminal.cc_waiting_ack_time.remove(&wt_path);
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
        let conductor_dir = git_engine::GitEngine::open(&self.repo.path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo.path.clone())
            .join(".conductor");

        // Helper: scan a signal directory and collect matching worktree paths.
        let scan_signal_dir =
            |dir_name: &str, worktrees: &[crate::git_engine::WorktreeInfo]| -> HashSet<PathBuf> {
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
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == current_wt_path
            }) {
                let t = *session
                    .last_output_time
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                self.terminal
                    .cc_waiting_ack_time
                    .insert(current_wt_path.clone(), t);
            }
        }

        // Suppress re-triggering for worktrees the user already acknowledged
        // if the PTY has not produced any new output since that acknowledgment.
        let mut ack_expired: Vec<PathBuf> = Vec::new();
        new_waiting.retain(|wt_path| {
            if let Some(&ack_time) = self.terminal.cc_waiting_ack_time.get(wt_path) {
                if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                    s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == *wt_path
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
                let display_name = self
                    .worktrees
                    .iter()
                    .find(|w| &w.path == wt_path)
                    .map(|w| w.branch.clone())
                    .unwrap_or_else(|| "?".to_string());
                // Newly waiting — notify if user is not focused on that terminal.
                let skip_notify = is_terminal_focused && *wt_path == current_wt_path;
                if !skip_notify {
                    self.set_status(
                        format!("CC waiting for input: {display_name}"),
                        StatusLevel::Info,
                    );
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
                let _ = self
                    .terminal
                    .pty_manager
                    .write_chunked_to_session(idx, &prompt);
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
        self.terminal
            .cc_waiting_ack_time
            .insert(working_dir.clone(), last_output);

        let conductor_dir = git_engine::GitEngine::open(&self.repo.path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo.path.clone())
            .join(".conductor");
        // Normalize the path (strip trailing slash) to match the shell's $PWD encoding.
        let normalized: PathBuf = session.working_dir.components().collect();
        let sanitized = normalized.display().to_string().replace('/', "__");
        let _ = std::fs::remove_file(conductor_dir.join("cc-waiting").join(&sanitized));
        let _ = std::fs::remove_file(conductor_dir.join("cc-active").join(&sanitized));
        self.terminal.cc_waiting_worktrees.remove(&working_dir);
        self.terminal.cc_active_worktrees.remove(&working_dir);
    }
}
