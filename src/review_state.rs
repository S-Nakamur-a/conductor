//! レビューパネルの UI 状態。

use std::collections::{HashMap, HashSet};

use crate::review_store::{
    Author, CommentKind, CommentStatus, CommentTemplate, ReviewComment, ReviewReply, ReviewStore,
};
use crate::text_input::TextInput;
use crate::types::{Notice, StatusLevel};

/// コメント一覧の 1 行。展開すると親の行のあとに返信の行が並ぶので、UI と
/// イベントハンドラは親子関係を保ったまま平坦な列として扱える。
#[derive(Debug, Clone)]
pub enum CommentListRow {
    Comment {
        comment_idx: usize,
    },
    Reply {
        comment_idx: usize,
        reply_idx: usize,
    },
}

/// レビューパネルの入力モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewInputMode {
    Normal,
    /// バッファの形式は "file:line body"。
    AddingComment,
    EditingComment,
    EditingReply,
    ReplyingToComment,
    ConfirmingDelete,
}

/// 確認待ちの削除の対象。
#[derive(Debug, Clone)]
pub enum PendingDelete {
    /// 返信も連鎖して消える。
    Comment { id: String },
    /// 親コメントは残す。
    Reply { id: String, parent_id: String },
}

/// レビューモードの UI 状態。
pub struct ReviewState {
    /// 現在の worktree のコメント。DB から読み込む。
    pub comments: Vec<ReviewComment>,
    pub selected: usize,
    pub input_mode: ReviewInputMode,
    pub input_buffer: TextInput,
    pub input_kind: CommentKind,
    /// 作成中のコメントの対象 (file_path, line_start, line_end)。
    ///
    /// Some のあいだ入力欄はその行にインラインで描かれ、バッファは本文だけを持つ。
    /// None なら "file:line " の接頭辞をバッファからパースする従来の経路に落ちる
    /// (テンプレートピッカーやコマンドパレットからの入口)。
    pub input_anchor: Option<(String, u32, Option<u32>)>,
    /// パネル下部に出す一時的なメッセージ。
    pub status_message: Option<String>,
    pub search_query: TextInput,
    pub search_active: bool,
    /// 絞り込み後の、comments に対する添字。
    pub filtered_indices: Vec<usize>,
    pub templates: Vec<CommentTemplate>,
    pub template_picker_active: bool,
    pub template_selected: usize,
    /// 表示中のファイルのコメント。1 始まりの行番号がキー。
    pub file_comments: HashMap<usize, Vec<ReviewComment>>,
    /// file_comments を作ったときのパス。無効化の判定に使う。
    pub file_comments_path: Option<String>,
    /// コメント ID ごとの返信数。コメントと一緒に読み込む。
    pub reply_counts: HashMap<String, usize>,
    pub expanded_comments: HashSet<String>,
    /// 展開中のコメントの返信。コメント ID がキー。
    pub cached_replies: HashMap<String, Vec<ReviewReply>>,
    /// 展開状態が変わるたびに作り直す。
    pub comment_list_rows: Vec<CommentListRow>,

    pub comment_detail_active: bool,
    pub comment_detail_scroll: usize,
    /// 描画が書き込む上限。
    pub comment_detail_max_scroll: usize,
    pub comment_detail_idx: usize,

    /// 差分全体の「何を・なぜ」。コメントと一緒に読み込み、差分の上にバナーで出す。
    pub change_summary: Option<String>,

    /// input_mode == ConfirmingDelete のあいだ入る。
    pub pending_delete: Option<PendingDelete>,
    /// input_mode == EditingReply のあいだ入る (reply_id, parent_comment_id)。
    pub editing_reply: Option<(String, String)>,
}

impl ReviewState {
    pub fn new() -> Self {
        Self {
            comments: Vec::new(),
            selected: 0,
            input_mode: ReviewInputMode::Normal,
            input_buffer: TextInput::new_multiline(),
            input_kind: CommentKind::Suggest,
            input_anchor: None,
            status_message: None,
            search_query: TextInput::new(),
            search_active: false,
            filtered_indices: Vec::new(),
            templates: Vec::new(),
            template_picker_active: false,
            template_selected: 0,
            file_comments: HashMap::new(),
            file_comments_path: None,
            reply_counts: HashMap::new(),
            expanded_comments: HashSet::new(),
            cached_replies: HashMap::new(),
            comment_list_rows: Vec::new(),
            comment_detail_active: false,
            comment_detail_scroll: 0,
            comment_detail_max_scroll: 0,
            comment_detail_idx: 0,
            change_summary: None,
            pending_delete: None,
            editing_reply: None,
        }
    }

