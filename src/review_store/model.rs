//! Enums and plain data structs shared across the `review_store` submodules.

/// The kind of review comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    Suggest,
    Question,
}

impl CommentKind {
    /// Convert to the string representation stored in the database.
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

/// The author of a review comment or reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Claude,
}

impl Author {
    /// Convert to the string representation stored in the database.
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

/// The resolution status of a review comment.
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

/// A single review comment attached to a file and line range.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReviewComment {
    pub id: String,
    pub worktree: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub kind: CommentKind,
    pub body: String,
    pub status: CommentStatus,
    pub commit_ref: String,
    pub author: Author,
    pub branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A reusable comment template (saved feedback pattern).
#[derive(Debug, Clone)]
pub struct CommentTemplate {
    pub id: String,
    pub name: String,
    pub body: String,
    pub kind: CommentKind,
}

/// A reply to a review comment.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReviewReply {
    pub id: String,
    pub review_id: String,
    pub body: String,
    pub author: Author,
    pub created_at: String,
}

/// Daily activity statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyStats {
    pub reviews_created: i64,
    pub branches_created: i64,
    pub commits_made: i64,
}

/// Summary statistics for the current session.
#[derive(Debug, Clone, Default)]
pub struct SessionStatsSnapshot {
    pub reviews_created: i64,
    pub branches_created: i64,
    pub commits_made: i64,
}

/// Streak information.
#[derive(Debug, Clone)]
pub struct StreakInfo {
    pub consecutive_days: u32,
}

/// PR metadata for a review-mode branch (`pr_review_meta` table) — the facts
/// needed to render the review header and, later, to publish comments back
/// to the right PR. Unlike `worktree_metadata`, every field but `branch` is
/// optional since a review can start from a bare branch name without a PR.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PrReviewMeta {
    pub branch: String,
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
    pub pr_title: Option<String>,
    pub base_ref: Option<String>,
    pub head_ref: Option<String>,
    pub author: Option<String>,
    pub created_at: String,
}

/// A saved session history record.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionHistory {
    pub id: String,
    pub session_id: String,
    pub worktree: String,
    pub label: String,
    pub kind: String,
    pub output_text: String,
    pub saved_at: String,
}
