use std::path::Path;
use std::thread;
use std::time::Duration;

use rusqlite::{Connection, params};

use super::*;

fn memory_store() -> ReviewStore {
    ReviewStore::open(Path::new(":memory:")).expect("open in-memory DB")
}

fn comment<'a>(branch: &'a str, file: &'a str, line: u32) -> NewReview<'a> {
    NewReview {
        branch,
        file_path: file,
        line_start: line,
        line_end: None,
        kind: CommentKind::Suggest,
        body: "note",
        author: Author::User,
    }
}

fn pragma<T: rusqlite::types::FromSql>(store: &ReviewStore, name: &str) -> T {
    store
        .conn
        .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
        .unwrap()
}

fn table_exists(store: &ReviewStore, table: &str) -> bool {
    let found: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )
        .unwrap();
    found == 1
}

fn insert_legacy_review(store: &ReviewStore, id: &str, worktree: &str, branch: Option<&str>) {
    store
        .conn
        .execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, branch)
             VALUES (?1, ?2, 'src/main.rs', 1, 'suggest', 'note', ?3)",
            params![id, worktree, branch],
        )
        .unwrap();
}

#[test]
fn db_pathはディレクトリを作る() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    assert_eq!(path, dir.path().join(".conductor").join("conductor.db"));
    assert!(dir.path().join(".conductor").is_dir());
}

#[test]
fn openはwalとbusy_timeoutを設定する() {
    let dir = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(&dir.path().join("conductor.db")).unwrap();
    assert_eq!(pragma::<String>(&store, "journal_mode"), "wal");
    assert_eq!(pragma::<i64>(&store, "busy_timeout"), 5000);
    assert_eq!(pragma::<i64>(&store, "foreign_keys"), 1);
}

#[test]
fn walに切り替えられなくてもopenは成功する() {
    let store = memory_store();
    assert_ne!(pragma::<String>(&store, "journal_mode"), "wal");
}

#[test]
fn 読み手が居てもwalへの切替を待って成功する() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conductor.db");

    let holder = Connection::open(&path).unwrap();
    holder
        .execute_batch("CREATE TABLE t (x); BEGIN; SELECT count(*) FROM t;")
        .unwrap();

    let opener = thread::spawn({
        let path = path.clone();
        move || ReviewStore::open(&path).unwrap()
    });
    thread::sleep(Duration::from_millis(300));
    holder.execute_batch("COMMIT;").unwrap();

    let store = opener.join().unwrap();
    assert_eq!(pragma::<String>(&store, "journal_mode"), "wal");
}

#[test]
fn commit_refは既定でheadになる() {
    let store = memory_store();
    insert_legacy_review(&store, "r1", "feat/x", Some("feat/x"));
    let commit_ref: String = store
        .conn
        .query_row("SELECT commit_ref FROM reviews WHERE id = 'r1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(commit_ref, "HEAD");
}

#[test]
fn worktreeとbranchの組み合わせはcheckで縛られる() {
    for (worktree, branch, accepted) in [
        ("feat/x", Some("feat/x"), true),
        ("wt1", None, true),
        ("feat/x", Some("feat/y"), false),
    ] {
        let store = memory_store();
        let result = store.conn.execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, branch)
             VALUES ('r1', ?1, 'src/main.rs', 1, 'suggest', 'note', ?2)",
            params![worktree, branch],
        );
        assert_eq!(
            result.is_ok(),
            accepted,
            "worktree={worktree} branch={branch:?}"
        );
        if accepted {
            let rows = store.reviews_for_worktree(worktree).unwrap();
            assert_eq!(rows[0].branch.as_deref(), branch);
        }
    }
}

#[test]
fn 既存のv5から最後まで移行できる() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conductor.db");
    {
        let store = ReviewStore::open(&path).unwrap();
        store
            .add_review(NewReview {
                body: "predates the v6 migration",
                ..comment("feat/x", "src/main.rs", 1)
            })
            .unwrap();
        store
            .conn
            .execute_batch(
                "
                DROP TABLE pr_review_meta;
                ALTER TABLE reviews DROP COLUMN published_at;
                CREATE TABLE daily_stats (date TEXT PRIMARY KEY);
                CREATE TABLE session_stats (id TEXT PRIMARY KEY);
                PRAGMA user_version = 5;
                ",
            )
            .unwrap();
    }

    let store = ReviewStore::open(&path).unwrap();
    assert_eq!(pragma::<i32>(&store, "user_version"), 9);

    let reviews = store.reviews_for_worktree("feat/x").unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].body, "predates the v6 migration");

    assert!(store.get_pr_review_meta("feat/x").unwrap().is_none());
    assert_eq!(store.unpublished_reviews("feat/x").unwrap().len(), 1);

    for table in [
        "walkthroughs",
        "walkthrough_steps",
        "daily_stats",
        "session_stats",
    ] {
        assert!(!table_exists(&store, table), "{table} should be gone");
    }
}

