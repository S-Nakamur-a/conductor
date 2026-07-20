//! Review comment/reply deletion for [`App`].
//!
//! Deletion always goes through a y/n confirmation: `request_delete_*`
//! records the pending target and prompts, `confirm_pending_delete` performs
//! it, and `cancel_pending_delete` backs out without deleting anything.

use super::*;

impl App {
    /// Begin deleting the item under the comment-list selection — a *reply* if a
    /// reply row is selected, otherwise the whole comment. Opens a y/n
    /// confirmation; the delete itself happens in [`Self::confirm_pending_delete`].
    /// (Previously a reply row deleted its parent comment — a data-loss bug.)
    pub fn request_delete_selected_review_item(&mut self) {
        use crate::review_state::PendingDelete;
        let visual = self.viewer_state.explorer.comment_list_selected;
        let target = if let Some((c_idx, r_idx)) = self.review_state.selected_reply_at(visual) {
            self.review_state
                .reply_id_at(c_idx, r_idx)
                .map(|(id, parent_id)| PendingDelete::Reply { id, parent_id })
        } else {
            self.review_state
                .selected_comment_idx(visual)
                .and_then(|idx| self.review_state.comments.get(idx))
                .map(|c| PendingDelete::Comment { id: c.id.clone() })
        };
        if let Some(target) = target {
            self.begin_delete_confirmation(target);
        }
    }

    /// Begin deleting a specific comment by id (e.g. the inline-thread delete
    /// button), via the same y/n confirmation.
    pub fn request_delete_comment_by_id(&mut self, comment_id: String) {
        self.begin_delete_confirmation(crate::review_state::PendingDelete::Comment {
            id: comment_id,
        });
    }

    fn begin_delete_confirmation(&mut self, target: crate::review_state::PendingDelete) {
        use crate::review_state::{PendingDelete, ReviewInputMode};
        let prompt = match &target {
            PendingDelete::Reply { .. } => "Delete this reply? (y/n)",
            PendingDelete::Comment { .. } => "Delete this comment and its replies? (y/n)",
        };
        self.review_state.pending_delete = Some(target);
        self.review_state.input_mode = ReviewInputMode::ConfirmingDelete;
        self.set_status(prompt.to_string(), StatusLevel::Warning);
    }

    /// Cancel a pending delete confirmation without deleting anything.
    pub fn cancel_pending_delete(&mut self) {
        self.review_state.pending_delete = None;
        self.review_state.input_mode = crate::review_state::ReviewInputMode::Normal;
        self.review_state.status_message = None;
    }

    /// Perform the confirmed delete (comment or single reply) and refresh.
    pub fn confirm_pending_delete(&mut self) {
        use crate::review_state::{PendingDelete, ReviewInputMode};
        let Some(target) = self.review_state.pending_delete.take() else {
            self.review_state.input_mode = ReviewInputMode::Normal;
            return;
        };
        self.review_state.input_mode = ReviewInputMode::Normal;
        let wt = self.selected_worktree_branch();
        let msg = if let Some(store) = self.review_store.as_ref() {
            match &target {
                PendingDelete::Comment { id } => {
                    let r = store.delete_review(id);
                    if r.is_ok() {
                        self.review_state.load_comments(store, &wt);
                    }
                    match r {
                        Ok(()) => ("Comment deleted.".to_string(), true),
                        Err(e) => (format!("Error: {e}"), false),
                    }
                }
                PendingDelete::Reply { id, parent_id } => {
                    let r = store.delete_reply(id);
                    if r.is_ok() {
                        self.review_state.load_comments(store, &wt);
                        self.review_state.refresh_replies(store, parent_id);
                    }
                    match r {
                        Ok(()) => ("Reply deleted.".to_string(), true),
                        Err(e) => (format!("Error: {e}"), false),
                    }
                }
            }
        } else {
            return;
        };
        if let Some(file) = self.viewer_state.content.current_file.clone() {
            self.review_state.build_file_comment_cache(&file);
        }
        let row_count = self.review_state.comment_list_rows.len();
        if row_count == 0 {
            self.viewer_state.explorer.comment_list_selected = 0;
        } else if self.viewer_state.explorer.comment_list_selected >= row_count {
            self.viewer_state.explorer.comment_list_selected = row_count - 1;
        }
        let (text, ok) = msg;
        if !ok {
            log::warn!("delete failed: {text}");
        }
        let level = if ok {
            StatusLevel::Success
        } else {
            StatusLevel::Error
        };
        self.status_message = Some(StatusMessage::new(text, level, self.ui_tick));
    }
}
