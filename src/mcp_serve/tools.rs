//! The eight review-database tools exposed over stdio.
//!
//! These are a port of the Node server that used to ship as a separate
//! marketplace plugin package. The wire contract — argument names, defaults,
//! and the exact reply text — is deliberately unchanged, because sessions and
//! slash commands in the wild already depend on it; `docs/spec-s6-mcp-tools.md`
//! records that contract and wins over this file if the two disagree.
//!
//! Every tool that writes also pokes the TUI's refresh FIFO, so a comment shows
//! up in the Explorer without the reviewer having to do anything.
//!
//! Every handler body below is fully synchronous; `async fn` is only there
//! because rmcp's `#[tool]` trait requires it. Blocking the single-threaded
//! runtime on SQLite is safe precisely because there is exactly one pipe and
//! one client — supporting a second concurrent caller would need this
//! reworked (e.g. `spawn_blocking`) first.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};

use super::args::{
    CommentIdOnly, CreateComment, GetChangeSummary, GetPendingComments, ReplyToComment,
    SaveWalkthrough, SetChangeSummary,
};
use super::reply::{
    ensure_not_blank, ensure_repo_relative, err_text, line_range, normalize_repo_relative, ok_text,
    render_thread, short_id,
};
use super::resolve;
use crate::review_store::{Author, CommentKind, CommentStatus, ReviewStore};
use crate::walkthrough::NewWalkthroughStep;

/// Above this many unresolved self-review comments on one branch, the success
/// message adds a nudge so the author notices the density is getting high.
/// A soft signal, not a hard cap.
const SELF_REVIEW_SOFT_LIMIT: usize = 5;

/// Widest line range a single comment may cover.
///
/// A review comment anchors to a hunk, not to a whole file. The cap exists
/// because `review_state`'s per-line comment cache materialises one entry per
/// line in the range, so a nonsense `line_end` (a model emitting 4_000_000_000)
/// freezes the TUI on the next refresh — with no user action involved, since
/// every write pokes the refresh FIFO.
const MAX_COMMENT_SPAN: u32 = 10_000;

/// A store failure the model can do nothing about: reported as a protocol
/// error rather than a tool-level one.
fn db_error(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(format!("database error: {e}"), None)
}

// ── Server ──────────────────────────────────────────────────────────

