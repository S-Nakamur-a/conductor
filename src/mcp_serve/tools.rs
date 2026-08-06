//! stdio 越しに公開される、8個のレビューデータベースツール。
//!
//! これらは、かつて別のマーケットプレイスプラグインパッケージとして配布されて
//! いた Node サーバの移植版である。ワイヤ上の契約 — 引数名、デフォルト値、
//! 返信文の一字一句 — はあえて変えていない。既に世に出ているセッションや
//! スラッシュコマンドがそれに依存しているため。この契約を記録しているのは
//! docs/spec-s6-mcp-tools.md であり、両者が食い違う場合はそちらが優先する。
//!
//! 書き込みを行うツールは全て TUI のリフレッシュ用 FIFO も突く。これにより、
//! レビュー担当者が何もしなくてもコメントが Explorer に表示される。
//!
//! 以下のハンドラの本体は全て完全に同期的である。async fn なのは rmcp の
//! #[tool] トレイトがそれを要求するからにすぎない。シングルスレッド
//! ランタイムを SQLite でブロックしても安全なのは、まさにパイプもクライアント
//! も1つずつしかないからである — 2つ目の同時呼び出し元をサポートするには、
//! 先にこれを作り直す必要がある（例えば spawn_blocking を使うなど）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};

use super::args::{
    CommentIdOnly, CreateComment, GetChangeSummary, GetPendingComments, ReplyToComment,
    SetChangeSummary,
};
use super::reply::{
    ensure_not_blank, err_text, line_range, normalize_repo_relative, ok_text,
    render_thread, short_id,
};
use super::resolve;
use crate::review_store::{Author, CommentKind, CommentStatus, ReviewStore};

/// 1ブランチ上の未解決な自己レビューコメントがこの件数を超えると、成功
/// メッセージに軽い注意書きが添えられ、作者が密度の高まりに気づけるように
/// なる。ハードな上限ではなく、あくまでソフトなシグナルである。
const SELF_REVIEW_SOFT_LIMIT: usize = 5;

/// 1つのコメントがカバーできる最も広い行範囲。
///
/// レビューコメントはファイル全体ではなくハンクに紐づく。この上限がある
/// のは、review_state の行ごとのコメントキャッシュが範囲内の各行に1エント
/// リを実体化するためで、でたらめな line_end（モデルが 4_000_000_000 を
/// 出力するなど）は次のリフレッシュで TUI を固まらせてしまう — しかも
/// 全ての書き込みがリフレッシュ FIFO を突くので、ユーザ操作を介さずに
/// それが起こる。
const MAX_COMMENT_SPAN: u32 = 10_000;

/// モデル側では何もできない種類のストア障害: ツールレベルのエラーではなく
/// プロトコルエラーとして報告する。
fn db_error(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(format!("database error: {e}"), None)
}

// サーバ

/// ツールハンドラの背後にある共有状態。
///
/// ストアが mutex の背後にあるのは、rusqlite::Connection が Send だが
/// Sync ではなく、rmcp がハンドラを共有状態としてトランスポートに渡すため。
/// パイプもクライアントも1つしかないので、実際に競合することはない。
struct Inner {
    store: Mutex<ReviewStore>,
    db_path: PathBuf,
}

