use std::path::PathBuf;

use super::*;
use crate::test_support::{TestRepo, Tree};

/// PR 42 の worktree ディレクトリだけが既にあるリポジトリ。git_marker が true なら
/// 本物の worktree と同じく .git (gitdir を指すファイル) を置く。
fn repo_with_existing_pr_dir(git_marker: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let parent = tempfile::tempdir().unwrap();
    // macOS の /tmp -> /private/tmp のように一時ディレクトリ自体が symlink の
    // 環境でもパスを等値比較できるようにする。
    let parent_path = parent.path().canonicalize().unwrap();
    let repo_path = parent_path.join("repo");
    git2::Repository::init(&repo_path).unwrap();

    let wt_dir = parent_path.join("repo-worktrees").join("pr-42");
    std::fs::create_dir_all(&wt_dir).unwrap();
    if git_marker {
        std::fs::write(wt_dir.join(".git"), "gitdir: /tmp/fake").unwrap();
    }
    (parent, repo_path, wt_dir)
}

#[test]
fn 先頭がダッシュのrefは拒む() {
    assert!(is_suspicious_ref("--upload-pack=evil"));
    assert!(!is_suspicious_ref("main"));
    assert!(!is_suspicious_ref("release/1.0"));
}

#[test]
fn pr番号とurlをパースする() {
    let cases = [
        ("279", Ok(279)),
        ("  42  ", Ok(42)),
        ("https://github.com/S-Nakamur-a/conductor/pull/279", Ok(279)),
        ("https://github.com/o/r/pull/12/files", Ok(12)),
        (
            "not-a-pr",
            Err(PrIntakeError::InvalidInput("not-a-pr".to_string())),
        ),
    ];
    for (input, want) in cases {
        assert_eq!(parse_pr_input(input), want, "{input}");
    }
}

#[test]
fn ghとgitのstderrを手の打てるエラーに直す() {
    let cases = [
        (
            1,
            "To get started with GitHub CLI, please run:  gh auth login",
            PrIntakeError::GhNotAuthenticated,
        ),
        (
            404,
            r#"no pull requests found for branch "x""#,
            PrIntakeError::PrNotFound(404),
        ),
        (
            5,
            "fatal: couldn't find remote ref pull/5/head",
            PrIntakeError::PrNotFound(5),
        ),
        (
            1,
            "fatal: unable to access: Could not resolve host: github.com",
            PrIntakeError::NetworkError,
        ),
        (
            1,
            "something unexpected happened",
            PrIntakeError::Other("something unexpected happened".to_string()),
        ),
    ];
    for (pr, stderr, want) in cases {
        assert_eq!(classify_failure_text(pr, stderr), want, "{stderr}");
    }
}

#[test]
fn エラー文は次の手が分かる形になっている() {
    assert!(
        PrIntakeError::GhNotAuthenticated
            .to_string()
            .contains("gh auth login")
    );
    assert!(PrIntakeError::PrNotFound(9).to_string().contains('9'));
}

#[test]
fn forkのprは既にあるworktreeへ通信無しで入り直す() {
    let (_parent, repo_path, wt_dir) = repo_with_existing_pr_dir(true);
    let engine = GitEngine::open(&repo_path).unwrap();

    let path = checkout_fork_head(&engine, 42, "main", None).unwrap();
    assert_eq!(path, wt_dir);
}

/// .git の無い残骸で Ready を返すと、空のレビュー画面を黙って見せることになる。
#[test]
fn 壊れたディレクトリでは手の打てるエラーで落ちる() {
    let (_parent, repo_path, wt_dir) = repo_with_existing_pr_dir(false);
    let engine = GitEngine::open(&repo_path).unwrap();

    let error = checkout_fork_head(&engine, 42, "main", None).unwrap_err();
    assert!(error.to_string().contains(&wt_dir.display().to_string()));
}

/// origin は feature/x を checkout した状態で返すので、そのまま commit すれば push 相当になる。
fn clone_with_remote_branch() -> (TestRepo, TestRepo) {
    let origin = TestRepo::with_base_commit();
    origin.branch("feature/x");
    let clone = TestRepo::new();
    clone
        .repo
        .remote("origin", &origin.path.display().to_string())
        .unwrap();
    (origin, clone)
}

#[test]
fn 同じリポジトリのprはheadブランチの名前でworktreeになる() {
    let (origin, clone) = clone_with_remote_branch();

    let (path, pulled) = checkout_head_branch(
        &clone.engine(),
        "feature/x",
        "main",
        Some(&clone.worktrees_dir()),
    )
    .unwrap();

    let worktree = Tree::open(path);
    assert_eq!(worktree.head_branch(), "feature/x");
    assert_eq!(worktree.tip("feature/x"), origin.tip("feature/x"));
    assert_eq!(pulled, None);
}

#[test]
fn 既にあるheadブランチのworktreeはoriginの最新まで進める() {
    let (origin, clone) = clone_with_remote_branch();
    let engine = clone.engine();
    let worktrees = clone.worktrees_dir();
    let (first, _) = checkout_head_branch(&engine, "feature/x", "main", Some(&worktrees)).unwrap();
    origin
        .file("b.txt", "pushed\n")
        .add("b.txt")
        .commit("pushed later");

    let (again, pulled) =
        checkout_head_branch(&engine, "feature/x", "main", Some(&worktrees)).unwrap();

    assert_eq!(again.canonicalize().unwrap(), first.canonicalize().unwrap());
    assert_eq!(Tree::open(again).tip("feature/x"), origin.tip("feature/x"));
    assert!(
        pulled
            .as_deref()
            .is_some_and(|m| m.contains("fast-forward")),
        "{pulled:?}"
    );
}

#[test]
fn レビューdbのauthorにはheadリポジトリの所有者が入る() {
    let fetched = FetchedPr {
        branch: "pr-7".to_string(),
        title: "Add feature".to_string(),
        base_ref: "main".to_string(),
        head_ref: "feat/x".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        head_owner_login: Some("octocat".to_string()),
    };
    assert_eq!(
        fetched.review_meta(7),
        PrReviewMeta {
            pr_number: Some(7),
            pr_url: Some("https://github.com/o/r/pull/7".to_string()),
            pr_title: Some("Add feature".to_string()),
            base_ref: Some("main".to_string()),
            head_ref: Some("feat/x".to_string()),
            author: Some("octocat".to_string()),
        }
    );
}

/// 実在の GitHub リモートとマージ済み PR (refs/pull/<N>/head はマージ後も解決
/// できる) に対する fork 経路の end-to-end。ネットワークが要るので既定では
/// 走らせない。このリポジトリ自身の worktree に触らないよう一時ディレクトリへ
/// clone する。
#[test]
#[ignore]
fn 実在するprのheadをforkと同じ経路で取り込む() {
    let parent = tempfile::tempdir().unwrap();
    let repo_path = parent.path().join("repo");
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.depth(50);
    git2::build::RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone("https://github.com/S-Nakamur-a/conductor.git", &repo_path)
        .expect("clone should succeed");

    let engine = GitEngine::open(&repo_path).unwrap();
    let path = checkout_fork_head(&engine, 279, "main", None).unwrap();
    assert_eq!(Tree::open(path).head_branch(), "pr-279");
}
