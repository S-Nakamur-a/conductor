//! Terminal / PTY / Permission methods for [`App`].
//!
//! This module contains methods for spawning and managing Claude Code and Shell
//! PTY sessions, permission auto-response, and related helpers.

use std::collections::HashSet;
use std::path::PathBuf;
use super::*;

const INSTRUMENTS: &[&str] = &[
    "\u{1f3b9}", // 🎹 Keyboard
    "\u{1f3b8}", // 🎸 Guitar
    "\u{1f3ba}", // 🎺 Trumpet
    "\u{1f3bb}", // 🎻 Violin
    "\u{1f941}", // 🥁 Drum
    "\u{1f3b7}", // 🎷 Saxophone
    "\u{1fa97}", // 🪗 Accordion
    "\u{1fa95}", // 🪕 Banjo
    "\u{1fa88}", // 🪈 Flute
    "\u{1fa98}", // 🪘 Conga
    "\u{1fa87}", // 🪇 Maracas
    "\u{1f4ef}", // 📯 Postal Horn
];

impl App {
    /// Spawn a new Claude Code PTY session for the currently selected worktree.
    pub fn spawn_claude_code(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let used_emojis: Vec<&str> = self
            .terminal.pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::ClaudeCode)
            .filter_map(|s| s.label.strip_prefix("CC:"))
            .collect();
        let emoji = INSTRUMENTS
            .iter()
            .find(|e| !used_emojis.contains(e))
            .unwrap_or(&INSTRUMENTS[used_emojis.len() % INSTRUMENTS.len()]);
        let label = format!("CC:{}", emoji);
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
    pub fn cleanup_dead_sessions(&mut self) {
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

        let selected_wt_path = self.selected_worktree_path();
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let repo_path = self.repo_path.clone();
        let mut resumed_count = 0;

        for wt in &self.worktrees.clone() {
            let canonical = std::fs::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());
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

    /// Scan all Claude Code sessions and update `cc_waiting_worktrees`.
    ///
    /// Uses two sources:
    /// 1. Hook signal files in `.conductor/cc-waiting/` (high reliability).
    /// 2. PTY pattern matching fallback (for `[Y/n]` prompts).
    ///
    /// If a worktree newly enters the waiting state and the user is not
    /// currently focused on that worktree's terminal, a status message is
    /// shown as a notification.
    pub fn check_cc_waiting_state(&mut self) {
        let mut new_waiting: HashSet<PathBuf> = HashSet::new();

        // Source 1: Hook signal files (high reliability).
        // Signal files are written by the plugin hook to the main repo's
        // `.conductor/cc-waiting/` directory.  Resolve via git so we look
        // in the right place even when Conductor was launched from a linked
        // worktree.
        let signal_dir = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone())
            .join(".conductor")
            .join("cc-waiting");
        if let Ok(entries) = std::fs::read_dir(&signal_dir) {
            for entry in entries.flatten() {
                let filename = entry.file_name().to_string_lossy().to_string();
                let signal_path: PathBuf = PathBuf::from(filename.replace("__", "/"));
                // Normalize both sides (strip trailing slashes) to ensure
                // comparison succeeds regardless of how paths were serialized.
                let signal_normalized: PathBuf = signal_path.components().collect();
                for wt in &self.worktrees {
                    let wt_normalized: PathBuf = wt.path.components().collect();
                    if wt_normalized == signal_normalized {
                        new_waiting.insert(wt.path.clone());
                    }
                }
            }
        }

        // Source 2: PTY pattern match fallback (for [Y/n] prompts).
        let session_count = self.terminal.pty_manager.session_count();
        for idx in 0..session_count {
            let session = &self.terminal.pty_manager.sessions()[idx];
            if session.kind != pty_manager::SessionKind::ClaudeCode {
                continue;
            }
            if self.terminal.pty_manager.is_waiting_for_input(idx) {
                new_waiting.insert(session.working_dir.clone());
            }
        }

        // Ignore waiting state for worktrees that have no CC session open.
        // Signal files may persist after a session has exited; without this
        // filter the notification bar would animate for a non-existent panel.
        new_waiting.retain(|wt_path| {
            self.terminal.pty_manager.sessions().iter().any(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == *wt_path
            })
        });

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

