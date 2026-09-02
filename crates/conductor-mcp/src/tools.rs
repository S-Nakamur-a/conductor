//! stdio 越しに公開する 7 つのレビュー DB ツール。
//!
//! ワイヤ上の契約 — ツール名、引数名、description、返信文の一字一句 — はあえて
//! 変えない。既に世に出ているセッションとスラッシュコマンドがそれに依存する。
//!
//! ハンドラの本体が同期なのは、パイプもクライアントも 1 つずつしかないため。
//! 2 つ目の同時呼び出し元を支えるには先に作り直しが要る。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};

use conductor_core::review_store::{Author, CommentKind, CommentStatus, NewReview, ReviewStore};

use crate::args::{
    CommentIdOnly, CreateComment, GetChangeSummary, GetPendingComments, ReplyToComment,
    SetChangeSummary,
};
use crate::refresh_signal::signal_refresh;
use crate::reply::{
    ensure_not_blank, err_text, line_range, normalize_repo_relative, ok_text, render_thread,
    short_id,
};
use crate::resolve;

/// 1 ブランチ上の未解決な自己レビューコメントがこれを超えると、成功メッセージに
/// 注意書きが添えられる。ハードな上限ではなくソフトなシグナル。
const SELF_REVIEW_SOFT_LIMIT: usize = 5;

/// 1 つのコメントがカバーできる最も広い行範囲。
///
/// 行ごとのコメントキャッシュが範囲内の各行に 1 エントリを実体化するので、
/// でたらめな line_end (モデルが 4_000_000_000 を出力するなど) は次のリフレッシュ
/// で TUI を固まらせる。しかも書き込みは必ず FIFO を突くので、ユーザ操作なしに
/// それが起こる。
const MAX_COMMENT_SPAN: u32 = 10_000;

/// モデル側では何もできない種類のストア障害。ツールレベルのエラーではなく
/// プロトコルエラーとして報告する。
fn db_error(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(format!("database error: {e}"), None)
}

/// create_comment のアンカー検証。エラー文はそのままモデルに返る。
fn validate_anchor(
    file_path: &str,
    line_start: u32,
    line_end: Option<u32>,
) -> Result<String, String> {
    // 行番号は 1 始まり。u32 だけでは 0 を受け入れてしまい、スキーマの minimum
    // では「正の数」を表現できない。0 が保存されると読み戻す側は一律
    // saturating_sub(1) で最初の行にクランプし、理由の分からない 1 行ずれになる。
    if line_start == 0 {
        return Err("line_start must be 1-based (got 0).".to_string());
    }
    if let Some(end) = line_end {
        if end < line_start {
            return Err(format!(
                "Invalid range: line_end ({end}) is before line_start ({line_start})."
            ));
        }
        if end - line_start >= MAX_COMMENT_SPAN {
            return Err(format!(
                "Range too wide: {line_start}-{end} spans more than {MAX_COMMENT_SPAN} lines."
            ));
        }
    }
    normalize_repo_relative(file_path, "file_path")
}

/// ハンドラの背後にある共有状態。
///
/// ストアが mutex の背後にあるのは rusqlite::Connection が Send だが Sync では
/// なく、rmcp がハンドラを共有状態としてトランスポートに渡すため。
struct Inner {
    store: Mutex<ReviewStore>,
    db_path: PathBuf,
    repo_root: PathBuf,
    version: String,
}

#[derive(Clone)]
pub struct McpServer {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(store: ReviewStore, db_path: PathBuf, repo_root: PathBuf, version: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: Mutex::new(store),
                db_path,
                repo_root,
                version: version.to_string(),
            }),
            tool_router: Self::tool_router(),
        }
    }

    /// mutex の汚染は伝播させず回復する。rusqlite::Connection は drop で未完了の
    /// トランザクションを巻き戻すので、書きかけのデータは残らない。
    fn store(&self) -> MutexGuard<'_, ReviewStore> {
        self.inner.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 呼び出しごとに読み直す。セッションが裏でブランチを切り替えることがあり、
    /// git2::Repository は Sync でないので開いたまま持つ利点も無い。
    fn branch(&self) -> Option<String> {
        resolve::branch_at(&self.inner.repo_root)
    }

    fn signal_refresh(&self) {
        if let Some(pipe) = resolve::refresh_pipe_path(&self.inner.db_path) {
            signal_refresh(&pipe);
        }
    }

    fn resolve_comment_id(&self, comment_id: &str) -> Result<Option<String>, ErrorData> {
        self.store().resolve_id_prefix(comment_id).map_err(db_error)
    }
}