#[test]
fn v9は統計テーブルを落とす() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conductor.db");
    {
        let store = ReviewStore::open(&path).unwrap();
        store
            .conn
            .execute_batch(
                "
                CREATE TABLE daily_stats (date TEXT PRIMARY KEY);
                CREATE TABLE session_stats (id TEXT PRIMARY KEY);
                PRAGMA user_version = 8;
                ",
            )
            .unwrap();
    }
    let store = ReviewStore::open(&path).unwrap();
    assert_eq!(pragma::<i32>(&store, "user_version"), 9);
    assert!(!table_exists(&store, "daily_stats"));
    assert!(!table_exists(&store, "session_stats"));
    assert!(table_exists(&store, "session_history"));
}

#[test]
fn コメントを足して取り出す() {
    let store = memory_store();
    let review = store
        .add_review(NewReview {
            line_end: Some(20),
            kind: CommentKind::Question,
            body: "use guard clause",
            author: Author::Claude,
            ..comment("feat/x", "src/main.rs", 10)
        })
        .unwrap();

    assert_eq!(review.worktree, "feat/x");
    assert_eq!(review.branch.as_deref(), Some("feat/x"));
    assert_eq!(review.file_path, "src/main.rs");
    assert_eq!((review.line_start, review.line_end), (10, Some(20)));
    assert_eq!(review.kind, CommentKind::Question);
    assert_eq!(review.body, "use guard clause");
    assert_eq!(review.status, CommentStatus::Pending);
    assert_eq!(review.author, Author::Claude);

    let reviews = store.reviews_for_worktree("feat/x").unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].id, review.id);
    assert_eq!(store.get_review(&review.id).unwrap().id, review.id);
}

#[test]
fn 本文と状態を編集する() {
    let store = memory_store();
    let review = store.add_review(comment("wt1", "src/app.rs", 5)).unwrap();

    store.update_review_body(&review.id, "edited").unwrap();
    store
        .update_review_status(&review.id, CommentStatus::Resolved)
        .unwrap();

    let after = store.get_review(&review.id).unwrap();
    assert_eq!(after.body, "edited");
    assert_eq!(after.status, CommentStatus::Resolved);
}

#[test]
fn 無いidへの変更はエラーになる() {
    let store = memory_store();
    assert!(store.update_review_body("nope", "x").is_err());
    assert!(
        store
            .update_review_status("nope", CommentStatus::Resolved)
            .is_err()
    );
    assert!(store.delete_review("nope").is_err());
    assert!(store.update_reply_body("nope", "x").is_err());
    assert!(store.delete_reply("nope").is_err());
    assert!(store.delete_template("nope").is_err());
}

#[test]
fn 投稿済みの印は未投稿の一覧から外す() {
    let store = memory_store();
    let r1 = store
        .add_review(comment("feat/x", "src/main.rs", 1))
        .unwrap();
    let r2 = store
        .add_review(comment("feat/x", "src/lib.rs", 2))
        .unwrap();
    assert_eq!(store.unpublished_reviews("feat/x").unwrap().len(), 2);

    store
        .mark_published(std::slice::from_ref(&r1.id), "2026-07-05T00:00:00Z")
        .unwrap();

    let unpublished = store.unpublished_reviews("feat/x").unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(unpublished[0].id, r2.id);
}

#[test]
fn 未解決の一覧の絞り込み() {
    let store = memory_store();
    let open_a = store.add_review(comment("feat/x", "src/a.rs", 1)).unwrap();
    let open_b = store.add_review(comment("feat/x", "src/b.rs", 2)).unwrap();
    let resolved = store.add_review(comment("feat/x", "src/c.rs", 3)).unwrap();
    store
        .update_review_status(&resolved.id, CommentStatus::Resolved)
        .unwrap();
    insert_legacy_review(&store, "legacy", "feat/x", None);
    insert_legacy_review(&store, "other", "feat/y", Some("feat/y"));

    let cases = [
        (
            None,
            None,
            None,
            vec![open_a.id.as_str(), open_b.id.as_str(), "legacy", "other"],
        ),
        (
            Some("feat/x"),
            None,
            None,
            vec![open_a.id.as_str(), open_b.id.as_str(), "legacy"],
        ),
        (None, Some("feat/y"), None, vec!["other"]),
        (None, None, Some("src/a.rs"), vec![open_a.id.as_str()]),
    ];
    for (branch, worktree, file, expected) in cases {
        let rows = store.pending_reviews(branch, worktree, file).unwrap();
        let mut ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let mut expected = expected;
        ids.sort();
        expected.sort();
        assert_eq!(
            ids, expected,
            "branch={branch:?} worktree={worktree:?} file={file:?}"
        );
    }
}