#[derive(Clone)]
pub struct McpServer {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(store: ReviewStore, db_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: Mutex::new(store),
                db_path,
            }),
            tool_router: Self::tool_router(),
        }
    }

    /// レビューストア。mutex の汚染を伝播させるのではなく回復する。panic が
    /// 起こり得るのは文の途中またはトランザクションの途中で mutex が
    /// 汚染される場合だけであり、rusqlite::Connection は drop 時に未完了
    /// のトランザクションをロールバックするので、汚染されたロックの背後の
    /// データが書きかけのまま残ることはなく、into_inner() での回復は安全
    /// である。
    fn store(&self) -> MutexGuard<'_, ReviewStore> {
        self.inner.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// サーバの作業ディレクトリがチェックアウトしているブランチ。
    ///
    /// 起動時にキャッシュせず呼び出しごとに読み直す: セッションは裏で
    /// ブランチを切り替えることがあり得るし、git2::Repository は Sync
    /// ではないので、開いたまま保持しておく利点も無い。
    fn branch(&self) -> Option<String> {
        let repo = resolve::discover_repo().ok()?;
        resolve::current_branch(&repo)
    }

    /// TUI にレビューデータの再読み込みを促す。設計上ベストエフォートである
    /// — FIFO の読み手がいないのは単に conductor が動いていないだけである。
    fn signal_refresh(&self) {
        if let Some(pipe) = resolve::refresh_pipe_path(&self.inner.db_path) {
            crate::refresh_pipe::signal_refresh(&pipe);
        }
    }

    /// 完全な id またはプレフィックスを完全な id に解決する。見つからなければ
    /// そう報告する。
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
        // 明示的な branch は all_branches より優先される。どちらも
        // 与えられていない場合にのみ、チェックアウト中のブランチにフォール
        // バックする。
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

        // Node サーバはここで新しい返信自身の id を報告していたが、こちらの
        // 実装では id は add_reply の内部で生成され呼び出し元には返らない
        // ため、返信はそれが付いたコメントで識別する。
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
        // 行番号は1始まり。u32 だけでは 0 を受け入れてしまい、スキーマの
        // minimum では「正の数」として表現できない — そして 0 が保存されると、
        // 読み戻すあらゆる箇所で黙って最初の行にクランプされてしまい
        // （全ての利用箇所が saturating_sub(1) を使っているため）、コメントが
        // 理由も分からないまま1行ずれた位置に付いてしまう。
        if args.line_start == 0 {
            return err_text("line_start must be 1-based (got 0).");
        }
        if let Err(msg) = ensure_not_blank(&args.body, "body") {
            return err_text(msg);
        }
        if let Some(end) = args.line_end
            && end < args.line_start
        {
            return err_text(format!(
                "Invalid range: line_end ({end}) is before line_start ({}).",
                args.line_start
            ));
        }
        if let Some(end) = args.line_end
            && end - args.line_start >= MAX_COMMENT_SPAN
        {
            return err_text(format!(
                "Range too wide: {}-{end} spans more than {MAX_COMMENT_SPAN} lines.",
                args.line_start
            ));
        }
        let rel_path = match normalize_repo_relative(&args.file_path, "file_path") {
            Ok(p) => p,
            Err(msg) => return err_text(msg),
        };

        let Some(branch) = self.branch() else {
            return err_text(
                "Cannot determine the current git branch (detached HEAD?); a comment must be attached to a branch.",
            );
        };

        let kind = args.kind.map_or(CommentKind::Suggest, Into::into);

        // worktree と branch のどちらもブランチ名を持つ: スキーマ v4 には
        // 両者が一致することを強制する CHECK があり、commit_ref はデフォルト
        // で 'HEAD' になる。
        let created = self.store().add_review(
            &branch,
            &rel_path,
            args.line_start,
            args.line_end,
            kind,
            &args.body,
            "HEAD",
            Author::Claude,
            Some(&branch),
        );
        let created = match created {
            Ok(c) => c,
            Err(e) => return err_text(format!("Failed to create comment: {e}")),
        };

        // ベストエフォート: 件数集計の失敗が、今しがた成功裏に作成された
        // コメントの報告を妨げてはならないので、検索エラーは呼び出し全体を
        // 失敗させるのではなく黙って 0 として扱う。
        let count = self
            .store()
            .pending_reviews(Some(&branch), None, None)
            .map(|rows| rows.iter().filter(|r| r.author == Author::Claude).count())
            .unwrap_or(0);
        self.signal_refresh();

        // 現在の件数をそのまま返すのは、「控えめに使うこと」という静的な指示
        // よりも強い抑止力になる — 作者は自分自身の密度を目にできる。
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
        info.server_info = Implementation::new("conductor", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(
            "Conductor's review database: inline comments and change summaries \
             for the branch checked out in this working directory."
                .into(),
        );
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    // tools/list

    #[test]
    fn tool_router_lists_exactly_the_seven_tools() {
        let tools = McpServer::tool_router().list_all();
        let names: std::collections::BTreeSet<&str> =
            tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "get_pending_comments",
                "get_comment_thread",
                "reply_to_comment",
                "resolve_comment",
                "create_comment",
                "set_change_summary",
                "get_change_summary",
            ]
            .into_iter()
            .collect()
        );
    }

    // ハンドラ
    //
    // ここでカバーしているのは self.branch() を呼ばない4つのツールだけ
    // である — 残りは cwd がチェックアウトしている git ブランチを基準に
    // 動くが、ユニットテストにはそれを安定して制御する方法が無い。

    /// tokio は macros フィーチャ無しで取り込まれているので（mcp_serve/mod.rs
    /// 参照）、#[tokio::test] は使えない。代わりにランタイムを手で組み立てる。
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// 新規の tempdir データベースを背後に持つ McpServer。tempdir には
    /// refresh FIFO が無いので、signal_refresh の libc::open は黙って失敗
    /// する — スタブは不要。
    fn test_server() -> (McpServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("conductor.db");
        let store = ReviewStore::open(&db_path).unwrap();
        (McpServer::new(store, db_path), dir)
    }

    fn text_of(result: &CallToolResult) -> &str {
        &result.content[0].as_text().unwrap().text
    }

    #[test]
    fn resolve_comment_reports_not_found() {
        let (server, _dir) = test_server();
        let result = block_on(server.resolve_comment(Parameters(CommentIdOnly {
            comment_id: "deadbeef".into(),
        })))
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("not found"));
    }

    #[test]
    fn resolve_comment_marks_it_resolved() {
        let (server, _dir) = test_server();
        let id = server
            .store()
            .add_review(
                "feat/x",
                "src/foo.rs",
                3,
                None,
                CommentKind::Suggest,
                "note",
                "HEAD",
                Author::User,
                Some("feat/x"),
            )
            .unwrap()
            .id;

        let result = block_on(server.resolve_comment(Parameters(CommentIdOnly {
            comment_id: id.clone(),
        })))
        .unwrap();
        assert_eq!(result.is_error, Some(false));

        assert_eq!(
            server.store().get_review(&id).unwrap().status,
            CommentStatus::Resolved
        );
    }

    #[test]
    fn get_comment_thread_reports_not_found() {
        let (server, _dir) = test_server();
        let result = block_on(server.get_comment_thread(Parameters(CommentIdOnly {
            comment_id: "deadbeef".into(),
        })))
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("Comment not found: deadbeef"));
    }

    #[test]
    fn reply_to_comment_reports_not_found() {
        let (server, _dir) = test_server();
        let result = block_on(server.reply_to_comment(Parameters(ReplyToComment {
            comment_id: "deadbeef".into(),
            body: "hi".into(),
        })))
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("not found"));
    }

    #[test]
    fn reply_to_comment_stores_a_claude_authored_reply() {
        let (server, _dir) = test_server();
        let id = server
            .store()
            .add_review(
                "feat/x",
                "src/foo.rs",
                3,
                None,
                CommentKind::Suggest,
                "note",
                "HEAD",
                Author::User,
                Some("feat/x"),
            )
            .unwrap()
            .id;

        let result = block_on(server.reply_to_comment(Parameters(ReplyToComment {
            comment_id: id.clone(),
            body: "Looks good.".into(),
        })))
        .unwrap();
        assert_eq!(result.is_error, Some(false));

        let replies = server.store().get_replies(&id).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].author, Author::Claude);
        assert_eq!(replies[0].body, "Looks good.");
    }
}