    /// 見た目上の行が返信の行であれば (comment_idx, reply_idx) に解決する。
    pub fn selected_reply_at(&self, visual_idx: usize) -> Option<(usize, usize)> {
        match self.comment_list_rows.get(visual_idx) {
            Some(CommentListRow::Reply {
                comment_idx,
                reply_idx,
            }) => Some((*comment_idx, *reply_idx)),
            _ => None,
        }
    }

    /// 親コメントの返信キャッシュを介して、(comment_idx, reply_idx) を
    /// (reply_id, parent_comment_id) に解決する。
    pub fn reply_id_at(&self, comment_idx: usize, reply_idx: usize) -> Option<(String, String)> {
        let comment = self.comments.get(comment_idx)?;
        let replies = self.cached_replies.get(&comment.id)?;
        let reply = replies.get(reply_idx)?;
        Some((reply.id.clone(), comment.id.clone()))
    }

    /// (返信の追加・編集・削除のあとに) コメント 1 件の返信をキャッシュへ取り直し、
    /// 返信数を更新し、変更がスレッドに反映されるよう仮想的な行一覧を作り直す。
    pub fn refresh_replies(&mut self, store: &ReviewStore, comment_id: &str) {
        match store.get_replies(comment_id) {
            Ok(replies) => {
                self.reply_counts
                    .insert(comment_id.to_string(), replies.len());
                if replies.is_empty() {
                    self.cached_replies.remove(comment_id);
                } else {
                    self.cached_replies.insert(comment_id.to_string(), replies);
                }
            }
            Err(e) => log::warn!("failed to refresh replies: {e}"),
        }
        self.rebuild_comment_list_rows();
    }

    /// 指定した worktree のコメントをデータベースから読み直す。
    pub fn load_comments(&mut self, store: &ReviewStore, worktree: &str) {
        match store.reviews_for_worktree(worktree) {
            Ok(comments) => {
                self.comments = comments;
                self.filtered_indices = (0..self.comments.len()).collect();
                // 選択を有効な範囲に収める。
                if !self.comments.is_empty() && self.selected >= self.comments.len() {
                    self.selected = self.comments.len() - 1;
                }
            }
            Err(e) => {
                log::warn!("failed to load review comments: {e}");
                self.comments.clear();
                self.filtered_indices.clear();
                self.selected = 0;
            }
        }
        // この worktree の全コメントについて返信数を読み込む。
        match store.reply_counts_for_worktree(worktree) {
            Ok(counts) => {
                self.reply_counts = counts;
            }
            Err(e) => {
                log::warn!("failed to load reply counts: {e}");
                self.reply_counts.clear();
            }
        }
        // この worktree のブランチ単位の変更サマリを読み込む。
        match store.get_change_summary(worktree) {
            Ok(summary) => self.change_summary = summary,
            Err(e) => {
                log::warn!("failed to load change summary: {e}");
                self.change_summary = None;
            }
        }
        // もう存在しないコメントの展開状態を掃除する。
        let current_ids: HashSet<String> = self.comments.iter().map(|c| c.id.clone()).collect();
        self.expanded_comments.retain(|id| current_ids.contains(id));
        self.cached_replies.retain(|id, _| current_ids.contains(id));
        self.rebuild_comment_list_rows();
    }

    /// comments, expanded_comments, cached_replies から仮想的な行一覧を
    /// 作り直す。
    pub fn rebuild_comment_list_rows(&mut self) {
        self.comment_list_rows.clear();
        for (comment_idx, comment) in self.comments.iter().enumerate() {
            self.comment_list_rows
                .push(CommentListRow::Comment { comment_idx });
            if self.expanded_comments.contains(&comment.id)
                && let Some(replies) = self.cached_replies.get(&comment.id)
            {
                for reply_idx in 0..replies.len() {
                    self.comment_list_rows.push(CommentListRow::Reply {
                        comment_idx,
                        reply_idx,
                    });
                }
            }
        }
    }

    /// 見た目上の行の添字を、親コメントの添字に解決する。
    pub fn selected_comment_idx(&self, visual_idx: usize) -> Option<usize> {
        match self.comment_list_rows.get(visual_idx) {
            Some(CommentListRow::Comment { comment_idx }) => Some(*comment_idx),
            Some(CommentListRow::Reply { comment_idx, .. }) => Some(*comment_idx),
            None => None,
        }
    }

    /// 現在選択しているコメントへの参照を返す (あれば)。
    pub fn selected_comment(&self) -> Option<&ReviewComment> {
        self.comments.get(self.selected)
    }

