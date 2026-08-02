//! [App] におけるレビューコメント/返信の削除。
//!
//! 削除は常に y/n の確認を経る: request_delete_* が保留中の対象を記録して
//! プロンプトを出し、confirm_pending_delete が実際に削除し、
//! cancel_pending_delete は何も削除せずに取り消す。

use super::*;

impl App {
    /// コメント一覧の選択位置にある項目の削除を開始する — 返信の行が選択
    /// されていれば返信そのものを、そうでなければコメント全体を対象にする。
    /// y/n の確認を開く。削除自体は [Self::confirm_pending_delete] で行われる。
    /// (以前は返信の行を選んでも親コメントが削除されるデータロスのバグがあった。)
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

    /// (インラインスレッドの削除ボタンなど)id で指定した特定のコメントの
    /// 削除を、同じ y/n 確認を通して開始する。
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

    /// 何も削除せずに保留中の削除確認をキャンセルする。
    pub fn cancel_pending_delete(&mut self) {
        self.review_state.pending_delete = None;
        self.review_state.input_mode = crate::review_state::ReviewInputMode::Normal;
        self.review_state.status_message = None;
    }

    /// 確認済みの削除(コメントまたは単一の返信)を実行し、再読み込みする。
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