        let signal_dir = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone())
            .join(".conductor")
            .join("cc-waiting");
        // Normalize the path (strip trailing slash) to match the shell's $PWD encoding.
        let normalized: PathBuf = session.working_dir.components().collect();
        let sanitized = normalized.display().to_string().replace('/', "__");
        let _ = std::fs::remove_file(signal_dir.join(&sanitized));
        self.terminal.cc_waiting_worktrees.remove(&working_dir);
    }

    // ── Permission auto-response ────────────────────────────────────

    /// Remove the Unix socket file on shutdown.
    pub fn cleanup_permission_server(&self) {
        let repo_root = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone());
        let sock_path = repo_root.join(".conductor").join("server.sock");
        let _ = std::fs::remove_file(&sock_path);
    }

    /// Start the Unix domain socket server for receiving permission requests
    /// from hooks.  The socket is created at `.conductor/server.sock`.
    pub(super) fn start_permission_server(&self) {
        use std::os::unix::net::UnixListener;

        let repo_root = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone());
        let conductor_dir = repo_root.join(".conductor");
        let _ = std::fs::create_dir_all(&conductor_dir);
        let sock_path = conductor_dir.join("server.sock");

        // Remove stale socket if it exists.
        if sock_path.exists() {
            let _ = std::fs::remove_file(&sock_path);
        }

        let listener = match UnixListener::bind(&sock_path) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("failed to bind permission socket {:?}: {}", sock_path, e);
                return;
            }
        };
        // Non-blocking so the accept loop can check a shutdown flag.
        listener.set_nonblocking(true).ok();

        log::info!("permission server listening on {:?}", sock_path);

        let tx = self.terminal.permission_incoming_tx.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};

            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            let reader = BufReader::new(&stream);
                            let mut data = String::new();
                            for line in reader.lines() {
                                match line {
                                    Ok(l) => {
                                        data.push_str(&l);
                                        data.push('\n');
                                    }
                                    Err(_) => break,
                                }
                            }

                            let parsed: serde_json::Value = match serde_json::from_str(data.trim()) {
                                Ok(v) => v,
                                Err(_) => return,
                            };

                            let incoming = crate::terminal_state::IncomingPermission {
                                session_id: parsed.get("session_id")
                                    .and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                tool_name: parsed.get("tool")
                                    .and_then(|t| t.get("tool_name"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?").to_string(),
                                tool_input: parsed.get("tool")
                                    .and_then(|t| t.get("tool_input"))
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Object(Default::default())),
                                user_message: parsed.get("user_message")
                                    .and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                hook_message: parsed.get("message")
                                    .and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                cwd: parsed.get("cwd")
                                    .and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                timestamp: parsed.get("timestamp")
                                    .and_then(|v| v.as_i64()).unwrap_or(0),
                            };

                            let _ = tx.send(incoming);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        });
    }

    /// Process incoming permission requests received via the Unix socket.
    /// If `auto_permission` is enabled, spawn `claude -p` to judge.
    pub fn process_permission_judgments(&mut self) {
        // Drain pending requests back into the processing queue first.
        let mut all_incoming: Vec<crate::terminal_state::IncomingPermission> =
            std::mem::take(&mut self.terminal.permission_pending);
        while let Ok(incoming) = self.terminal.permission_incoming_rx.try_recv() {
            all_incoming.push(incoming);
        }

        for incoming in all_incoming {
            let session_id = incoming.session_id.clone();
            let cwd = incoming.cwd.clone();
            let tool_name = incoming.tool_name.clone();
            let tool_input = incoming.tool_input.clone();
            let user_message = incoming.user_message.clone();
            let hook_message = incoming.hook_message.clone();

            // Deduplicate using session_id + timestamp.
            let dedup_key = format!("{}:{}", session_id, incoming.timestamp);
            if self.terminal.permission_processed_sessions.contains(&dedup_key) {
                continue;
            }

            let cwd_path = PathBuf::from(&cwd);
            let session_idx = self.terminal.pty_manager.sessions().iter().position(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == cwd_path
            });

            let Some(idx) = session_idx else {
                continue;
            };

            // Already judging this session? Defer to next poll cycle.
            if self.terminal.permission_judging.iter().any(|j| j.session_idx == idx) {
                self.terminal.permission_pending.push(incoming);
                continue;
            }

            self.terminal.permission_processed_sessions.insert(dedup_key);

            // If auto_permission is disabled, skip claude -p and treat as ask_user directly.
            if !self.config.notification.auto_permission {
                let tx = self.terminal.permission_judge_tx.clone();
                let _ = tx.send(crate::terminal_state::PermissionJudgeResult {
                    session_idx: idx,
                    action: "ask_user".to_string(),
                    reason: "auto_permission is disabled".to_string(),
                    tool_name: tool_name.clone(),
                    user_message: user_message.clone(),
                    cwd: cwd_path.clone(),
                });
                continue;
            }

            // Read PERMISSION.md files.
            let repo_root = git_engine::GitEngine::open(&self.repo_path)
                .and_then(|e| e.main_worktree_path())
                .unwrap_or_else(|_| self.repo_path.clone());
            let project_md = repo_root.join(".claude").join("PERMISSION.md");
            let home = std::env::var("HOME").unwrap_or_default();
            let global_md = PathBuf::from(&home).join(".claude").join("PERMISSION.md");

            let project_rules = std::fs::read_to_string(&project_md).unwrap_or_default();
            let global_rules = std::fs::read_to_string(&global_md).unwrap_or_default();

            // Read allow/deny from settings.json and settings.local.json.
            let settings_permissions =
                read_settings_permissions(&repo_root, &home);

            if project_rules.is_empty()
                && global_rules.is_empty()
                && settings_permissions.is_empty()
            {
                continue; // No rules — skip judgment.
            }

            let mut rules_section = if !global_rules.is_empty() && !project_rules.is_empty() {
                format!(
                    "以下の2つのルールがあります。矛盾する場合はプロジェクトルールを優先してください。\n\n\
                     ### グローバルルール (~/.claude/PERMISSION.md)\n{global_rules}\n\n\
                     ### プロジェクトルール (.claude/PERMISSION.md) ※こちらが優先\n{project_rules}"
                )
            } else if !project_rules.is_empty() {
                project_rules
            } else {
                global_rules
            };

            if !settings_permissions.is_empty() {
                rules_section.push_str(&format!(
                    "\n\n### settings.json / settings.local.json の許可・拒否パターン\n\
                     ユーザーが明示的に設定した allow/deny パターンです。これらに合致する場合は優先してください。\n\
                     {settings_permissions}"
                ));
            }

            let tool_context = serde_json::json!({
                "tool": { "tool_name": &tool_name, "tool_input": &tool_input },
                "user_message": &user_message,
            }).to_string();

            let prompt = format!(
                "ツール実行の許可判定を行ってください。\n\n\
                 ## ルール\n{rules_section}\n\n\
                 ## 判定対象\n通知: {hook_message}\nツール詳細: {tool_context}\n\
                 作業ディレクトリ: {cwd}\n\n\
                 action は approve, deny, ask_user のいずれか。reason は日本語で1文。"
            );

            let json_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["approve", "deny", "ask_user"]},
                    "reason": {"type": "string"}
                },
                "required": ["action", "reason"]
            }).to_string();

            // Track the judging state.
            let pid_arc: std::sync::Arc<std::sync::Mutex<Option<u32>>> =
                std::sync::Arc::new(std::sync::Mutex::new(None));
            self.terminal.permission_judging.push(
                crate::terminal_state::PermissionJudging {
                    session_idx: idx,
                    tool_name: tool_name.clone(),
                    cwd: cwd_path.clone(),
                    started_at: std::time::Instant::now(),
                    pid: std::sync::Arc::clone(&pid_arc),
                },
            );
            self.set_status(
                format!("Judging permission: {tool_name}..."),
                StatusLevel::Info,
            );

            // Spawn claude -p in background.
            let tx = self.terminal.permission_judge_tx.clone();
            let pid_slot = std::sync::Arc::clone(&pid_arc);
            let tool_name_for_thread = tool_name.clone();
            let user_msg_for_thread = user_message.clone();
            let cwd_for_thread = cwd_path.clone();
            std::thread::spawn(move || {
                let mut child = match std::process::Command::new("claude")
                    .args([
                        "-p",
                        "--model", "haiku",
                        "--output-format", "json",
                        "--json-schema", &json_schema,
                        "--tools", "",
                        "--max-budget-usd", "0.10",
                        "--no-session-persistence",
                        "--disable-slash-commands",
                        "--no-chrome",
                        "--system-prompt", "You are a permission judgment assistant. Output JSON only.",
                    ])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => return,
                };

                *pid_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());

                // Write prompt to stdin then close it.
                use std::io::Write;
                {
                    let stdin = child.stdin.as_mut();
                    if let Some(s) = stdin {
                        let _ = s.write_all(prompt.as_bytes());
                    }
                }
                // Close stdin by taking it.
                drop(child.stdin.take());

                let output = child.wait_with_output();
                let (action, reason) = match &output {
                    Ok(o) if o.status.success() => {
                        let raw = String::from_utf8_lossy(&o.stdout);
                        if let Ok(outer) = serde_json::from_str::<serde_json::Value>(&raw) {
                            let so = outer.get("structured_output")
                                .and_then(|v| v.as_object());
                            if let Some(obj) = so {
                                let a = obj.get("action")
                                    .and_then(|v| v.as_str()).unwrap_or("ask_user").to_string();
                                let r = obj.get("reason")
                                    .and_then(|v| v.as_str()).unwrap_or("").to_string();
                                (a, r)
                            } else {
                                ("ask_user".to_string(), "パース失敗".to_string())
                            }
                        } else {
                            ("ask_user".to_string(), "パース失敗".to_string())
                        }
                    }
                    _ => return, // Process killed or failed — discard.
                };

                let _ = tx.send(crate::terminal_state::PermissionJudgeResult {
                    session_idx: idx,
                    action,
                    reason,
                    tool_name: tool_name_for_thread,
                    user_message: user_msg_for_thread,
                    cwd: cwd_for_thread,
                });
            });
        }
    }

    /// Process results from `claude -p` judgment threads.
    pub fn process_permission_judge_results(&mut self) {
        while let Ok(result) = self.terminal.permission_judge_rx.try_recv() {
            // Remove from judging list.
            self.terminal.permission_judging.retain(|j| j.session_idx != result.session_idx);

            // Check if the permission prompt is still visible (user hasn't
            // already responded manually).
            let prompt_visible = self.terminal.pty_manager
                .permission_prompt_keystrokes(result.session_idx, true)
                .is_some();
            if !prompt_visible {
                continue; // User already responded — discard.
            }

            match result.action.as_str() {
                "approve" | "deny" => {
                    let approve = result.action == "approve";
                    let keystrokes = self.terminal.pty_manager
                        .permission_prompt_keystrokes(result.session_idx, approve);
                    if let Some(input_bytes) = keystrokes {
                        self.set_status(
                            format!("Auto-{}: {} ({})",
                                if approve { "approved" } else { "denied" },
                                result.tool_name, result.reason),
                            StatusLevel::Info,
                        );
                        let _ = self.terminal.pty_manager.write_to_session(result.session_idx, &input_bytes);
                        self.clear_cc_waiting_signal(result.session_idx);
                    }
                }
                _ => {
                    // ask_user — add to queue with dialog.
                    let dialog_pid: std::sync::Arc<std::sync::Mutex<Option<u32>>> =
                        std::sync::Arc::new(std::sync::Mutex::new(None));

                    self.terminal.permission_queue.push(
                        crate::terminal_state::PermissionRequest {
                            session_idx: result.session_idx,
                            tool_name: result.tool_name.clone(),
                            reason: result.reason.clone(),
                            user_message: result.user_message,
                            cwd: result.cwd,
                            created_at: std::time::Instant::now(),
                            dialog_pid: Some(std::sync::Arc::clone(&dialog_pid)),
                        },
                    );

                    let notify_msg = format!("{}: {}", result.tool_name, result.reason);
                    self.set_status(
                        format!("Permission needed: {notify_msg}"),
                        StatusLevel::Warning,
                    );

                    // Show macOS dialog — include user_message for context on "why".
                    let tx = self.terminal.permission_dialog_tx.clone();
                    let dialog_session_idx = result.session_idx;
                    let pid_slot = std::sync::Arc::clone(&dialog_pid);
                    // Retrieve user_message from the just-pushed request.
                    let dialog_user_msg = self.terminal.permission_queue.last()
                        .map(|r| r.user_message.clone())
                        .unwrap_or_default();
                    std::thread::spawn(move || {
                        let mut dialog_text = notify_msg.replace('\\', "\\\\").replace('"', "\\\"");
                        if !dialog_user_msg.is_empty() {
                            // Truncate long user messages for the dialog.
                            let truncated: String = dialog_user_msg.chars().take(120).collect();
                            let suffix = if dialog_user_msg.chars().count() > 120 { "…" } else { "" };
                            let escaped = format!("\\n\\nContext: {truncated}{suffix}")
                                .replace('\\', "\\\\").replace('"', "\\\"");
                            dialog_text.push_str(&escaped);
                        }
                        let script = format!(
                            "display dialog \"{}\" with title \"Conductor Permission\" buttons {{\"Deny\", \"Approve\"}} default button \"Approve\"",
                            dialog_text
                        );
                        let child = match std::process::Command::new("osascript")
                            .args(["-e", &script])
                            .stdout(std::process::Stdio::piped())
                            .spawn()
                        {
                            Ok(c) => c,
                            Err(_) => return,
                        };
                        *pid_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());
                        let output = child.wait_with_output();
                        let approved = matches!(&output, Ok(o) if o.status.success()
                            && String::from_utf8_lossy(&o.stdout).contains("Approve"));
                        if output.map(|o| o.status.success()).unwrap_or(false) {
                            let _ = tx.send(crate::terminal_state::PermissionDialogResult {
                                session_idx: dialog_session_idx,
                                approved,
                            });
                        }
                    });
                }
            }
        }

        // Clean up judging entries for sessions that no longer exist.
        let session_count = self.terminal.pty_manager.session_count();
        self.terminal.permission_judging.retain(|j| j.session_idx < session_count);
    }

    /// Process results from OS permission dialogs (background threads).
    pub fn process_permission_dialog_results(&mut self) {
        while let Ok(result) = self.terminal.permission_dialog_rx.try_recv() {
            let idx = result.session_idx;

            // Check if prompt is still visible.
            let prompt_visible = self.terminal.pty_manager
                .permission_prompt_keystrokes(idx, true)
                .is_some();

            if let Some(pos) = self.terminal.permission_queue.iter()
                .position(|r| r.session_idx == idx)
            {
                let req = self.terminal.permission_queue.remove(pos);
                if self.terminal.permission_queue_selected >= self.terminal.permission_queue.len()
                    && self.terminal.permission_queue_selected > 0
                {
                    self.terminal.permission_queue_selected =
                        self.terminal.permission_queue.len().saturating_sub(1);
                }

                if prompt_visible {
                    let keystrokes = self.terminal.pty_manager
                        .permission_prompt_keystrokes(idx, result.approved);
                    if let Some(input_bytes) = keystrokes {
                        let action_str = if result.approved { "Approved" } else { "Denied" };
                        self.set_status(
                            format!("{action_str} (dialog): {}", req.tool_name),
                            StatusLevel::Info,
                        );
                        let _ = self.terminal.pty_manager.write_to_session(idx, &input_bytes);
                        self.clear_cc_waiting_signal(idx);
                    }
                }
            }
        }
    }

    /// Respond to the currently selected permission request.
    /// Called when the user presses y (approve) or n (deny) on the queue.
    pub fn respond_permission_request(&mut self, approve: bool) {
        let idx = self.terminal.permission_queue_selected;
        let request = match self.terminal.permission_queue.get(idx) {
            Some(r) => r,
            None => return,
        };
        let session_idx = request.session_idx;
        let tool_name = request.tool_name.clone();

        // Kill the OS dialog process if it's still running.
        if let Some(ref pid_arc) = request.dialog_pid {
            if let Some(pid) = *pid_arc.lock().unwrap_or_else(|e| e.into_inner()) {
                let _ = std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .output();
            }
        }

        let keystrokes = self.terminal.pty_manager
            .permission_prompt_keystrokes(session_idx, approve);

        if let Some(input_bytes) = keystrokes {
            self.set_status(
                format!("{}: {}",
                    if approve { "Approved" } else { "Denied" },
                    tool_name),
                StatusLevel::Info,
            );
            let _ = self.terminal.pty_manager.write_to_session(session_idx, &input_bytes);
            self.clear_cc_waiting_signal(session_idx);
        }

        self.terminal.permission_queue.remove(idx);
        if self.terminal.permission_queue_selected > 0
            && self.terminal.permission_queue_selected >= self.terminal.permission_queue.len()
        {
            self.terminal.permission_queue_selected =
                self.terminal.permission_queue.len().saturating_sub(1);
        }
    }

    /// Cancel any running `claude -p` judgment for a given session.
    /// Called when the user manually responds to a permission prompt.
    pub fn cancel_permission_judging(&mut self, session_idx: usize) {
        self.terminal.permission_judging.retain(|j| {
            if j.session_idx == session_idx {
                if let Some(pid) = *j.pid.lock().unwrap_or_else(|e| e.into_inner()) {
                    let _ = std::process::Command::new("kill")
                        .arg(pid.to_string())
                        .output();
                }
                false // remove
            } else {
                true // keep
            }
        });
    }
}

