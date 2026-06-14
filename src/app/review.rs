//! Review / Comment / History methods for [`App`].
//!
//! This module contains methods for managing review comments, templates,
//! and session history.

use super::*;

impl App {
    /// Reload review comments from the database for the currently selected worktree.
    pub fn refresh_reviews(&mut self) {
        if let Some(store) = &self.review_store {
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // Rebuild per-file cache for the currently viewed file.
            if let Some(file_path) = self.viewer_state.content.current_file.clone() {
                self.review_state.build_file_comment_cache(&file_path);
            }
            // Keep the diff list's SUMMARY pseudo-file in sync with whether this
            // branch has a change summary. Only rebuild when it actually flips,
            // so we don't disturb the display list on every reload.
            let has_summary = self.review_state.change_summary.is_some();
            if self.diff_state.has_summary != has_summary {
                self.diff_state.has_summary = has_summary;
                self.diff_state.rebuild_display_list();
                // If the summary vanished while its pseudo-file was open, leave
                // the now-orphaned summary view so the Viewer and the list agree.
                if !has_summary && self.viewer_state.is_summary() {
                    self.viewer_state.exit_diff_mode();
                }
            }
        }
    }

    /// Open the diff file currently selected in the diff list (the entry at
    /// `diff_list_selected`) into the Viewer. Shared by the file-jump keys; a
    /// no-op if the selected entry isn't a file.
    pub fn open_diff_file_at_selected(&mut self) {
        let idx = self.viewer_state.explorer.diff_list_selected;
        let (file_path, file_diff_clone) = match self.diff_state.resolve_file(idx) {
            Some((f, _)) => (f.path.clone(), f.clone()),
            None => return,
        };
        let Some(wt) = self.worktrees.get(self.selected_worktree) else {
            return;
        };
        let wt_path = wt.path.clone();
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&wt_path, &file_path, tab_width);
        self.viewer_state.reveal_file_in_tree(&file_path, &wt_path);
        self.rehighlight_viewer();
        self.review_state.build_file_comment_cache(&file_path);
        self.expand_threads_for_file(&file_path);
        self.viewer_state.build_unified_diff_view(&file_diff_clone);
        // Land on the first review comment if the file has any (so the reviewer
        // sees it immediately — answers "jump to the file's first comment"),
        // otherwise on the first change.
        let first_comment_line = self
            .review_state
            .comments
            .iter()
            .filter(|c| c.file_path == file_path)
            .map(|c| c.line_start as usize)
            .min();
        let target = first_comment_line
            .and_then(|line| {
                self.viewer_state
                    .diff_view
                    .diff_view_lines
                    .iter()
                    .position(|e| matches!(e, crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(n), .. } if *n == line))
            })
            .or_else(|| {
                self.viewer_state
                    .diff_view
                    .diff_view_lines
                    .iter()
                    .position(|e| {
                        matches!(e, crate::viewer::UnifiedDiffEntry::Line { tag, .. }
                            if *tag != crate::diff_state::DiffLineTag::Equal)
                    })
            });
        if let Some(pos) = target {
            self.viewer_state.diff_view.diff_view_scroll = pos.saturating_sub(3);
        }
    }

    /// Jump to the next (or previous) changed file in the diff list and open it.
    /// Skips non-file rows (section headers, directories, the SUMMARY entry).
    /// The lightweight substitute for GitHub-style cross-file scrolling.
    pub fn jump_to_changed_file(&mut self, forward: bool) {
        use crate::diff_state::DiffListEntry;
        let len = self.diff_state.display_list.len();
        // Clamp the cursor: a stale `diff_list_selected` (e.g. after the list
        // shrank on refresh) must never index past the list in the backward
        // scan below, or `display_list[i]` panics.
        let cur = self.viewer_state.explorer.diff_list_selected.min(len);
        let target = if forward {
            (cur + 1..len)
                .find(|&i| matches!(self.diff_state.display_list[i], DiffListEntry::File { .. }))
        } else {
            (0..cur)
                .rev()
                .find(|&i| matches!(self.diff_state.display_list[i], DiffListEntry::File { .. }))
        };
        if let Some(idx) = target {
            self.viewer_state.explorer.diff_list_selected = idx;
            self.open_diff_file_at_selected();
        }
    }

    /// Default-expand the inline comment threads for a freshly opened file, so
    /// review comments are visible at a glance instead of starting collapsed.
    /// Only the opened file's threads are expanded (not every file's), matching
    /// "the selected file's comments are open by default". The user can still
    /// collapse individual threads afterward.
    pub fn expand_threads_for_file(&mut self, file_path: &str) {
        // Only auto-expand lines with at least one *unresolved* comment.
        // Resolved comments are collapsed by default (their gutter badge still
        // shows, and clicking it opens the thread on demand).
        let lines: Vec<usize> = self
            .review_state
            .comments
            .iter()
            .filter(|c| {
                c.file_path == file_path
                    && c.status != crate::review_store::CommentStatus::Resolved
            })
            .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
            .collect();
        for line in lines {
            self.viewer_state
                .explorer
                .expanded_inline_threads
                .insert(line);
        }
    }

    /// Add a new review comment for the current worktree and refresh the
    /// comment list.
    pub fn add_review_comment(
        &mut self,
        file_path: &str,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: &str,
        author: Author,
    ) {
        let branch = self
            .worktrees
            .get(self.selected_worktree)
            .map(|w| w.branch.clone());

        if let Some(store) = &self.review_store {
            // Invariant: a comment's `worktree` column stores the branch name,
            // `commit_ref` is the symbolic "HEAD", and `branch` is the same
            // branch. The MCP `create_comment` tool (plugins/.../mcp) is a
            // sibling writer that mirrors this exactly — keep the two in sync.
            let wt = self.selected_worktree_branch();
            match store.add_review(
                &wt,
                file_path,
                line_start,
                line_end,
                kind,
                body,
                "HEAD",
                author,
                branch.as_deref(),
            ) {
                Ok(_) => {
                    self.review_state.status_message = Some("Comment added.".to_string());
                    self.record_stat("reviews_created");
                }
                Err(e) => {
                    log::warn!("failed to add review comment: {e}");
                    self.review_state.status_message = Some(format!("Error: {e}"));
                }
            }
            self.review_state.load_comments(store, &wt);
            // Rebuild per-file cache for the commented file.
            self.review_state.build_file_comment_cache(file_path);
        }
    }

    /// Update the body of the currently selected review comment.
    pub fn update_selected_review_body(&mut self, new_body: &str) {
        let id = self.review_state.selected_comment().map(|c| c.id.clone());

        if let (Some(store), Some(id)) = (&self.review_store, id) {
            match store.update_review_body(&id, new_body) {
                Ok(()) => {
                    self.review_state.status_message = Some("Comment updated.".to_string());
                }
                Err(e) => {
                    log::warn!("failed to update review body: {e}");
                    self.review_state.status_message = Some(format!("Error: {e}"));
                }
            }
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
        }
    }

    /// Delete the currently selected review comment (from explorer comment list).
    pub fn delete_selected_review_comment(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.explorer.comment_list_selected);
        let id = comment_idx
            .and_then(|idx| self.review_state.comments.get(idx))
            .map(|c| c.id.clone());

        if let (Some(store), Some(id)) = (&self.review_store, id) {
            match store.delete_review(&id) {
                Ok(()) => {
                    self.status_message = Some(StatusMessage::new(
                        "Comment deleted.".to_string(),
                        StatusLevel::Success,
                        self.ui_tick,
                    ));
                }
                Err(e) => {
                    log::warn!("failed to delete review comment: {e}");
                    self.status_message = Some(StatusMessage::new(
                        format!("Error: {e}"),
                        StatusLevel::Error,
                        self.ui_tick,
                    ));
                }
            }
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // Clamp selection to valid range after deletion (using virtual row count).
            let row_count = self.review_state.comment_list_rows.len();
            if row_count == 0 {
                self.viewer_state.explorer.comment_list_selected = 0;
            } else if self.viewer_state.explorer.comment_list_selected >= row_count {
                self.viewer_state.explorer.comment_list_selected = row_count - 1;
            }
        }
    }

    /// Toggle the status of the currently selected review comment (Pending <-> Resolved).
    pub fn toggle_selected_review_status(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.explorer.comment_list_selected);
        let id_and_status = comment_idx
            .and_then(|idx| self.review_state.comments.get(idx))
            .map(|c| (c.id.clone(), c.status));

        if let (Some(store), Some((id, current_status))) = (&self.review_store, id_and_status) {
            use crate::review_store::CommentStatus;
            let new_status = match current_status {
                CommentStatus::Pending => CommentStatus::Resolved,
                CommentStatus::Resolved => CommentStatus::Pending,
            };
            match store.update_review_status(&id, new_status) {
                Ok(()) => {
                    let label = new_status.as_str();
                    self.status_message = Some(StatusMessage::new(
                        format!("Comment marked as {label}."),
                        StatusLevel::Success,
                        self.ui_tick,
                    ));
                }
                Err(e) => {
                    log::warn!("failed to update review status: {e}");
                    self.status_message = Some(StatusMessage::new(
                        format!("Error: {e}"),
                        StatusLevel::Error,
                        self.ui_tick,
                    ));
                }
            }
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
        }
    }

    /// Add a reply to the currently selected comment (from explorer comment list).
    pub fn add_reply_to_selected_comment(&mut self, body: &str) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.explorer.comment_list_selected);
        let review_id = comment_idx
            .and_then(|idx| self.review_state.comments.get(idx))
            .map(|c| c.id.clone());

        if let (Some(store), Some(review_id)) = (&self.review_store, review_id) {
            match store.add_reply(&review_id, body, Author::User) {
                Ok(()) => {
                    self.status_message = Some(StatusMessage::new(
                        "Reply added.".to_string(),
                        StatusLevel::Success,
                        self.ui_tick,
                    ));
                }
                Err(e) => {
                    log::warn!("failed to add reply: {e}");
                    self.status_message = Some(StatusMessage::new(
                        format!("Error: {e}"),
                        StatusLevel::Error,
                        self.ui_tick,
                    ));
                }
            }
            // Invalidate cached replies and reload.
            self.review_state.cached_replies.remove(&review_id);
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // Reload replies for this comment if it was expanded.
            if self.review_state.expanded_comments.contains(&review_id)
                && let Ok(replies) = store.get_replies(&review_id)
            {
                self.review_state.cached_replies.insert(review_id, replies);
                self.review_state.rebuild_comment_list_rows();
            }
        }
    }

    /// Toggle expansion of the comment thread at the current visual selection.
    ///
    /// Only acts on `CommentListRow::Comment` rows that have replies.
    /// On expand: loads replies from DB, caches them, and rebuilds row list.
    /// On collapse: removes from expanded set and rebuilds.
    pub fn toggle_comment_expansion(&mut self) {
        use crate::review_state::CommentListRow;

        let visual = self.viewer_state.explorer.comment_list_selected;
        let row = self.review_state.comment_list_rows.get(visual).cloned();

        let Some(CommentListRow::Comment { comment_idx }) = row else {
            return;
        };

        let Some(comment) = self.review_state.comments.get(comment_idx) else {
            return;
        };

        let reply_count = self
            .review_state
            .reply_counts
            .get(&comment.id)
            .copied()
            .unwrap_or(0);
        if reply_count == 0 {
            return;
        }

        let comment_id = comment.id.clone();

        if self.review_state.expanded_comments.contains(&comment_id) {
            // Collapse.
            self.review_state.expanded_comments.remove(&comment_id);
            self.review_state.rebuild_comment_list_rows();
            // Clamp selection.
            let row_count = self.review_state.comment_list_rows.len();
            if row_count > 0 && self.viewer_state.explorer.comment_list_selected >= row_count {
                self.viewer_state.explorer.comment_list_selected = row_count - 1;
            }
        } else {
            // Expand — load replies from DB if not cached.
            if !self.review_state.cached_replies.contains_key(&comment_id)
                && let Some(store) = &self.review_store
            {
                match store.get_replies(&comment_id) {
                    Ok(replies) => {
                        self.review_state
                            .cached_replies
                            .insert(comment_id.clone(), replies);
                    }
                    Err(e) => {
                        log::warn!("failed to load replies: {e}");
                        self.set_status(format!("Error loading replies: {e}"), StatusLevel::Error);
                        return;
                    }
                }
            }
            self.review_state.expanded_comments.insert(comment_id);
            self.review_state.rebuild_comment_list_rows();
        }
    }

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