#[test]
fn idのプレフィックス解決() {
    let store = memory_store();
    let review = store.add_review(comment("wt1", "src/a.rs", 1)).unwrap();

    for (prefix, expected, why) in [
        (review.id.as_str(), Some(review.id.as_str()), "完全な id"),
        (
            &review.id[..MIN_ID_PREFIX_LEN],
            Some(review.id.as_str()),
            "公表している長さちょうど",
        ),
        ("deadbeef", None, "当たらない"),
        ("%", None, "LIKE の任意長ワイルドカード"),
        ("_", None, "LIKE の 1 文字ワイルドカード"),
        ("", None, "空文字"),
        ("xyz", None, "ワイルドカードを含まない不正文字"),
    ] {
        assert_eq!(
            store.resolve_id_prefix(prefix).unwrap().as_deref(),
            expected,
            "{why}"
        );
    }
    for len in 1..MIN_ID_PREFIX_LEN {
        assert_eq!(
            store.resolve_id_prefix(&review.id[..len]).unwrap(),
            None,
            "{len} 文字は実在する id の頭でも拒む"
        );
    }
}

#[test]
fn 複数当たっても決定的に1件を返す() {
    let store = memory_store();
    // rowid 順と id 順が一致しないよう、id の降順で挿入する。
    for id in [
        "aaaaaaaa-2222-0000-0000-000000000000",
        "aaaaaaaa-1111-0000-0000-000000000000",
    ] {
        insert_legacy_review(&store, id, "wt1", None);
    }
    assert_eq!(
        store.resolve_id_prefix("aaaaaaaa").unwrap().as_deref(),
        Some("aaaaaaaa-1111-0000-0000-000000000000")
    );
}

#[test]
fn 返信を足して数える() {
    let store = memory_store();
    let review = store.add_review(comment("wt1", "src/main.rs", 42)).unwrap();
    assert!(store.get_replies(&review.id).unwrap().is_empty());
    assert!(store.reply_counts_for_worktree("wt1").unwrap().is_empty());

    store
        .add_reply(&review.id, "I'll fix it", Author::User)
        .unwrap();
    store
        .add_reply(&review.id, "Thanks!", Author::Claude)
        .unwrap();

    let replies = store.get_replies(&review.id).unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(
        (replies[0].body.as_str(), replies[0].author),
        ("I'll fix it", Author::User)
    );
    assert_eq!(
        (replies[1].body.as_str(), replies[1].author),
        ("Thanks!", Author::Claude)
    );

    let counts = store.reply_counts_for_worktree("wt1").unwrap();
    assert_eq!(counts.get(&review.id), Some(&2));
    assert!(store.reply_counts_for_worktree("wt2").unwrap().is_empty());
}

#[test]
fn 親を消すと返信も消える() {
    let store = memory_store();
    let review = store.add_review(comment("wt1", "src/app.rs", 10)).unwrap();
    store
        .add_reply(&review.id, "because reasons", Author::Claude)
        .unwrap();

    store.delete_review(&review.id).unwrap();
    assert!(store.get_replies(&review.id).unwrap().is_empty());
}

#[test]
fn 返信の削除と編集はその返信だけに効く() {
    let store = memory_store();
    let review = store.add_review(comment("wt1", "src/app.rs", 10)).unwrap();
    store.add_reply(&review.id, "typo", Author::Claude).unwrap();
    store.add_reply(&review.id, "second", Author::User).unwrap();
    let replies = store.get_replies(&review.id).unwrap();

    store.update_reply_body(&replies[0].id, "fixed").unwrap();
    assert_eq!(store.get_replies(&review.id).unwrap()[0].body, "fixed");

    store.delete_reply(&replies[0].id).unwrap();
    let after = store.get_replies(&review.id).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].body, "second");
    assert!(store.get_review(&review.id).is_ok());
}