    /// 現在の検索クエリを適用してコメント一覧を絞り込む。
    pub fn apply_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_indices = (0..self.comments.len()).collect();
        } else {
            let query_lower = self.search_query.to_lowercase();
            self.filtered_indices = self
                .comments
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.body.to_lowercase().contains(&query_lower)
                        || c.file_path.to_lowercase().contains(&query_lower)
                })
                .map(|(i, _)| i)
                .collect();
        }
        // 選択を範囲内に収める。
        if !self.filtered_indices.is_empty() && self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }

    /// メモリ上のコメントから、ファイル単位のコメントキャッシュを作る。
    ///
    /// self.comments を file_path で絞り込み、コメントの範囲に含まれる各行を、
    /// その行を覆うコメントの列へ対応づける。解決済みのコメントもここには残す。
    /// 溝にバッジを出し続けるため。解決済みコメントを隠しているのはインラインの
    /// スレッド展開のほう (build_inline_thread_lines を参照)。
    pub fn build_file_comment_cache(&mut self, file_path: &str) {
        self.file_comments.clear();
        self.file_comments_path = Some(file_path.to_string());

        for comment in &self.comments {
            if comment.file_path != file_path {
                continue;
            }
            let start = comment.line_start as usize;
            let end = comment.line_end.unwrap_or(comment.line_start) as usize;
            for line in start..=end {
                self.file_comments
                    .entry(line)
                    .or_default()
                    .push(comment.clone());
            }
        }
    }

    /// コメントテンプレートをデータベースから読み込む。
    pub fn load_templates(&mut self, store: &ReviewStore) {
        match store.list_templates() {
            Ok(templates) => {
                self.templates = templates;
                if !self.templates.is_empty() && self.template_selected >= self.templates.len() {
                    self.template_selected = self.templates.len() - 1;
                }
            }
            Err(e) => {
                log::warn!("failed to load comment templates: {e}");
                self.templates.clear();
                self.template_selected = 0;
            }
        }
    }
}

/// レビューコメント/返信の編集・ステータス切り替え・スレッド展開・削除。
///
/// 削除は常に y/n の確認を経る: request_delete_at / begin_delete が保留中の
/// 対象を記録してプロンプトを返し、confirm_delete が実際に削除し、
/// cancel_delete は何も削除せずに取り消す。
impl ReviewState {
    /// 現在選択中のコメントの本文を更新する。
    pub fn update_selected_body(&mut self, store: &ReviewStore, worktree: &str, new_body: &str) {
        let Some(id) = self.selected_comment().map(|c| c.id.clone()) else {
            return;
        };
        self.status_message = Some(match store.update_review_body(&id, new_body) {
            Ok(()) => "Comment updated.".to_string(),
            Err(e) => {
                log::warn!("failed to update review body: {e}");
                format!("Error: {e}")
            }
        });
        self.load_comments(store, worktree);
    }

    /// 表示上の選択位置にある項目の編集を開始する — 返信の行が選択されて
    /// いれば返信を、そうでなければコメントを対象にする。
    pub fn start_edit_at(&mut self, visual: usize) {
        if let Some((c_idx, r_idx)) = self.selected_reply_at(visual) {
            let Some((reply_id, parent_id)) = self.reply_id_at(c_idx, r_idx) else {
                return;
            };
            let body = self
                .cached_replies
                .get(&parent_id)
                .and_then(|rs| rs.get(r_idx))
                .map(|r| r.body.clone())
                .unwrap_or_default();
            self.input_buffer.set_text(&body);
            self.editing_reply = Some((reply_id, parent_id));
            self.input_mode = ReviewInputMode::EditingReply;
            self.status_message = Some("Edit reply (Enter to save, Esc to cancel)".to_string());
        } else if let Some(comment_idx) = self.selected_comment_idx(visual)
            && let Some(comment) = self.comments.get(comment_idx)
        {
            let body = comment.body.clone();
            self.input_buffer.set_text(&body);
            self.input_mode = ReviewInputMode::EditingComment;
            self.selected = comment_idx;
            self.status_message = Some("Edit comment (Enter to save, Esc to cancel)".to_string());
        }
    }

    /// 編集中の返信(EditingReply モード)の本文を保存する。
    pub fn update_editing_reply_body(
        &mut self,
        store: &ReviewStore,
        worktree: &str,
        new_body: &str,
    ) {
        let Some((reply_id, parent_id)) = self.editing_reply.clone() else {
            return;
        };
        let result = store.update_reply_body(&reply_id, new_body);
        if result.is_ok() {
            self.load_comments(store, worktree);
            self.refresh_replies(store, &parent_id);
        }
        self.editing_reply = None;
        self.status_message = Some(match result {
            Ok(()) => "Reply updated.".to_string(),
            Err(e) => format!("Error: {e}"),
        });
    }

