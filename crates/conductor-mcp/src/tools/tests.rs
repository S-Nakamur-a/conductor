use std::collections::BTreeSet;

use super::*;

/// tokio は macros フィーチャ無しなので #[tokio::test] は使えない。
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(fut)
}

struct Fixture {
    server: McpServer,
    repo: git2::Repository,
    _dir: tempfile::TempDir,
}

impl Fixture {
    /// 1 コミットだけのリポジトリと、その .conductor/ に置いた DB。
    /// refresh FIFO は無いので signal_refresh の open は黙って失敗する。
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut index = repo.index().unwrap();
            let oid = index.write_tree().unwrap();
            let tree = repo.find_tree(oid).unwrap();
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
                .unwrap();
        }

        let db_path = conductor_core::review_store::db_path(dir.path());
        let store = ReviewStore::open(&db_path).unwrap();
        let server = McpServer::new(store, db_path, dir.path().to_path_buf(), "test");
        Self {
            server,
            repo,
            _dir: dir,
        }
    }

    fn branch(&self) -> String {
        self.server.branch().unwrap()
    }

    fn detach_head(&self) {
        let head = self.repo.head().unwrap().target().unwrap();
        self.repo.set_head_detached(head).unwrap();
    }

    fn seed_comment(&self) -> String {
        self.server
            .store()
            .add_review(NewReview {
                branch: "feat/x",
                file_path: "src/foo.rs",
                line_start: 3,
                line_end: None,
                kind: CommentKind::Suggest,
                body: "note",
                author: Author::User,
            })
            .unwrap()
            .id
    }

    fn create(&self, file_path: &str, line_start: u32, line_end: Option<u32>) -> CallToolResult {
        block_on(self.server.create_comment(Parameters(CreateComment {
            file_path: file_path.into(),
            line_start,
            line_end,
            body: "note".into(),
            kind: None,
        })))
        .unwrap()
    }
}

fn text_of(result: &CallToolResult) -> &str {
    &result.content[0].as_text().unwrap().text
}

#[test]
fn 公開するツールはちょうど7つ() {
    let tools = McpServer::tool_router().list_all();
    let names: BTreeSet<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
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

/// id を取る 3 つのツールは、当たらない id でプロトコルエラーではなく
/// モデルが読めるツールエラーを返す。
#[test]
fn 見つからないidはツールエラーになる() {
    let f = Fixture::new();
    let missing = "deadbeef".to_string();

    let results = [
        block_on(f.server.resolve_comment(Parameters(CommentIdOnly {
            comment_id: missing.clone(),
        }))),
        block_on(f.server.get_comment_thread(Parameters(CommentIdOnly {
            comment_id: missing.clone(),
        }))),
        block_on(f.server.reply_to_comment(Parameters(ReplyToComment {
            comment_id: missing.clone(),
            body: "hi".into(),
        }))),
    ];
    for result in results {
        let result = result.unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("Comment not found: deadbeef"));
    }
}

#[test]
fn 解決するとコメントに印が付く() {
    let f = Fixture::new();
    let id = f.seed_comment();

    let result = block_on(f.server.resolve_comment(Parameters(CommentIdOnly {
        comment_id: id.clone(),
    })))
    .unwrap();

    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        f.server.store().get_review(&id).unwrap().status,
        CommentStatus::Resolved
    );
}

#[test]
fn 返信はclaude名義で保存される() {
    let f = Fixture::new();
    let id = f.seed_comment();

    let result = block_on(f.server.reply_to_comment(Parameters(ReplyToComment {
        comment_id: id.clone(),
        body: "Looks good.".into(),
    })))
    .unwrap();
    assert_eq!(result.is_error, Some(false));

    let replies = f.server.store().get_replies(&id).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].author, Author::Claude);
    assert_eq!(replies[0].body, "Looks good.");
}

/// 0 行目、逆転した範囲、広すぎる範囲、リポジトリ外へ脱出するパスは保存させない。
/// 通ったものは git の綴りに正規化されて返る。
#[test]
fn アンカーの検証() {
    let ok: [(&str, u32, Option<u32>, &str); 4] = [
        ("src/foo.rs", 1, None, "src/foo.rs"),
        ("./src/foo.rs", 1, Some(1), "src/foo.rs"),
        ("src//foo.rs", 2, Some(9), "src/foo.rs"),
        ("src/foo.rs", 1, Some(MAX_COMMENT_SPAN), "src/foo.rs"),
    ];
    for (path, start, end, want) in ok {
        assert_eq!(
            validate_anchor(path, start, end),
            Ok(want.to_string()),
            "{path} {start}-{end:?}"
        );
    }

    let rejected: [(&str, u32, Option<u32>); 5] = [
        ("src/foo.rs", 0, None),
        ("src/foo.rs", 5, Some(4)),
        ("src/foo.rs", 1, Some(MAX_COMMENT_SPAN + 1)),
        ("/etc/passwd", 1, None),
        (".//etc/passwd", 1, None),
    ];
    for (path, start, end) in rejected {
        assert!(
            validate_anchor(path, start, end).is_err(),
            "{path} {start}-{end:?}"
        );
    }
}

/// コメントもサマリもブランチをキーにするので、detached HEAD では書けない。
#[test]
fn detached_headでは書き込みを断る() {
    let f = Fixture::new();
    f.detach_head();

    let created = f.create("src/foo.rs", 1, None);
    assert_eq!(created.is_error, Some(true));
    assert!(text_of(&created).contains("detached HEAD?"));

    let summary = block_on(
        f.server
            .set_change_summary(Parameters(SetChangeSummary { body: "why".into() })),
    )
    .unwrap();
    assert_eq!(summary.is_error, Some(true));
    assert!(text_of(&summary).contains("detached HEAD?"));

    assert!(
        f.server
            .store()
            .pending_reviews(None, None, None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn コメントは正規化したパスと現在のブランチで保存される() {
    let f = Fixture::new();
    let result = f.create("./src//foo.rs", 10, Some(12));
    assert_eq!(result.is_error, Some(false));
    assert!(text_of(&result).contains("at src/foo.rs:10-12"));

    let rows = f.server.store().pending_reviews(None, None, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].file_path, "src/foo.rs");
    assert_eq!(rows[0].branch, Some(f.branch()));
    assert_eq!(rows[0].author, Author::Claude);
}

/// 密度は静的な指示ではなく現在の件数で伝える。境界はソフト上限ちょうどで、
/// そこまでは注意書きが出ない。
#[test]
fn 自己レビューが増えると注意書きが付く() {
    let f = Fixture::new();
    const NUDGE: &str = "that's a lot";

    for n in 1..=SELF_REVIEW_SOFT_LIMIT {
        let result = f.create("src/foo.rs", n as u32, None);
        assert!(!text_of(&result).contains(NUDGE), "{n} 件目");
    }
    let over = f.create("src/foo.rs", SELF_REVIEW_SOFT_LIMIT as u32 + 1, None);
    assert!(text_of(&over).contains(NUDGE));
}