#[test]
fn セッション履歴の保存と一覧と検索() {
    let store = memory_store();
    assert!(store.list_session_history(50).unwrap().is_empty());

    for (session, worktree, label, kind, output) in [
        ("sess-1", "wt1", "CC:1", "claude_code", "Hello world output"),
        ("sess-2", "wt1", "SH:1", "shell", "ls -la\ntotal 42"),
        (
            "sess-3",
            "wt2",
            "CC:2",
            "claude_code",
            "Error: file not found",
        ),
    ] {
        store
            .save_session_history(session, worktree, label, kind, output)
            .unwrap();
    }

    let history = store.list_session_history(50).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(
        (
            history[0].label.as_str(),
            history[0].worktree.as_str(),
            history[0].kind.as_str()
        ),
        ("CC:2", "wt2", "claude_code")
    );
    assert_eq!(store.list_session_history(2).unwrap().len(), 2);

    for (query, expected_labels) in [
        ("Error", vec!["CC:2"]),
        ("SH:1", vec!["SH:1"]),
        ("nonexistent", vec![]),
    ] {
        let labels: Vec<String> = store
            .search_session_history(query)
            .unwrap()
            .into_iter()
            .map(|h| h.label)
            .collect();
        assert_eq!(labels, expected_labels, "query={query}");
    }
}

#[test]
fn ビュー状態の保存と取得() {
    let store = memory_store();
    assert_eq!(store.get_view_state("feat/x").unwrap(), None);

    for (file, line) in [
        (Some("src/main.rs"), 42),
        (Some("src/app/mod.rs"), 7),
        (None, 0),
    ] {
        store.save_view_state("feat/x", file, line).unwrap();
        assert_eq!(
            store.get_view_state("feat/x").unwrap(),
            Some((file.map(str::to_string), line))
        );
    }
}

#[test]
fn 選択中worktreeの保存と取得() {
    let store = memory_store();
    assert_eq!(store.get_selected_worktree().unwrap(), None);

    for branch in ["feat/a", "feat/b"] {
        store.set_selected_worktree(branch).unwrap();
        assert_eq!(
            store.get_selected_worktree().unwrap().as_deref(),
            Some(branch)
        );
    }
}

#[test]
fn ベースブランチと子ブランチ() {
    let store = memory_store();
    assert_eq!(store.get_worktree_base_branch("feat/a").unwrap(), None);
    assert!(store.get_worktree_children("main").unwrap().is_empty());

    store.save_worktree_base_branch("feat/a", "main").unwrap();
    store.save_worktree_base_branch("feat/b", "main").unwrap();
    store.save_worktree_base_branch("feat/b", "feat/a").unwrap();

    assert_eq!(
        store.get_worktree_base_branch("feat/b").unwrap().as_deref(),
        Some("feat/a")
    );
    assert_eq!(store.get_worktree_children("main").unwrap(), vec!["feat/a"]);
    assert_eq!(
        store.get_worktree_children("feat/a").unwrap(),
        vec!["feat/b"]
    );
}

#[test]
fn 変更サマリの保存と取得と置き換え() {
    let store = memory_store();
    assert_eq!(store.get_change_summary("feat/x").unwrap(), None);

    for (body, author) in [
        ("Refactor the parser for clarity.", Author::Claude),
        ("Updated summary.", Author::User),
    ] {
        store.save_change_summary("feat/x", body, author).unwrap();
        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some(body)
        );
    }
    assert_eq!(store.get_change_summary("feat/y").unwrap(), None);
}

#[test]
fn prレビューのメタ情報のupsertと取得() {
    let store = memory_store();
    assert!(store.get_pr_review_meta("feat/x").unwrap().is_none());

    let first = PrReviewMeta {
        pr_number: Some(42),
        pr_url: Some("https://github.com/o/r/pull/42".into()),
        pr_title: Some("Add feature".into()),
        base_ref: Some("main".into()),
        head_ref: Some("feat/x".into()),
        author: Some("octocat".into()),
    };
    let renamed = PrReviewMeta {
        pr_title: Some("Add feature (renamed)".into()),
        ..first.clone()
    };
    for meta in [&first, &renamed] {
        store.save_pr_review_meta("feat/x", meta).unwrap();
        assert_eq!(
            store.get_pr_review_meta("feat/x").unwrap().as_ref(),
            Some(meta)
        );
    }
    assert!(store.get_pr_review_meta("feat/y").unwrap().is_none());
}
