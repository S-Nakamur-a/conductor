//! Review template deletion and PTY session history (list/search/save) for
//! [`App`].

use super::*;

impl App {
    // ── Template helpers ─────────────────────────────────────────

    pub fn delete_review_template(&mut self, id: &str) {
        if let Some(store) = &self.review_store {
            match store.delete_template(id) {
                Ok(()) => {
                    self.review_state.status_message = Some("Template deleted.".to_string());
                }
                Err(e) => {
                    self.review_state.status_message = Some(format!("Error: {e}"));
                }
            }
            self.review_state.load_templates(store);
        }
    }

    // ── Session history helpers ─────────────────────────────────

    pub fn load_session_history(&mut self) {
        if let Some(store) = &self.review_store {
            match store.list_session_history(50) {
                Ok(records) => {
                    self.overlays.history.records = records;
                    self.overlays.history.selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to load session history: {e}");
                    self.overlays.history.records.clear();
                }
            }
        }
    }

    pub fn search_session_history(&mut self) {
        if let Some(store) = &self.review_store {
            let query = self.overlays.history.search_query.text().to_string();
            let result = if query.is_empty() {
                store.list_session_history(50)
            } else {
                store.search_session_history(&query)
            };
            match result {
                Ok(records) => {
                    self.overlays.history.records = records;
                    self.overlays.history.selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to search session history: {e}");
                }
            }
        }
    }

    pub fn save_current_session_history(&mut self) {
        // Try the active Claude session first, then Shell.
        let active_idx = self
            .terminal
            .active_claude_session
            .or(self.terminal.active_shell_session);
        let active_idx = match active_idx {
            Some(idx) => idx,
            None => {
                self.set_status(
                    "No active PTY session to save.".to_string(),
                    StatusLevel::Warning,
                );
                return;
            }
        };

        let sessions = self.terminal.pty_manager.sessions();
        let session = match sessions.get(active_idx) {
            Some(s) => s,
            None => {
                self.set_status("Session not found.".to_string(), StatusLevel::Error);
                return;
            }
        };

        let session_id = session.id.clone();
        let worktree = session.worktree.clone();
        let label = session.label.clone();
        let kind = match session.kind {
            pty_manager::SessionKind::ClaudeCode => "claude_code",
            pty_manager::SessionKind::Shell => "shell",
            pty_manager::SessionKind::Editor => "editor",
        };
        let output = self.terminal.pty_manager.get_output(active_idx).join("\n");

        if let Some(store) = &self.review_store {
            match store.save_session_history(&session_id, &worktree, &label, kind, &output) {
                Ok(()) => {
                    self.status_message = Some(StatusMessage::new(
                        "Session history saved.".to_string(),
                        StatusLevel::Success,
                        self.ui_tick,
                    ));
                    if self.overlays.active == ActiveOverlay::History {
                        match store.list_session_history(50) {
                            Ok(records) => {
                                self.overlays.history.records = records;
                                self.overlays.history.selected = 0;
                            }
                            Err(e) => {
                                log::warn!("failed to reload session history: {e}");
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("failed to save session history: {e}");
                    self.status_message = Some(StatusMessage::new(
                        format!("Error saving history: {e}"),
                        StatusLevel::Error,
                        self.ui_tick,
                    ));
                }
            }
        }
    }
}
