use std::path::PathBuf;

use super::*;

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

/// gh が入っていなくても、既にある worktree には gh も通信もなしで入り直せる。
#[test]
fn 既にあるworktreeには通信無しで入り直す() {
    let (_parent, repo_path, wt_dir) = repo_with_existing_pr_dir(true);

    match intake_pr(&repo_path, None, "42") {
        PrIntakeOutcome::Ready {
            pr_number,
            worktree_path,
            ..
        } => {
            assert_eq!(pr_number, 42);
            assert_eq!(worktree_path, wt_dir);
        }
        PrIntakeOutcome::Failed { error } => panic!("expected Ready, got Failed: {error}"),
    }
}

/// .git の無い残骸で Ready を返すと、空のレビュー画面を黙って見せることになる。
#[test]
fn 壊れたディレクトリでは手の打てるエラーで落ちる() {
    let (_parent, repo_path, wt_dir) = repo_with_existing_pr_dir(false);

    match intake_pr(&repo_path, None, "42") {
        PrIntakeOutcome::Failed { error } => {
            assert!(error.to_string().contains(&wt_dir.display().to_string()));
        }
        PrIntakeOutcome::Ready { .. } => panic!("expected Failed, got Ready"),
    }
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
/// できる) に対する end-to-end。ネットワークと認証済みの gh が要るので既定では
/// 走らせない。このリポジトリ自身の worktree に触らないよう一時ディレクトリへ
/// clone する。
#[test]
#[ignore]
fn 実在するprに対する取り込み() {
    let parent = tempfile::tempdir().unwrap();
    let repo_path = parent.path().join("repo");
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.depth(50);
    git2::build::RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone("https://github.com/S-Nakamur-a/conductor.git", &repo_path)
        .expect("clone should succeed");

    match intake_pr(&repo_path, None, "279") {
        PrIntakeOutcome::Ready {
            pr_number,
            worktree_path,
            meta,
        } => {
            assert_eq!(pr_number, 279);
            assert!(worktree_path.exists());
            let meta = meta.expect("fresh intake should carry fetched metadata");
            assert_eq!(meta.base_ref, "main");
            assert_eq!(meta.branch, "pr-279");
        }
        PrIntakeOutcome::Failed { error } => panic!("expected Ready, got Failed: {error}"),
    }
}
