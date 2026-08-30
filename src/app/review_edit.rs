//! レビューコメント/返信の編集・削除の配線。
//!
//! 実際の状態遷移は [crate::review_state::ReviewState] が持つ。ここは
//! ReviewState が知らない 3 つ — 現在の worktree、レビュー DB、コメント一覧の
//! 選択位置 — を渡し、返ってきた通知をステータスバーへ流すだけ。

use super::*;

impl App {
    /// 現在選択中のレビューコメントの本文を更新する。
    pub fn update_selected_review_body(&mut self, new_body: &str) {
        let wt = self.selected_worktree_branch();
        if let Some(store) = self.review_store.as_ref() {
            self.review_state.update_selected_body(store, &wt, new_body);
        }
    }

    /// コメント一覧の選択位置にある項目の編集を開始する。
    pub fn start_edit_selected_review_item(&mut self) {
        let visual = self.explorer.comments_cursor.selected();
        self.review_state.start_edit_at(visual);
    }

    /// 編集中の返信(EditingReply モード)の本文を保存する。
    pub fn update_selected_reply_body(&mut self, new_body: &str) {
        let wt = self.selected_worktree_branch();
        if let Some(store) = self.review_store.as_ref() {
            self.review_state
                .update_editing_reply_body(store, &wt, new_body);
        }
    }

    /// 現在選択中のレビューコメントのステータスを切り替える(Pending <-> Resolved)。
    pub fn toggle_selected_review_status(&mut self) {
        let wt = self.selected_worktree_branch();
        let visual = self.explorer.comments_cursor.selected();
        let notice = self
            .review_store
            .as_ref()
            .and_then(|store| self.review_state.toggle_status_at(store, &wt, visual));
        self.flash(notice);
    }

    /// (Explorer のコメント一覧から)現在選択中のコメントに返信を追加する。
    pub fn add_reply_to_selected_comment(&mut self, body: &str) {
        let wt = self.selected_worktree_branch();
        let visual = self.explorer.comments_cursor.selected();
        let notice = self
            .review_store
            .as_ref()
            .and_then(|store| self.review_state.add_reply_at(store, &wt, visual, body));
        self.flash(notice);
    }

    /// 現在の表示上の選択位置にあるコメントスレッドの展開状態を切り替える。
    pub fn toggle_comment_expansion(&mut self) {
        let visual = self.explorer.comments_cursor.selected();
        let notice = self
            .review_state
            .toggle_expansion_at(self.review_store.as_ref(), visual);
        self.clamp_comment_list_selection();
        self.flash(notice);
    }

    /// コメント一覧の選択位置にある項目の削除を開始する(y/n の確認を開く)。
    pub fn request_delete_selected_review_item(&mut self) {
        let visual = self.explorer.comments_cursor.selected();
        let notice = self.review_state.request_delete_at(visual);
        self.flash(notice);
    }

    /// id で指定した特定のコメントの削除を、同じ y/n 確認を通して開始する。
    pub fn request_delete_comment_by_id(&mut self, comment_id: String) {
        let notice = self.review_state.request_delete_comment(comment_id);
        self.flash(Some(notice));
    }

    /// 何も削除せずに保留中の削除確認をキャンセルする。
    pub fn cancel_pending_delete(&mut self) {
        self.review_state.cancel_delete();
    }

    /// 確認済みの削除(コメントまたは単一の返信)を実行し、再読み込みする。
    pub fn confirm_pending_delete(&mut self) {
        let wt = self.selected_worktree_branch();
        let file = self.viewer.content.current_file.clone();
        let notice =
            self.review_state
                .confirm_delete(self.review_store.as_ref(), &wt, file.as_deref());
        self.clamp_comment_list_selection();
        self.flash(notice);
    }

    /// コメント一覧の行数が減ったあと、選択位置を範囲内へ収める。
    fn clamp_comment_list_selection(&mut self) {
        let row_count = self.review_state.comment_list_rows.len();
        let selected = &mut self.explorer.comments_cursor.selected();
        *selected = row_count.saturating_sub(1).min(*selected);
    }

    fn flash(&mut self, notice: Option<Notice>) {
        if let Some((text, level)) = notice {
            self.set_status(text, level);
        }
    }
}
