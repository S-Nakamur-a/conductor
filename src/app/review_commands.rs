//! レビューコメントのコマンドハンドラ群: インラインのレビューコメントの
//! 作成、閲覧、編集、返信、削除/解決 — review_state を操作するコマンド
//! パレットの入口。

use super::{App, StatusLevel};
use crate::types::Focus;

impl App {
    pub fn cmd_add_review_comment(&mut self) {
        if let Some(file_path) = self.viewer.content.current_file.clone() {
            // コメントを選択範囲(なければ先頭の可視行)にアンカーし、その行に
            // 本文だけを入力するインラインの作成ボックスを開く — GitHub 風に
            // file:line のプレフィックスは入力させない。
            let (start, end) = if let Some((start, end)) = self.viewer.selected_range() {
                (
                    start as u32,
                    if start == end { None } else { Some(end as u32) },
                )
            } else {
                ((self.viewer.content.file_scroll + 1) as u32, None)
            };
            self.viewer.clear_selection();
            self.review_state.input_anchor = Some((file_path, start, end));
            self.review_state.input_buffer.clear();
            self.review_state.input_kind = crate::review_store::CommentKind::Suggest;
            self.review_state.input_mode = crate::review_state::ReviewInputMode::AddingComment;
            self.review_state.status_message = None;
            self.set_focus(Focus::Viewer);
        } else {
            self.set_status("No file open in viewer.".to_string(), StatusLevel::Warning);
        }
    }

    pub(super) fn cmd_view_comment_detail(&mut self) {
        // まず viewer のコンテキスト(現在行)を試し、次にコメント一覧の
        // コンテキストを試す。
        if self.viewer.content.current_file.is_some() {
            let cursor_line = if let Some((start, _)) = self.viewer.selected_range() {
                start
            } else {
                self.viewer.content.file_scroll + 1
            };
            if let Some(comments) = self.review_state.file_comments.get(&cursor_line)
                && !comments.is_empty()
            {
                let target_id = &comments[0].id;
                if let Some(idx) = self
                    .review_state
                    .comments
                    .iter()
                    .position(|c| c.id == *target_id)
                {
                    let cid = target_id.clone();
                    if !self.review_state.cached_replies.contains_key(&cid)
                        && let Some(store) = self.review_store.as_ref()
                        && let Ok(replies) = store.get_replies(&cid)
                    {
                        self.review_state.cached_replies.insert(cid, replies);
                    }
                    self.review_state.comment_detail_idx = idx;
                    self.review_state.comment_detail_scroll = 0;
                    self.review_state.comment_detail_active = true;
                    self.set_focus(Focus::Viewer);
                    return;
                }
            }
        }
        self.set_status(
            "No comment on current line.".to_string(),
            StatusLevel::Warning,
        );
    }

    pub(super) fn cmd_delete_comment(&mut self) {
        if self.explorer.bottom_view == crate::viewer::ExplorerBottomView::Comments
            && self.explorer.focus_on_diff_list
            && !self.review_state.comment_list_rows.is_empty()
        {
            self.request_delete_selected_review_item();
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }

    pub(super) fn cmd_toggle_comment_resolve(&mut self) {
        if self.explorer.bottom_view == crate::viewer::ExplorerBottomView::Comments
            && self.explorer.focus_on_diff_list
            && !self.review_state.comment_list_rows.is_empty()
        {
            self.toggle_selected_review_status();
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }

    pub(super) fn cmd_edit_comment(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.explorer.comment_list_selected);
        if let Some(comment) = comment_idx.and_then(|idx| self.review_state.comments.get(idx)) {
            self.review_state.input_buffer.set_text(&comment.body);
            self.review_state.input_mode = crate::review_state::ReviewInputMode::EditingComment;
            self.review_state.selected = comment_idx.unwrap();
            self.review_state.status_message =
                Some("Edit comment (Enter to save, Esc to cancel)".to_string());
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }

    pub(super) fn cmd_reply_to_comment(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.explorer.comment_list_selected);
        if let Some(idx) = comment_idx {
            self.review_state.input_buffer.clear();
            self.review_state.input_mode = crate::review_state::ReviewInputMode::ReplyingToComment;
            self.review_state.selected = idx;
            self.review_state.status_message =
                Some("Reply to comment (Enter to send, Esc to cancel)".to_string());
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }
}
