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
fn pull_headから取ったprは既にあるworktreeへ通信無しで入り直す() {
    let (_parent, repo_path, wt_dir) = repo_with_existing_pr_dir(true);
    let engine = GitEngine::open(&repo_path).unwrap();

    let path = checkout_pull_head(&engine, 42, "main", None).unwrap();
    assert_eq!(path, wt_dir);
}

/// .git の無い残骸で Ready を返すと、空のレビュー画面を黙って見せることになる。
#[test]
fn 壊れたディレクトリでは手の打てるエラーで落ちる() {
    let (_parent, repo_path, wt_dir) = repo_with_existing_pr_dir(false);
    let engine = GitEngine::open(&repo_path).unwrap();

    let error = checkout_pull_head(&engine, 42, "main", None).unwrap_err();
    assert!(error.to_string().contains(&wt_dir.display().to_string()));
}

/// gh pr view の実出力 (PR #396) の state と isCrossRepository だけを振る。
fn pr_head(state: &str, from_fork: bool) -> PrHead {
    serde_json::from_str(&format!(
        r#"{{"headRefName":"feature/pty-daemon","isCrossRepository":{from_fork},"number":396,"state":"{state}"}}"#
    ))
    .unwrap()
}

#[test]
fn originからheadを取るのは同じリポジトリのopenなprだけ() {
    let cases = [
        ("OPEN", false, "feature/pty-daemon"),
        ("OPEN", true, "pr-396"),
        ("MERGED", false, "pr-396"),
        ("CLOSED", false, "pr-396"),
    ];
    for (state, from_fork, want) in cases {
        assert_eq!(
            pr_head(state, from_fork).local_branch(),
            want,
            "{state} fork={from_fork}"
        );
    }
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
fn upstreamがmainを向いていてもheadブランチはmainへ進めない() {
    let (origin, clone) = clone_with_remote_branch();
    let engine = clone.engine();
    let worktrees = clone.worktrees_dir();
    let (path, _) = checkout_head_branch(&engine, "feature/x", "main", Some(&worktrees)).unwrap();
    let worktree = Tree::open(path);
    worktree
        .repo
        .find_branch("feature/x", git2::BranchType::Local)
        .unwrap()
        .set_upstream(Some("origin/main"))
        .unwrap();
    origin.checkout("main");
    origin
        .file("c.txt", "main moved\n")
        .add("c.txt")
        .commit("main moved on");

    checkout_head_branch(&engine, "feature/x", "main", Some(&worktrees)).unwrap();

    assert_eq!(worktree.tip("feature/x"), origin.tip("feature/x"));
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
/// できる) に対する end-to-end。ネットワークが要るので既定では走らせない。
/// このリポジトリ自身の worktree に触らないよう一時ディレクトリへ clone する。
#[test]
#[ignore]
fn マージ済みのprはpull_headから取り込める() {
    let parent = tempfile::tempdir().unwrap();
    let repo_path = parent.path().join("repo");
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.depth(50);
    git2::build::RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone("https://github.com/S-Nakamur-a/conductor.git", &repo_path)
        .expect("clone should succeed");

    let engine = GitEngine::open(&repo_path).unwrap();
    let path = checkout_pull_head(&engine, 279, "main", None).unwrap();
    assert_eq!(Tree::open(path).head_branch(), "pr-279");
}

#[test]
fn gh_pr_listの出力は1行ずつprのheadになる() {
    let listed: Vec<PrHead> = serde_json::from_str(
        r#"[{"headRefName":"feature/pty-daemon","isCrossRepository":false,"number":396,"state":"MERGED"},{"headRefName":"fix/warm-when-cold","isCrossRepository":false,"number":395,"state":"MERGED"}]"#,
    )
    .unwrap();
    let numbers: Vec<u64> = listed.iter().map(|pr| pr.number).collect();
    assert_eq!(numbers, [396, 395]);
}

fn requested_heads() -> Vec<PrHead> {
    serde_json::from_str(
        r#"[{"headRefName":"a","isCrossRepository":false,"number":1,"state":"OPEN"},
            {"headRefName":"b","isCrossRepository":false,"number":2,"state":"OPEN"},
            {"headRefName":"c","isCrossRepository":false,"number":3,"state":"OPEN"}]"#,
    )
    .unwrap()
}

fn intaken(repo: &Path, number: u64) -> Result<IntakenPr, String> {
    Ok(IntakenPr {
        pr_number: number,
        worktree_path: repo.join(number.to_string()),
        meta: FetchedPr {
            branch: number.to_string(),
            title: String::new(),
            base_ref: String::from("main"),
            head_ref: number.to_string(),
            url: String::new(),
            head_owner_login: None,
        },
        pulled: None,
    })
}

fn opened(main: &str, branches: &[&str]) -> impl Fn(&Path) -> anyhow::Result<RepoWorktrees> {
    let main = PathBuf::from(main);
    let branches: Vec<String> = branches.iter().map(|b| b.to_string()).collect();
    move |_: &Path| {
        Ok(RepoWorktrees {
            main: main.clone(),
            branches: branches.clone(),
        })
    }
}

#[test]
fn worktreeも試した記録も無い最初の1件だけを取り込む() {
    let tried = HashSet::from([(PathBuf::from("/r"), 2)]);
    let round = review_round(
        &[PathBuf::from("/r")],
        &tried,
        opened("/r", &["main", "a"]),
        |_| Ok(requested_heads()),
        intaken,
    );
    let intake = round.intaken.expect("取り込んでいない");
    assert_eq!((intake.repo, intake.number), (PathBuf::from("/r"), 3));
    assert!(round.failures.is_empty());
}

#[test]
fn 取り込むものが無ければ何も呼ばない() {
    let round = review_round(
        &[PathBuf::from("/r")],
        &HashSet::new(),
        opened("/r", &["a", "b", "c"]),
        |_| Ok(requested_heads()),
        |_, _| panic!("取り込もうとした"),
    );
    assert!(round.intaken.is_none());
    assert!(round.failures.is_empty());
}

#[test]
fn 開けないリポジトリとghの失敗は名前を残して次のリポジトリを見る() {
    let repos = [
        PathBuf::from("/missing"),
        PathBuf::from("/unauthed"),
        PathBuf::from("/ok"),
    ];
    let round = review_round(
        &repos,
        &HashSet::new(),
        |path| match path.to_str() {
            Some("/missing") => anyhow::bail!("not a git repository"),
            _ => Ok(RepoWorktrees {
                main: path.to_path_buf(),
                branches: Vec::new(),
            }),
        },
        |main| match main.to_str() {
            Some("/unauthed") => Err(PrIntakeError::Other(String::from("gh auth login"))),
            _ => Ok(requested_heads()),
        },
        intaken,
    );
    assert_eq!(
        round.intaken.expect("取り込んでいない").repo,
        PathBuf::from("/ok")
    );
    assert_eq!(round.failures.len(), 2, "{:?}", round.failures);
    assert!(
        round.failures[0].starts_with("/missing: "),
        "{:?}",
        round.failures
    );
    assert!(
        round.failures[1].starts_with("/unauthed: "),
        "{:?}",
        round.failures
    );
}

#[test]
fn 同じリポジトリを別の綴りで書いても問い合わせは1回() {
    let repo = TestRepo::with_base_commit();
    repo.linked_worktree("a");
    let linked = repo.linked_worktree("b");
    let link = repo.path.parent().unwrap().join("link");
    std::os::unix::fs::symlink(&repo.path, &link).unwrap();
    let asked = std::cell::Cell::new(0);
    let round = review_round(
        &[repo.path.clone(), link, linked.path.clone()],
        &HashSet::new(),
        repo_worktrees,
        |_| {
            asked.set(asked.get() + 1);
            Ok(Vec::new())
        },
        intaken,
    );
    assert_eq!(asked.get(), 1);
    assert!(round.failures.is_empty(), "{:?}", round.failures);
}

#[test]
fn worktreeのあるブランチは取り込み済みとみなす() {
    let repo = TestRepo::with_base_commit();
    repo.linked_worktree("a");
    let found = repo_worktrees(&repo.path).unwrap();
    assert!(
        found.branches.contains(&String::from("a")),
        "{:?}",
        found.branches
    );
    assert!(
        found.branches.contains(&String::from("main")),
        "{:?}",
        found.branches
    );
}

#[test]
fn 読めた依頼一覧は取り込んだ1件より後ろのworktreeありまで数える() {
    let round = review_round(
        &[PathBuf::from("/r")],
        &HashSet::new(),
        opened("/r", &["c"]),
        |_| Ok(requested_heads()),
        intaken,
    );
    assert_eq!(round.intaken.expect("取り込んでいない").number, 1);
    let [queried] = round.queried.as_slice() else {
        panic!("{:?}", round.queried);
    };
    let mut listed = queried.listed.clone();
    listed.sort();
    assert_eq!(listed, [1, 2, 3]);
    assert_eq!(queried.with_worktree, [3]);
}

#[test]
fn 取り込んだrepoより後ろと失敗したrepoは読めた側に入らない() {
    let repos = [
        PathBuf::from("/fail"),
        PathBuf::from("/a"),
        PathBuf::from("/b"),
    ];
    let asked = std::cell::RefCell::new(Vec::new());
    let round = review_round(
        &repos,
        &HashSet::new(),
        |path| {
            Ok(RepoWorktrees {
                main: path.to_path_buf(),
                branches: Vec::new(),
            })
        },
        |main| {
            asked.borrow_mut().push(main.to_path_buf());
            match main.to_str() {
                Some("/fail") => Err(PrIntakeError::Other(String::from("offline"))),
                _ => Ok(requested_heads()),
            }
        },
        intaken,
    );
    let mains: Vec<&Path> = round.queried.iter().map(|q| q.main.as_path()).collect();
    assert_eq!(mains, [Path::new("/a")]);
    assert_eq!(
        *asked.borrow(),
        [PathBuf::from("/fail"), PathBuf::from("/a")]
    );
}
