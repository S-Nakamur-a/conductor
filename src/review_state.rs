//! レビューモードの状態。レビューパネルの UI 状態を保持する。
//!
//! 現在表示しているコメントの一覧、選択、スクロール、そしてレビューコメントの
//! 追加・編集のための入力モードを扱う。

use std::collections::{HashMap, HashSet};

use crate::review_store::{CommentKind, CommentTemplate, ReviewComment, ReviewReply, ReviewStore};
use crate::text_input::TextInput;

/// 仮想的なコメント一覧の 1 行。
///
/// コメントのスレッドを展開すると、親コメントの行のあとに返信の行が並ぶ。この
/// 列挙型のおかげで、UI とイベントハンドラは親と返信の関係を保ったまま一覧を
/// 平坦な列として扱える。
#[derive(Debug, Clone)]
pub enum CommentListRow {
    /// `ReviewState::comments` の指定添字にあるトップレベルのコメント。
    Comment { comment_idx: usize },
    /// `comment_idx` のコメントに属する返信。
    Reply {
        comment_idx: usize,
        reply_idx: usize,
    },
}

/// レビューパネルの入力モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewInputMode {
    /// コメント一覧を移動している。
    Normal,
    /// 新しいコメント本文を入力している (形式: "file:line body")。
    AddingComment,
    /// 既存コメントの本文を編集している。
    EditingComment,
    /// 既存の返信の本文を編集している。
    EditingReply,
    /// 既存コメントへ返信している。
    ReplyingToComment,
    /// コメントまたは返信を削除する前の y/n 確認を待っている。
    ConfirmingDelete,
}

/// 確認待ちの削除が何を対象にしているか。
#[derive(Debug, Clone)]
pub enum PendingDelete {
    /// コメント全体を削除する (返信も連鎖して消える)。
    Comment { id: String },
    /// 返信 1 件だけを削除し、親コメントは残す。
    Reply { id: String, parent_id: String },
}

/// レビューモードの UI 状態。
pub struct ReviewState {
    /// 現在の worktree のコメント。データベースから読み込む。
    pub comments: Vec<ReviewComment>,
    /// 現在選択しているコメントの添字。
    pub selected: usize,
    /// 現在の入力モード。
    pub input_mode: ReviewInputMode,
    /// 入力欄のテキストバッファ (追加・編集中に使う)。
    pub input_buffer: TextInput,
    /// 作成中のコメントの種別 (Suggest か Question)。
    pub input_kind: CommentKind,
    /// 作成中の新規コメントの対象: `(file_path, line_start, line_end)`。
    /// 設定されている (`AddingComment` の) あいだ、入力ボックスはその行に
    /// インラインで描かれ、バッファは本文だけを持つ (`file:line` の接頭辞は無い)。
    /// `None` のときは、バッファ内の接頭辞をパースする従来の経路
    /// (テンプレートピッカーやコマンドパレットからの入口) に落ちる。
    pub input_anchor: Option<(String, u32, Option<u32>)>,
    /// パネル下部に出す一時的なメッセージ。
    pub status_message: Option<String>,
    /// コメントに対する現在の検索・絞り込みクエリ。
    pub search_query: TextInput,
    /// 検索入力が有効かどうか。
    pub search_active: bool,
    /// 絞り込み後のコメントの添字 (`comments` に対する添字)。
    pub filtered_indices: Vec<usize>,
    /// データベースから読み込んだ、利用可能なコメントテンプレート。
    pub templates: Vec<CommentTemplate>,
    /// テンプレートピッカーが表示中かどうか。
    pub template_picker_active: bool,
    /// ピッカー内で現在選択しているテンプレートの添字。
    pub template_selected: usize,
    /// 現在表示中のファイルのコメントのキャッシュ。1 始まりの行番号がキー。
    pub file_comments: HashMap<usize, Vec<ReviewComment>>,
    /// `file_comments` を作ったときのファイルパス (キャッシュ無効化のため)。
    pub file_comments_path: Option<String>,
    /// コメント ID ごとの返信数のキャッシュ。コメントと一緒に読み込む。
    pub reply_counts: HashMap<String, usize>,
    /// 返信スレッドを展開しているコメント ID の集合。
    pub expanded_comments: HashSet<String>,
    /// 展開中コメントの返信のキャッシュ。コメント ID がキー。
    pub cached_replies: HashMap<String, Vec<ReviewReply>>,
    /// コメントパネルの仮想的な行一覧 (展開状態が変わるたびに作り直す)。
    pub comment_list_rows: Vec<CommentListRow>,

    // ── コメント詳細のオーバーレイ ──────────────────────────────
    /// コメント詳細モーダルが表示中かどうか。
    pub comment_detail_active: bool,
    /// 詳細モーダル内のスクロール位置。
    pub comment_detail_scroll: usize,
    /// スクロール位置の上限 (描画時に設定される)。
    pub comment_detail_max_scroll: usize,
    /// 詳細モーダルで表示しているコメントの添字。
    pub comment_detail_idx: usize,

    /// ブランチ単位の変更サマリ (差分全体の「何を・なぜ」)。コメントと一緒に
    /// 読み込み、差分の上にバナーとして描画する。現在のブランチにサマリが
    /// 書かれていなければ `None`。
    pub change_summary: Option<String>,

    /// y/n の確認を待っている削除の対象 (`input_mode == ConfirmingDelete` の
    /// あいだ設定される)。
    pub pending_delete: Option<PendingDelete>,
    /// 編集中の返信の `(reply_id, parent_comment_id)`
    /// (`input_mode == EditingReply` のあいだ設定される)。
    pub editing_reply: Option<(String, String)>,
}

impl ReviewState {
    /// 空の既定値で `ReviewState` を作る。
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

    /// 見た目上の行が返信の行であれば `(comment_idx, reply_idx)` に解決する。
    pub fn selected_reply_at(&self, visual_idx: usize) -> Option<(usize, usize)> {
        match self.comment_list_rows.get(visual_idx) {
            Some(CommentListRow::Reply {
                comment_idx,
                reply_idx,
            }) => Some((*comment_idx, *reply_idx)),
            _ => None,
        }
    }

    /// 親コメントの返信キャッシュを介して、`(comment_idx, reply_idx)` を
    /// `(reply_id, parent_comment_id)` に解決する。
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

    /// `comments`, `expanded_comments`, `cached_replies` から仮想的な行一覧を
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
    /// `self.comments` を `file_path` で絞り込み、コメントの範囲に含まれる各行を、
    /// その行を覆うコメントの列へ対応づける。解決済みのコメントもここには残す。
    /// 溝にバッジを出し続けるため。解決済みコメントを隠しているのはインラインの
    /// スレッド展開のほう (`build_inline_thread_lines` を参照)。
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
            commit_ref: "abc".to_string(),
            author: Author::User,
            branch: None,
            created_at: String::new(),
            updated_at: String::new(),
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