/// Read allow/deny permission patterns from Claude Code settings files.
///
/// Reads from up to 4 files (global settings, global local settings,
/// project settings, project local settings) and formats them for the
/// permission judgment prompt.
fn read_settings_permissions(repo_root: &std::path::Path, home: &str) -> String {
    let home_path = std::path::PathBuf::from(home);
    let files: &[(&str, std::path::PathBuf)] = &[
        (
            "~/.claude/settings.json",
            home_path.join(".claude").join("settings.json"),
        ),
        (
            "~/.claude/settings.local.json",
            home_path.join(".claude").join("settings.local.json"),
        ),
        (
            ".claude/settings.json",
            repo_root.join(".claude").join("settings.json"),
        ),
        (
            ".claude/settings.local.json",
            repo_root.join(".claude").join("settings.local.json"),
        ),
    ];

    let mut parts = Vec::new();
    for (label, path) in files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let permissions = &json["permissions"];
        let allow = permissions["allow"].as_array();
        let deny = permissions["deny"].as_array();

        if allow.is_none() && deny.is_none() {
            continue;
        }

        let mut section = format!("**{label}**\n");
        if let Some(allow_list) = allow {
            if !allow_list.is_empty() {
                section.push_str("- allow: ");
                let items: Vec<String> = allow_list
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| format!("`{s}`")))
                    .collect();
                section.push_str(&items.join(", "));
                section.push('\n');
            }
        }
        if let Some(deny_list) = deny {
            if !deny_list.is_empty() {
                section.push_str("- deny: ");
                let items: Vec<String> = deny_list
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| format!("`{s}`")))
                    .collect();
                section.push_str(&items.join(", "));
                section.push('\n');
            }
        }
        parts.push(section);
    }

    parts.join("\n")
}