#[tool_router(router = tool_router)]
impl McpServer {
    #[tool(
        description = "List unresolved (pending) review comments. By default, only comments for the current git branch are returned. Set all_branches=true to see comments across all branches. Use get_comment_thread to read full details and replies for a specific comment."
    )]
    async fn get_pending_comments(
        &self,
        Parameters(args): Parameters<GetPendingComments>,
    ) -> Result<CallToolResult, ErrorData> {
        // 明示的な branch は all_branches に優先する。どちらも無いときだけ
        // チェックアウト中のブランチにフォールバックする。
        let effective_branch = match (&args.branch, args.all_branches) {
            (Some(b), _) => Some(b.clone()),
            (None, Some(true)) => None,
            (None, _) => self.branch(),
        };

        let rows = self
            .store()
            .pending_reviews(
                effective_branch.as_deref(),
                args.worktree.as_deref(),
                args.file_path.as_deref(),
            )
            .map_err(db_error)?;

        if rows.is_empty() {
            let note = match &effective_branch {
                Some(b) => format!(" (branch: {b})"),
                None => String::new(),
            };
            return ok_text(format!("No pending comments found{note}."));
        }

        let entries: Vec<String> = rows
            .iter()
            .map(|r| {
                format!(
                    "[{}] {} (id: {})\n  {}",
                    r.kind.as_str().to_uppercase(),
                    line_range(&r.file_path, r.line_start, r.line_end),
                    short_id(&r.id),
                    r.body
                )
            })
            .collect();

        let note = match &effective_branch {
            Some(b) => format!(" on branch \"{b}\""),
            None => " across all branches".to_string(),
        };
        ok_text(format!(
            "{} pending comment(s){note}:\n\n{}",
            rows.len(),
            entries.join("\n\n")
        ))
    }

    #[tool(
        description = "Get full details of a review comment and all its replies. Use the comment ID (or prefix) from get_pending_comments."
    )]
    async fn get_comment_thread(
        &self,
        Parameters(args): Parameters<CommentIdOnly>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = self.resolve_comment_id(&args.comment_id)? else {
            return err_text(format!("Comment not found: {}", args.comment_id));
        };

        let comment = self.store().get_review(&id).map_err(db_error)?;
        let replies = self.store().get_replies(&id).map_err(db_error)?;

        ok_text(render_thread(&comment, &replies))
    }

    #[tool(
        description = "Add a reply to a review comment. Author is automatically set to 'claude'."
    )]
    async fn reply_to_comment(
        &self,
        Parameters(args): Parameters<ReplyToComment>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = self.resolve_comment_id(&args.comment_id)? else {
            return err_text(format!("Comment not found: {}", args.comment_id));
        };

        self.store()
            .add_reply(&id, &args.body, Author::Claude)
            .map_err(db_error)?;
        self.signal_refresh();

        // 返信の id は add_reply の内部で生成され返らないので、付いた先の
        // コメントで識別する。
        ok_text(format!("Reply added to comment {}.", short_id(&id)))
    }

    #[tool(description = "Mark a review comment as resolved.")]
    async fn resolve_comment(
        &self,
        Parameters(args): Parameters<CommentIdOnly>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = self.resolve_comment_id(&args.comment_id)? else {
            return err_text(format!("Comment not found: {}", args.comment_id));
        };

        if let Err(e) = self
            .store()
            .update_review_status(&id, CommentStatus::Resolved)
        {
            return err_text(format!("Comment not found: {id} ({e})"));
        }
        self.signal_refresh();
        ok_text(format!("Comment {} marked as resolved.", short_id(&id)))
    }

    #[tool(
        description = "Leave an inline self-review comment on a file/line range in the current branch's diff. Author is set to 'claude' and it appears inline in the Conductor diff view. High-signal, low-frequency tool: flag only what a human reviewer genuinely needs — tricky logic, deliberate tradeoffs, or spots you are unsure about — not routine changes the diff already makes obvious."
    )]
    async fn create_comment(
        &self,
        Parameters(args): Parameters<CreateComment>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(msg) = ensure_not_blank(&args.body, "body") {
            return err_text(msg);
        }
        let rel_path = match validate_anchor(&args.file_path, args.line_start, args.line_end) {
            Ok(p) => p,
            Err(msg) => return err_text(msg),
        };
        let Some(branch) = self.branch() else {
            return err_text(
                "Cannot determine the current git branch (detached HEAD?); a comment must be attached to a branch.",
            );
        };

        let created = self.store().add_review(NewReview {
            branch: &branch,
            file_path: &rel_path,
            line_start: args.line_start,
            line_end: args.line_end,
            kind: args.kind.map_or(CommentKind::Suggest, Into::into),
            body: &args.body,
            author: Author::Claude,
        });
        let created = match created {
            Ok(c) => c,
            Err(e) => return err_text(format!("Failed to create comment: {e}")),
        };

        // 件数集計の失敗が、今しがた成功したコメントの報告を妨げてはならない。
        let count = self
            .store()
            .pending_reviews(Some(&branch), None, None)
            .map(|rows| rows.iter().filter(|r| r.author == Author::Claude).count())
            .unwrap_or(0);
        self.signal_refresh();

        // 現在の件数をそのまま返すのは、「控えめに使うこと」という静的な指示より
        // 強い抑止になる — 作者が自分の密度を目にできる。
        let nudge = if count > SELF_REVIEW_SOFT_LIMIT {
            " — that's a lot; make sure each one is genuinely high-signal before adding more"
        } else {
            ""
        };
        ok_text(format!(
            "Comment created (id: {}) at {} on branch \"{branch}\". ({count} unresolved self-review comment(s) now on this branch{nudge}.)",
            short_id(&created.id),
            line_range(&rel_path, args.line_start, args.line_end),
        ))
    }

    #[tool(
        description = "Set the branch-level change summary — the 'what & why' of the whole diff (the PR-description counterpart to line-anchored comments). It renders as a fixed, Markdown-formatted banner above the diff in the Conductor Viewer. Write one overview describing the overall intent of the change and the rationale for the files being touched; calling it again replaces the previous summary. Use this for the high-level narrative, and create_comment for specific line-level notes."
    )]
    async fn set_change_summary(
        &self,
        Parameters(args): Parameters<SetChangeSummary>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(msg) = ensure_not_blank(&args.body, "body") {
            return err_text(msg);
        }
        let Some(branch) = self.branch() else {
            return err_text(
                "Cannot determine the current git branch (detached HEAD?); a change summary must be attached to a branch.",
            );
        };

        if let Err(e) = self
            .store()
            .save_change_summary(&branch, &args.body, Author::Claude)
        {
            return err_text(format!("Failed to set change summary: {e}"));
        }
        self.signal_refresh();
        ok_text(format!(
            "Change summary set for branch \"{branch}\" ({} chars). It now shows as a banner above the diff.",
            args.body.chars().count()
        ))
    }

    #[tool(
        description = "Get the branch-level change summary (the 'what & why' overview written by set_change_summary) for the current branch, or a specified branch. Useful for reusing the summary as a PR description body."
    )]
    async fn get_change_summary(
        &self,
        Parameters(args): Parameters<GetChangeSummary>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(target) = args.branch.or_else(|| self.branch()) else {
            return err_text(
                "Cannot determine the current git branch (detached HEAD?); specify a branch explicitly.",
            );
        };

        let summary = self.store().get_change_summary(&target).map_err(db_error)?;

        match summary {
            Some(body) => ok_text(body),
            None => ok_text(format!("No change summary set for branch \"{target}\".")),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("conductor", self.inner.version.clone());
        info.instructions = Some(
            "Conductor's review database: inline comments and change summaries \
             for the branch checked out in this working directory."
                .into(),
        );
        info
    }
}

#[cfg(test)]
mod tests;
