//! review_store 配下のサブモジュールが共有する enum と単純なデータ構造体。

/// レビューコメントの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    Suggest,
    Question,
}

impl CommentKind {
    /// データベースに保存される文字列表現に変換する。
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentKind::Suggest => "suggest",
            CommentKind::Question => "question",
        }
    }
}

impl std::fmt::Display for CommentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// レビューコメントまたは返信の投稿者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Claude,
}

impl Author {
    /// データベースに保存される文字列表現に変換する。
    pub fn as_str(&self) -> &'static str {
        match self {
            Author::User => "user",
            Author::Claude => "claude",
        }
    }
}

impl std::fmt::Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// レビューコメントの解決状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentStatus {
    Pending,
    Resolved,
}

impl CommentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentStatus::Pending => "pending",
            CommentStatus::Resolved => "resolved",
        }
    }
}

impl std::fmt::Display for CommentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ファイルと行範囲に紐づく単一のレビューコメント。
#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub id: String,
    pub worktree: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub kind: CommentKind,
    pub body: String,
    pub status: CommentStatus,
    pub author: Author,
    pub branch: Option<String>,
    pub created_at: String,
}

/// 再利用可能なコメントテンプレート（保存済みのフィードバックパターン）。
#[derive(Debug, Clone)]
pub struct CommentTemplate {
    pub id: String,
    pub name: String,
    pub body: String,
    pub kind: CommentKind,
}

/// レビューコメントへの返信。
#[derive(Debug, Clone)]
pub struct ReviewReply {
    pub id: String,
    pub body: String,
    pub author: Author,
    pub created_at: String,
}

/// 日次のアクティビティ統計。
#[derive(Debug, Clone, PartialEq)]
pub struct DailyStats {
    pub reviews_created: i64,
    pub branches_created: i64,
    pub commits_made: i64,
}

/// 現在のセッションの集計統計。
#[derive(Debug, Clone, Default)]
pub struct SessionStatsSnapshot {
    pub reviews_created: i64,
    pub branches_created: i64,
    pub commits_made: i64,
}

/// 連続活動日数の情報。
#[derive(Debug, Clone)]
pub struct StreakInfo {
    pub consecutive_days: u32,
}

/// レビューモード用ブランチの PR メタデータ（pr_review_meta テーブル）。
/// レビューヘッダーの表示と、後でコメントを正しい PR に投稿する際に
/// 必要になる情報を持つ。全フィールドが optional なのは、レビューが PR の
/// ないブランチ名だけからも始められるため。
#[derive(Debug, Clone)]
///
/// base_ref は列としては残っているが、ここには載せていない。読み手は
/// walkthrough の生成だけで、revidere は自分で base を決めるため。行の記録
/// としては書き続ける (PR の素性を後から辿れる)。
pub struct PrReviewMeta {
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
}

/// 保存されたセッション履歴レコード。
#[derive(Debug, Clone)]
pub struct SessionHistory {
    pub worktree: String,
    pub label: String,
    pub kind: String,
    pub output_text: String,
    pub saved_at: String,
}
