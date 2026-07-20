//! Terminal / PTY session lifecycle for [`App`].
//!
//! Spawning, switching, closing, and reaping Claude Code and Shell PTY
//! sessions. Resume-session handling lives in [`super::terminal_resume`],
//! PTY size syncing in [`super::terminal_resize`], and Claude Code
//! waiting/active state detection in [`super::terminal_cc_state`].

use super::*;

const SESSION_ICONS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9"];

impl App {
    /// Switch the Claude panel to display the session at `idx`.
    ///
    /// Closes any active reflow transcript first: the reflow view is bound to
    /// whichever session was showing when it was opened, so switching sessions
    /// makes that transcript stale and it must be torn down here. This mirrors
    /// the scroll/cache reset inside [`TerminalState::switch_claude_session`]
    /// (same "the panel now shows a different session" invariant), but reflow
    /// state lives on `App`, so the close has to happen one level up. Routing
    /// every Claude session switch through this wrapper keeps the panel from
    /// rendering the previous session's transcript after a tab/strip switch.
    pub fn switch_claude_session(&mut self, idx: usize) {
        if self.reflow.active {
            self.close_reflow();
        }
        self.terminal.switch_claude_session(idx);
    }

    /// Cycle to the next (`forward`) or previous session tab in the focused
    /// terminal panel — the keyboard equivalent of clicking a tab. No-op unless a
    /// terminal panel is focused and it has more than one session. Wraps around.
    pub fn cycle_terminal_session(&mut self, forward: bool) {
        let (sessions, active): (Vec<usize>, Option<usize>) = match self.focus {
            Focus::TerminalClaude => (
                self.current_worktree_claude_sessions()
                    .iter()
                    .map(|(i, _)| *i)
                    .collect(),
                self.terminal.active_claude_session,
            ),
            Focus::TerminalShell => (
                self.current_worktree_shell_sessions()
                    .iter()
                    .map(|(i, _)| *i)
                    .collect(),
                self.terminal.active_shell_session,
            ),
            _ => return,
        };
        if sessions.len() <= 1 {
            return;
        }
        let pos = active
            .and_then(|a| sessions.iter().position(|&i| i == a))
            .unwrap_or(0);
        let next = if forward {
            (pos + 1) % sessions.len()
        } else {
            (pos + sessions.len() - 1) % sessions.len()
        };
        let target = sessions[next];
        match self.focus {
            Focus::TerminalClaude => self.switch_claude_session(target),
            Focus::TerminalShell => self.terminal.switch_shell_session(target),
            _ => {}
        }
    }

    /// Spawn a new Claude Code PTY session for the currently selected worktree.
    pub fn spawn_claude_code(&mut self) -> anyhow::Result<usize> {
        self.spawn_claude_code_with_name(None)
    }

    /// Spawn a new Claude Code PTY session with an optional `--name` flag.
    pub fn spawn_claude_code_with_name(
        &mut self,
        session_name: Option<&str>,
    ) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let used_ids: Vec<&str> = self
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| {
                s.working_dir == working_dir && s.kind == pty_manager::SessionKind::ClaudeCode
            })
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
            session_name,
        )?;
        self.switch_claude_session(idx);
        self.rebuild_worktree_list_rows();
        Ok(idx)
    }

    /// Spawn a new interactive shell PTY session for the currently selected worktree.
    pub fn spawn_shell(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let sh_count = self
            .terminal
            .pty_manager
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
            None,
        )?;
        self.terminal.switch_shell_session(idx);
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
        for a in [
            &mut self.terminal.active_claude_session,
            &mut self.terminal.active_shell_session,
        ]
        .into_iter()
        .flatten()
        {
            if *a == global_idx {
                *a = usize::MAX; // mark for clear
            } else if *a > global_idx {
                *a -= 1;
            }
        }

        // Keep the embedded editor's session index valid when a lower-indexed
        // session is removed out from under it. The editor itself is never
        // closed through this path (it's torn down by `exit_editor`), so it can
        // only be shifted, never invalidated.
        if let Some(editor) = self.editor.as_mut()
            && editor.session_idx > global_idx
        {
            editor.session_idx -= 1;
        }

        // Clear invalidated indices and fall back to next available session.
        // The closed session was the displayed one, so the fallback target's
        // content differs — switch through the helper to reset scroll and the
        // render cache (otherwise the panel would show the closed session's
        // stale output). When no session remains, clear the cache directly.
        if self.terminal.active_claude_session == Some(usize::MAX) {
            match self
                .current_worktree_claude_sessions()
                .first()
                .map(|(idx, _)| *idx)
            {
                Some(idx) => self.switch_claude_session(idx),
                None => {
                    self.terminal.active_claude_session = None;
                    self.terminal.scroll_claude = 0;
                    self.terminal.cache_claude = Default::default();
                }
            }
        }
        if self.terminal.active_shell_session == Some(usize::MAX) {
            match self
                .current_worktree_shell_sessions()
                .first()
                .map(|(idx, _)| *idx)
            {
                Some(idx) => self.terminal.switch_shell_session(idx),
                None => {
                    self.terminal.active_shell_session = None;
                    self.terminal.scroll_shell = 0;
                    self.terminal.cache_shell = Default::default();
                }
            }
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
            // The editor's own session is owned by `poll_editor_exit` (which
            // restores the layout and reloads the file); never reap it here.
            if self.editor.as_ref().is_some_and(|e| e.session_idx == idx) {
                continue;
            }
            if !self.terminal.pty_manager.is_session_alive(idx) {
                log::info!("removing dead PTY session at index {idx}");
                self.terminal.pty_manager.remove_session(idx);
                removed_any = true;

                // Shift the editor's session index when a lower-indexed session
                // is reaped beneath it.
                if let Some(editor) = self.editor.as_mut()
                    && editor.session_idx > idx
                {
                    editor.session_idx -= 1;
                }

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
                for a in [
                    &mut self.terminal.active_claude_session,
                    &mut self.terminal.active_shell_session,
                ]
                .into_iter()
                .flatten()
                {
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

    /// Return `(index_in_pty_manager, &PtySession)` pairs for Claude Code sessions
    /// belonging to the currently selected worktree.
    pub fn current_worktree_claude_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal
            .pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.working_dir == wt_path && s.kind == pty_manager::SessionKind::ClaudeCode
            })
            .collect()
    }

    /// Return `(index_in_pty_manager, &PtySession)` pairs for Shell sessions
    /// belonging to the currently selected worktree.
    pub fn current_worktree_shell_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal
            .pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell)
            .collect()
    }
}
