//! Review comment/reply editing, status toggling, and thread expansion for
//! [`App`].

use super::*;
use crate::review_store::Author;

impl App {
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

    /// Start editing the item under the comment-list selection — a reply if a
    /// reply row is selected, otherwise the comment.
    pub fn start_edit_selected_review_item(&mut self) {
        use crate::review_state::ReviewInputMode;
        let visual = self.viewer_state.explorer.comment_list_selected;
        if let Some((c_idx, r_idx)) = self.review_state.selected_reply_at(visual) {
            if let Some((reply_id, parent_id)) = self.review_state.reply_id_at(c_idx, r_idx) {
                let body = self
                    .review_state
                    .cached_replies
                    .get(&parent_id)
                    .and_then(|rs| rs.get(r_idx))
                    .map(|r| r.body.clone())
                    .unwrap_or_default();
                self.review_state.input_buffer.set_text(&body);
                self.review_state.editing_reply = Some((reply_id, parent_id));
                self.review_state.input_mode = ReviewInputMode::EditingReply;
                self.review_state.status_message =
                    Some("Edit reply (Enter to save, Esc to cancel)".to_string());
            }
        } else if let Some(comment_idx) = self.review_state.selected_comment_idx(visual)
            && let Some(comment) = self.review_state.comments.get(comment_idx)
        {
            let body = comment.body.clone();
            self.review_state.input_buffer.set_text(&body);
            self.review_state.input_mode = ReviewInputMode::EditingComment;
            self.review_state.selected = comment_idx;
            self.review_state.status_message =
                Some("Edit comment (Enter to save, Esc to cancel)".to_string());
        }
    }

    /// Save the edited body of the reply being edited (`EditingReply` mode).
    pub fn update_selected_reply_body(&mut self, new_body: &str) {
        let Some((reply_id, parent_id)) = self.review_state.editing_reply.clone() else {
            return;
        };
        let wt = self.selected_worktree_branch();
        let msg = if let Some(store) = self.review_store.as_ref() {
            let r = store.update_reply_body(&reply_id, new_body);
            if r.is_ok() {
                self.review_state.load_comments(store, &wt);
                self.review_state.refresh_replies(store, &parent_id);
            }
            match r {
                Ok(()) => "Reply updated.".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        } else {
            return;
        };
        self.review_state.editing_reply = None;
        self.review_state.status_message = Some(msg);
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
}