/// Shared state behind the tool handlers.
///
/// The store is behind a mutex because `rusqlite::Connection` is `Send` but not
/// `Sync`, and rmcp hands the handler to the transport as shared state. There is
/// exactly one client on one pipe, so this never actually contends.
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

    /// The review store, recovering from mutex poisoning rather than
    /// propagating it. A panic could only leave the mutex poisoned mid
    /// statement or mid transaction; `rusqlite::Connection` rolls back an
    /// incomplete transaction on drop, so the data behind a poisoned lock is
    /// never left half-written, and recovering via `into_inner()` is safe.
    fn store(&self) -> MutexGuard<'_, ReviewStore> {
        self.inner.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The branch the server's working directory has checked out.
    ///
    /// Re-read per call rather than cached at startup: a session can switch
    /// branches under us, and `git2::Repository` is not `Sync` so there is
    /// nothing to gain by holding one open.
    fn branch(&self) -> Option<String> {
        let repo = resolve::discover_repo().ok()?;
        resolve::current_branch(&repo)
    }

    /// Nudge the TUI to reload review data. Best-effort by design — no reader
    /// on the FIFO simply means conductor is not running.
    fn signal_refresh(&self) {
        if let Some(pipe) = resolve::refresh_pipe_path(&self.inner.db_path) {
            crate::refresh_pipe::signal_refresh(&pipe);
        }
    }

    /// Resolve a full id or a prefix to a full id, or report it as missing.
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
        // Explicit `branch` wins over `all_branches`; only when neither is
        // given do we fall back to the checked-out branch.
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

        // The Node server reported the new reply's own id here; ours is
        // generated inside `add_reply` and not handed back, so the reply is
        // identified by the comment it landed on.
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
        // Line numbers are 1-based. `u32` alone would accept 0, which the
        // schema's `minimum` cannot express as "positive" — and a 0 would be
        // stored and then silently clamp to the first line everywhere it is
        // read back (every consumer uses `saturating_sub(1)`), so a comment
        // would land one line off with nothing to indicate why.
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

        // `worktree` and `branch` both carry the branch name: schema v4 has a
        // CHECK enforcing they agree, and `commit_ref` defaults to 'HEAD'.
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

        // Best-effort: a failed count must not block reporting the comment
        // that was just created successfully, so a lookup error silently
        // reads as 0 rather than failing the whole call.
        let count = self
            .store()
            .pending_reviews(Some(&branch), None, None)
            .map(|rows| rows.iter().filter(|r| r.author == Author::Claude).count())
            .unwrap_or(0);
        self.signal_refresh();

        // Handing the running count back is a stronger restraint than a static
        // "use sparingly" instruction — the author can see its own density.
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
        description = "Get the branch-level change summary (the 'what & why' overview written by set_change_summary or save_walkthrough) for the current branch, or a specified branch. Useful for reusing the summary as a PR description body."
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

    #[tool(
        description = "Save a completed PR walkthrough for a branch: an ordered set of steps (intent -> core -> ripple -> test) that narrate the change, each anchored to a file/line range. Called once, at the end, from the /conductor-walkthrough command after the exploration is done — replaces any prior walkthrough for the branch and marks it ready for the Conductor Viewer to render."
    )]
    async fn save_walkthrough(
        &self,
        Parameters(args): Parameters<SaveWalkthrough>,
    ) -> Result<CallToolResult, ErrorData> {
        if args.steps.is_empty() {
            return err_text("steps must not be empty.");
        }
        for (value, what) in [
            (&args.branch, "branch"),
            (&args.title, "title"),
            (&args.summary, "summary"),
        ] {
            if let Err(msg) = ensure_not_blank(value, what) {
                return err_text(msg);
            }
        }
        // Validate every step before writing any of them, so a bad step late in
        // the list cannot leave a half-saved walkthrough. `file_path` is also
        // normalised here, and the normalised form is what gets stored: steps
        // are matched against `FileDiff::path` by string equality when the
        // Explorer jumps to one, so a step saved as `./src/a.rs` would validate,
        // save, render — and then report "not in this diff" for a file sitting
        // in the list. `create_comment` has always normalised; this is the same
        // call, on the same helper.
        let mut normalized_paths: Vec<String> = Vec::with_capacity(args.steps.len());
        for step in &args.steps {
            if let Err(msg) = ensure_repo_relative(&step.file_path, "step file_path") {
                return err_text(msg);
            }
            match normalize_repo_relative(&step.file_path, "step file_path") {
                Ok(p) => normalized_paths.push(p),
                Err(msg) => return err_text(msg),
            }
            for (value, what) in [
                (&step.file_path, "step file_path"),
                (&step.title, "step title"),
                (&step.body, "step body"),
            ] {
                if let Err(msg) = ensure_not_blank(value, what) {
                    return err_text(msg);
                }
            }
            if let Some(end) = step.line_end
                && end < 1
            {
                return err_text(format!(
                    "Invalid line_end on step {} ({}): must be 1-based (got {end}).",
                    step.seq, step.file_path
                ));
            }
            // Same 1-based contract as `create_comment`; here the fields are
            // optional, so only a present-but-zero value is wrong.
            if let Some(start) = step.line_start
                && start < 1
            {
                return err_text(format!(
                    "Invalid line_start on step {} ({}): must be 1-based (got {start}).",
                    step.seq, step.file_path
                ));
            }
            if let (Some(start), Some(end)) = (step.line_start, step.line_end)
                && end < start
            {
                return err_text(format!(
                    "Invalid range on step {} ({}): line_end ({end}) is before line_start ({start}).",
                    step.seq, step.file_path
                ));
            }
        }

        let steps: Vec<NewWalkthroughStep> = args
            .steps
            .iter()
            .zip(normalized_paths)
            .map(|(s, file_path)| NewWalkthroughStep {
                file_path,
                line_start: s.line_start,
                line_end: s.line_end,
                kind: s.kind.into(),
                title: s.title.clone(),
                body: s.body.clone(),
            })
            .collect();

        let saved = self
            .store()
            .save_walkthrough(&args.branch, &args.title, &args.summary, &steps);
        let walkthrough_id = match saved {
            Ok(id) => id,
            Err(e) => return err_text(format!("Failed to save walkthrough: {e}")),
        };
        self.signal_refresh();

        ok_text(format!(
            "Walkthrough saved for branch \"{}\" (id: {}, {} step(s)), status=ready.",
            args.branch,
            short_id(&walkthrough_id),
            args.steps.len()
        ))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("conductor", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(
            "Conductor's review database: inline comments, change summaries, and PR walkthroughs \
             for the branch checked out in this working directory."
                .into(),
        );
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_serve::args::{StepKindArg, WalkthroughStep};

    // ── tools/list ──────────────────────────────────────────────────

    #[test]
    fn tool_router_lists_exactly_the_eight_tools() {
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
                "save_walkthrough",
            ]
            .into_iter()
            .collect()
        );
    }

    // ── handlers ────────────────────────────────────────────────────
    //
    // Only the four tools that don't call `self.branch()` are covered here —
    // the rest key off the cwd's checked-out git branch, which a unit test has
    // no stable way to control.

    /// `tokio` is pulled in without the `macros` feature (see
    /// `mcp_serve/mod.rs`), so `#[tokio::test]` isn't available; build the
    /// runtime by hand instead.
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// An `McpServer` backed by a fresh tempdir database. The tempdir has no
    /// refresh FIFO, so `signal_refresh`'s `libc::open` fails silently — no
    /// stub needed.
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
    fn save_walkthrough_rejects_empty_steps() {
        let (server, _dir) = test_server();
        let result = block_on(server.save_walkthrough(Parameters(SaveWalkthrough {
            branch: "feat/x".into(),
            title: "Title".into(),
            summary: "Summary".into(),
            steps: vec![],
        })))
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("steps must not be empty"));
    }

    #[test]
    fn save_walkthrough_rejects_paths_that_escape_the_repo() {
        let (server, _dir) = test_server();
        for bad_path in ["/etc/passwd", "../secret"] {
            let step = WalkthroughStep {
                seq: 0,
                file_path: bad_path.into(),
                line_start: None,
                line_end: None,
                kind: StepKindArg::Core,
                title: "Step".into(),
                body: "Body".into(),
            };
            let result = block_on(server.save_walkthrough(Parameters(SaveWalkthrough {
                branch: "feat/x".into(),
                title: "Title".into(),
                summary: "Summary".into(),
                steps: vec![step],
            })))
            .unwrap();

            assert_eq!(result.is_error, Some(true), "path: {bad_path}");
            let text = text_of(&result);
            assert!(
                text.contains("must be repo-relative") || text.contains("must not escape"),
                "path: {bad_path}, got: {text}"
            );
        }
    }

    #[test]
    fn save_walkthrough_rejects_reversed_range() {
        let (server, _dir) = test_server();
        let step = WalkthroughStep {
            seq: 0,
            file_path: "src/foo.rs".into(),
            line_start: Some(10),
            line_end: Some(5),
            kind: StepKindArg::Core,
            title: "Step".into(),
            body: "Body".into(),
        };
        let result = block_on(server.save_walkthrough(Parameters(SaveWalkthrough {
            branch: "feat/x".into(),
            title: "Title".into(),
            summary: "Summary".into(),
            steps: vec![step],
        })))
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("line_end (5) is before line_start (10)"));
    }

    #[test]
    fn save_walkthrough_success_reports_id_and_persists_all_steps() {
        let (server, _dir) = test_server();
        let steps = vec![
            WalkthroughStep {
                seq: 0,
                file_path: "src/foo.rs".into(),
                line_start: Some(1),
                line_end: Some(3),
                kind: StepKindArg::Intent,
                title: "Why".into(),
                body: "Because.".into(),
            },
            WalkthroughStep {
                seq: 1,
                file_path: "src/bar.rs".into(),
                line_start: None,
                line_end: None,
                kind: StepKindArg::Core,
                title: "What".into(),
                body: "The change.".into(),
            },
        ];
        let result = block_on(server.save_walkthrough(Parameters(SaveWalkthrough {
            branch: "feat/x".into(),
            title: "Title".into(),
            summary: "Summary".into(),
            steps,
        })))
        .unwrap();

        assert_eq!(result.is_error, Some(false));
        let text = text_of(&result).to_string();

        let store = server.store();
        let (walkthrough, saved_steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(saved_steps.len(), 2);
        assert!(text.contains(&format!("(id: {}, 2 step(s))", short_id(&walkthrough.id))));
    }

    /// The regression this fixes: a step whose `file_path` is spelled any way
    /// other than git's own spelling used to be stored verbatim, and the
    /// Explorer — which matches it against `FileDiff::path` by string equality
    /// — then reported the file as not being in the diff. What lands in the
    /// database has to be the canonical spelling.
    #[test]
    fn save_walkthrough_stores_canonical_paths() {
        let (server, _dir) = test_server();
        let steps = ["./src/foo.rs", "src//bar.rs", "  src/baz.rs  "]
            .iter()
            .enumerate()
            .map(|(i, path)| WalkthroughStep {
                seq: i as i64,
                file_path: (*path).into(),
                line_start: None,
                line_end: None,
                kind: StepKindArg::Core,
                title: "Step".into(),
                body: "Body".into(),
            })
            .collect();
        let result = block_on(server.save_walkthrough(Parameters(SaveWalkthrough {
            branch: "feat/x".into(),
            title: "Title".into(),
            summary: "Summary".into(),
            steps,
        })))
        .unwrap();
        assert_eq!(result.is_error, Some(false), "{}", text_of(&result));

        let store = server.store();
        let (_, saved) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(
            saved.iter().map(|s| s.file_path.as_str()).collect::<Vec<_>>(),
            vec!["src/foo.rs", "src/bar.rs", "src/baz.rs"]
        );
    }

    /// A step path that normalises away to nothing anchors to no file at all,
    /// so it is refused rather than saved.
    #[test]
    fn save_walkthrough_rejects_a_path_that_normalises_to_empty() {
        let (server, _dir) = test_server();
        let result = block_on(server.save_walkthrough(Parameters(SaveWalkthrough {
            branch: "feat/x".into(),
            title: "Title".into(),
            summary: "Summary".into(),
            steps: vec![WalkthroughStep {
                seq: 0,
                file_path: "./".into(),
                line_start: None,
                line_end: None,
                kind: StepKindArg::Core,
                title: "Step".into(),
                body: "Body".into(),
            }],
        })))
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("must not be empty"));
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