    /// 表示上の選択位置にあるコメントのステータスを切り替える(Pending <-> Resolved)。
    pub fn toggle_status_at(
        &mut self,
        store: &ReviewStore,
        worktree: &str,
        visual: usize,
    ) -> Option<Notice> {
        let (id, current) = self
            .selected_comment_idx(visual)
            .and_then(|idx| self.comments.get(idx))
            .map(|c| (c.id.clone(), c.status))?;

        let new_status = match current {
            CommentStatus::Pending => CommentStatus::Resolved,
            CommentStatus::Resolved => CommentStatus::Pending,
        };
        let notice = match store.update_review_status(&id, new_status) {
            Ok(()) => (
                format!("Comment marked as {}.", new_status.as_str()),
                StatusLevel::Success,
            ),
            Err(e) => {
                log::warn!("failed to update review status: {e}");
                (format!("Error: {e}"), StatusLevel::Error)
            }
        };
        self.load_comments(store, worktree);
        Some(notice)
    }

    /// 表示上の選択位置にあるコメントへ返信を追加する。
    pub fn add_reply_at(
        &mut self,
        store: &ReviewStore,
        worktree: &str,
        visual: usize,
        body: &str,
    ) -> Option<Notice> {
        let review_id = self
            .selected_comment_idx(visual)
            .and_then(|idx| self.comments.get(idx))
            .map(|c| c.id.clone())?;

        let notice = match store.add_reply(&review_id, body, Author::User) {
            Ok(()) => ("Reply added.".to_string(), StatusLevel::Success),
            Err(e) => {
                log::warn!("failed to add reply: {e}");
                (format!("Error: {e}"), StatusLevel::Error)
            }
        };
        // キャッシュ済みの返信を無効化して再読み込みする。
        self.cached_replies.remove(&review_id);
        self.load_comments(store, worktree);
        if self.expanded_comments.contains(&review_id)
            && let Ok(replies) = store.get_replies(&review_id)
        {
            self.cached_replies.insert(review_id, replies);
            self.rebuild_comment_list_rows();
        }
        Some(notice)
    }

    /// 表示上の選択位置にあるコメントスレッドの展開状態を切り替える。
    ///
    /// 返信を持つ CommentListRow::Comment の行にのみ作用する。展開時は
    /// DB から返信を読み込んでキャッシュし、行リストを作り直す。
    pub fn toggle_expansion_at(
        &mut self,
        store: Option<&ReviewStore>,
        visual: usize,
    ) -> Option<Notice> {
        let Some(CommentListRow::Comment { comment_idx }) = self.comment_list_rows.get(visual)
        else {
            return None;
        };
        let comment = self.comments.get(*comment_idx)?;
        if self.reply_counts.get(&comment.id).copied().unwrap_or(0) == 0 {
            return None;
        }
        let comment_id = comment.id.clone();

        if self.expanded_comments.contains(&comment_id) {
            self.expanded_comments.remove(&comment_id);
        } else {
            if !self.cached_replies.contains_key(&comment_id)
                && let Some(store) = store
            {
                match store.get_replies(&comment_id) {
                    Ok(replies) => {
                        self.cached_replies.insert(comment_id.clone(), replies);
                    }
                    Err(e) => {
                        log::warn!("failed to load replies: {e}");
                        return Some((format!("Error loading replies: {e}"), StatusLevel::Error));
                    }
                }
            }
            self.expanded_comments.insert(comment_id);
        }
        self.rebuild_comment_list_rows();
        None
    }

    /// 表示上の選択位置にある項目の削除を開始する — 返信の行が選択されて
    /// いれば返信そのものを、そうでなければコメント全体を対象にする。
    /// 実際の削除は [Self::confirm_delete] が行う。確認プロンプトを返す。
    /// (以前は返信の行を選んでも親コメントが削除されるデータロスのバグがあった。)
    pub fn request_delete_at(&mut self, visual: usize) -> Option<Notice> {
        let target = if let Some((c_idx, r_idx)) = self.selected_reply_at(visual) {
            self.reply_id_at(c_idx, r_idx)
                .map(|(id, parent_id)| PendingDelete::Reply { id, parent_id })
        } else {
            self.selected_comment_idx(visual)
                .and_then(|idx| self.comments.get(idx))
                .map(|c| PendingDelete::Comment { id: c.id.clone() })
        };
        Some(self.begin_delete(target?))
    }

    /// id で指定したコメントの削除を、同じ y/n 確認を通して開始する。
    pub fn request_delete_comment(&mut self, comment_id: String) -> Notice {
        self.begin_delete(PendingDelete::Comment { id: comment_id })
    }

    fn begin_delete(&mut self, target: PendingDelete) -> Notice {
        let prompt = match &target {
            PendingDelete::Reply { .. } => "Delete this reply? (y/n)",
            PendingDelete::Comment { .. } => "Delete this comment and its replies? (y/n)",
        };
        self.pending_delete = Some(target);
        self.input_mode = ReviewInputMode::ConfirmingDelete;
        (prompt.to_string(), StatusLevel::Warning)
    }

    /// 何も削除せずに保留中の削除確認をキャンセルする。
    pub fn cancel_delete(&mut self) {
        self.pending_delete = None;
        self.input_mode = ReviewInputMode::Normal;
        self.status_message = None;
    }

    /// 確認済みの削除(コメントまたは単一の返信)を実行し、再読み込みする。
    /// current_file はコメントバッジのキャッシュを作り直す対象。
    pub fn confirm_delete(
        &mut self,
        store: Option<&ReviewStore>,
        worktree: &str,
        current_file: Option<&str>,
    ) -> Option<Notice> {
        let target = self.pending_delete.take();
        self.input_mode = ReviewInputMode::Normal;
        let (target, store) = (target?, store?);

        let (result, ok_text) = match &target {
            PendingDelete::Comment { id } => (store.delete_review(id), "Comment deleted."),
            PendingDelete::Reply { id, .. } => (store.delete_reply(id), "Reply deleted."),
        };
        if result.is_ok() {
            self.load_comments(store, worktree);
            if let PendingDelete::Reply { parent_id, .. } = &target {
                self.refresh_replies(store, parent_id);
            }
        }
        if let Some(file) = current_file {
            self.build_file_comment_cache(file);
        }
        Some(match result {
            Ok(()) => (ok_text.to_string(), StatusLevel::Success),
            Err(e) => {
                log::warn!("delete failed: {e}");
                (format!("Error: {e}"), StatusLevel::Error)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_store::{Author, CommentKind, CommentStatus, ReviewComment};

    fn comment(id: &str, line: u32, status: CommentStatus) -> ReviewComment {
        range_comment(id, line, None, status)
    }

    fn range_comment(
        id: &str,
        line_start: u32,
        line_end: Option<u32>,
        status: CommentStatus,
    ) -> ReviewComment {
        ReviewComment {
            id: id.to_string(),
            worktree: "wt".to_string(),
            file_path: "src/main.rs".to_string(),
            line_start,
            line_end,
            kind: CommentKind::Suggest,
            body: "body".to_string(),
            status,
            author: Author::User,
            branch: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn build_file_comment_cache_keeps_resolved_for_badges() {
        let mut state = ReviewState::new();
        state.comments = vec![
            comment("c1", 10, CommentStatus::Pending),
            comment("c2", 20, CommentStatus::Resolved),
        ];

        state.build_file_comment_cache("src/main.rs");

        // どちらの行にもキャッシュエントリが残り、溝のバッジは出続ける。
        // 解決済みコメントが隠れるのはインラインのスレッド展開だけ。
        assert!(state.file_comments.contains_key(&10));
        assert!(state.file_comments.contains_key(&20));
    }

    #[test]
    fn build_file_comment_cache_supports_overlapping_ranges() {
        // 範囲が入れ子になる 2 つのコメント (L10-L20 と L11-L19) は共存できなければ
        // ならない。共有する行は両方を持ち、それぞれが自分の終端行 (💬 バッジと
        // インラインスレッドが置かれる行) を別々に保つ。
        let mut state = ReviewState::new();
        state.comments = vec![
            range_comment("outer", 10, Some(20), CommentStatus::Pending),
            range_comment("inner", 11, Some(19), CommentStatus::Pending),
        ];

        state.build_file_comment_cache("src/main.rs");

        let on = |line: usize| -> Vec<&str> {
            state.file_comments[&line]
                .iter()
                .map(|c| c.id.as_str())
                .collect()
        };
        // 両方の範囲に入る行は両方のコメントを見る。
        assert_eq!(on(15), vec!["outer", "inner"]);
        // 外側の範囲だけが覆う境界行は、それだけを見る。
        assert_eq!(on(10), vec!["outer"]);
        assert_eq!(on(20), vec!["outer"]);
        // 各コメントの終端行には、そのコメント自身が入っている。
        assert!(on(19).contains(&"inner"));
    }
}
