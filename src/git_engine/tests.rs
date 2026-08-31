//! git_engine モジュールのユニットテストとスモークテスト。

use super::*;
use std::env;
use std::path::Path;

/// スモークテスト: このソースファイル自身を含むリポジトリを開く。
#[test]
fn このリポジトリを開ける() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let _engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
}

/// スモークテスト: worktree を一覧する(少なくとも main が含まれるはず)。
#[test]
fn worktree一覧はmainを含む() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
    let worktrees = engine.list_worktrees().expect("list_worktrees() failed");
    assert!(!worktrees.is_empty(), "expected at least one worktree");
    assert!(
        worktrees.iter().any(|w| w.is_main),
        "expected one worktree to be marked as main"
    );
}

/// git init した直後 (コミットが 1 つも無く HEAD が未生成) のリポジトリでも
/// メインの worktree を 1 件返すこと。
///
/// ここが空を返すと、worktree を引いてからでないとファイルを開けない画面側は
/// まるごと動かなくなる。HEAD を解決できないだけで一覧ごと落とさない、という
/// のが worktree_info_at の設計 (head_oid は Option) なので、それを固定する。
#[test]
fn コミットの無いリポジトリでも一覧が取れる() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let repo_path = tmp.path().join("fresh");
    std::fs::create_dir_all(&repo_path).unwrap();
    Repository::init(&repo_path).expect("init repo");
    std::fs::write(repo_path.join("hello.txt"), "hi\n").unwrap();

    let engine = GitEngine::open(&repo_path).expect("open fresh repo");
    let worktrees = engine.list_worktrees().expect("list_worktrees() failed");

    assert_eq!(
        worktrees.len(),
        1,
        "a freshly initialised repo should still report its main worktree"
    );
    assert!(worktrees[0].is_main);
    assert!(
        worktrees[0].head_oid.is_none(),
        "unborn HEAD has no oid, and that must not fail the listing"
    );
}

/// main_worktree_path() が linked worktree から開いた場合でも正しいパスを
/// 返すことを確認する。
#[test]
fn リンクされたworktreeからmainのパスを引く() {
    use std::fs;

    // 一時的な最小限の git リポジトリと linked worktree を作成する。
    let tmp = tempfile::tempdir().expect("create temp dir");
    let main_repo_path = tmp.path().join("main-repo");
    fs::create_dir_all(&main_repo_path).unwrap();

    // main リポジトリを初期化し、最初のコミットを作成する。
    let repo = Repository::init(&main_repo_path).expect("init repo");
    {
        let mut index = repo.index().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
    }

    // linked worktree を作成する。
    let wt_path = tmp.path().join("linked-wt");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("test-branch", &head, false).unwrap();
    let branch_ref = repo.find_reference("refs/heads/test-branch").unwrap();
    repo.worktree(
        "test-branch",
        &wt_path,
        Some(git2::WorktreeAddOptions::new().reference(Some(&branch_ref))),
    )
    .expect("create linked worktree");

    // linked worktree から開いて main_worktree_path() を確認する。
    let engine = GitEngine::open(&wt_path).expect("open from linked worktree");
    let main_path = engine.main_worktree_path().expect("main_worktree_path()");

    // 比較のため両方のパスを正規化する(一時ディレクトリはシンボリック
    // リンクを使うことがある)。
    let expected = main_repo_path.canonicalize().unwrap();
    let actual = main_path.canonicalize().unwrap();
    assert_eq!(
        actual, expected,
        "main_worktree_path() should return main repo, not linked worktree"
    );
}

/// main_worktree_path() が main リポジトリからでも動作することを確認する。
#[test]
fn mainリポジトリからmainのパスを引く() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
    let main_path = engine.main_worktree_path().expect("main_worktree_path()");
    // main worktree のパスは存在し、.git ディレクトリを含むはず。
    assert!(main_path.exists(), "main worktree path should exist");
    assert!(
        main_path.join(".git").exists(),
        "main worktree should contain .git"
    );
}

#[test]
fn リモートurlはどの綴りでも同じhttpsに正規化する() {
    for url in [
        "git@github.com:owner/repo.git",
        "https://github.com/owner/repo.git",
        "https://github.com/owner/repo",
        "ssh://git@github.com/owner/repo.git",
    ] {
        assert_eq!(
            GitEngine::remote_url_to_https_base(url),
            Some("https://github.com/owner/repo".to_string()),
            "{url}"
        );
    }
}

/// ヘルパー: 一時的な git リポジトリを作成し、その engine を返す。
fn temp_repo_engine() -> (tempfile::TempDir, GitEngine) {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let repo = Repository::init(tmp.path()).expect("init repo");
    {
        let mut index = repo.index().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }
    let engine = GitEngine::open(tmp.path()).expect("open temp repo");
    (tmp, engine)
}

/// grab の状態がセッション ID なしで正しく往復することを確認するテスト。
#[test]
fn grab状態はセッション無しで往復する() {
    let (_tmp, engine) = temp_repo_engine();

    let branch = "test-branch";
    let wt_path = Path::new("/tmp/test-worktree");
    let stash = "test-branch__grab";

    engine
        .save_grab_state(branch, wt_path, stash, None)
        .unwrap();
    let loaded = engine
        .load_grab_state()
        .unwrap()
        .expect("should load state");

    assert_eq!(loaded.0, branch);
    assert_eq!(loaded.1, wt_path);
    assert_eq!(loaded.2, stash);
    assert_eq!(loaded.3, None);

    engine.remove_grab_state().unwrap();
}

/// grab の状態がセッション ID ありで正しく往復することを確認するテスト。
#[test]
fn grab状態はセッション付きで往復する() {
    let (_tmp, engine) = temp_repo_engine();

    let branch = "feature-x";
    let wt_path = Path::new("/tmp/test-worktree-2");
    let stash = "feature-x__grab";
    let session_id = "abc12345-6789-0def-ghij-klmnopqrstuv";

    engine
        .save_grab_state(branch, wt_path, stash, Some(session_id))
        .unwrap();
    let loaded = engine
        .load_grab_state()
        .unwrap()
        .expect("should load state");

    assert_eq!(loaded.0, branch);
    assert_eq!(loaded.1, wt_path);
    assert_eq!(loaded.2, stash);
    assert_eq!(loaded.3, Some(session_id.to_string()));

    engine.remove_grab_state().unwrap();
}

/// 後方互換性のテスト: 3行の wt-grab ファイル(セッション ID なし)を
/// 読み込む。
#[test]
fn 旧形式のgrab状態も読める() {
    let (_tmp, engine) = temp_repo_engine();

    // レガシーな3行のファイルを直接書き込む。
    let grab_file = engine.git_common_dir().unwrap().join("wt-grab");
    std::fs::write(&grab_file, "my-branch\n/tmp/wt\nmy-branch__grab\n").unwrap();

    let loaded = engine
        .load_grab_state()
        .unwrap()
        .expect("should load state");
    assert_eq!(loaded.0, "my-branch");
    assert_eq!(loaded.1, Path::new("/tmp/wt"));
    assert_eq!(loaded.2, "my-branch__grab");
    assert_eq!(loaded.3, None);

    engine.remove_grab_state().unwrap();
}

/// 返り値の origin_repo を使うと、テストから「remote」へ push するコミットや
/// ブランチを足せる。
fn temp_repo_with_origin() -> (tempfile::TempDir, tempfile::TempDir, Repository, GitEngine) {
    let origin_tmp = tempfile::tempdir().expect("create origin temp dir");
    let origin_repo = Repository::init_bare(origin_tmp.path()).expect("init bare origin");
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    {
        // bare リポジトリには index/workdir が無いので、Repository::index()
        // を経由せず直接(空の) tree を構築する。
        let tree_oid = origin_repo.treebuilder(None).unwrap().write().unwrap();
        let tree = origin_repo.find_tree(tree_oid).unwrap();
        origin_repo
            .commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }

    let local_tmp = tempfile::tempdir().expect("create local temp dir");
    let local_repo = Repository::init(local_tmp.path()).expect("init local repo");
    // 新規リポジトリの HEAD は(unborn の)refs/heads/main を指しており、
    // git fetch はこれを "checked out" とみなして fetch を拒否する。
    // main を直接 fetch できるよう、先に HEAD を別の場所へ移しておく。
    local_repo.set_head("refs/heads/scratch").unwrap();
    local_repo
        .remote("origin", &origin_tmp.path().display().to_string())
        .unwrap();
    let engine = GitEngine::open(local_tmp.path()).expect("open local repo");
    engine.fetch_refspec("main:refs/heads/main").unwrap();

    (origin_tmp, local_tmp, origin_repo, engine)
}

/// main の現在の tip を親にする。PR の head が origin に着地した状態の再現に使う。
fn commit_on_branch(repo: &Repository, branch_name: &str, message: &str) {
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    let parent = repo
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    let tree = parent.tree().unwrap();
    repo.commit(
        Some(&format!("refs/heads/{branch_name}")),
        &sig,
        &sig,
        message,
        &tree,
        &[&parent],
    )
    .unwrap();
}

#[test]
fn fetch_refspecはローカルブランチを作る() {
    let (_origin_tmp, _local_tmp, origin_repo, engine) = temp_repo_with_origin();
    commit_on_branch(&origin_repo, "feature", "a PR head");

    engine
        .fetch_refspec("refs/heads/feature:refs/heads/pr-99")
        .unwrap();

    let branch = engine
        .repo
        .find_branch("pr-99", git2::BranchType::Local)
        .expect("pr-99 branch should exist after fetch_refspec");
    assert_eq!(
        branch.get().peel_to_commit().unwrap().message(),
        Some("a PR head")
    );
}

#[test]
fn 知らないrefへのfetch_refspecは失敗を返す() {
    let (_origin_tmp, _local_tmp, _origin_repo, engine) = temp_repo_with_origin();
    let err = engine
        .fetch_refspec("refs/heads/does-not-exist:refs/heads/pr-1")
        .unwrap_err();
    assert!(err.to_string().contains("git fetch"));
}

#[test]
fn 既存ブランチのworktree作成はそのブランチを出す() {
    let (_origin_tmp, _local_tmp, origin_repo, engine) = temp_repo_with_origin();
    commit_on_branch(&origin_repo, "feature", "a PR head");
    engine
        .fetch_refspec("refs/heads/feature:refs/heads/pr-99")
        .unwrap();

    let base_dir = engine.worktrees_base_dir(None).unwrap();
    let wt_dir = base_dir.join("pr-99");
    let created = engine
        .create_worktree_for_existing_branch("pr-99", &wt_dir)
        .unwrap();

    assert_eq!(created, wt_dir);
    assert!(wt_dir.join(".git").exists());
    let checked_out = GitEngine::open(&wt_dir).unwrap();
    assert_eq!(
        checked_out
            .repo
            .head()
            .unwrap()
            .shorthand()
            .unwrap_or_default(),
        "pr-99"
    );
}

#[test]
fn 既にあるディレクトリへのworktree作成は拒む() {
    let (_origin_tmp, _local_tmp, origin_repo, engine) = temp_repo_with_origin();
    commit_on_branch(&origin_repo, "feature", "a PR head");
    engine
        .fetch_refspec("refs/heads/feature:refs/heads/pr-99")
        .unwrap();

    let base_dir = engine.worktrees_base_dir(None).unwrap();
    let wt_dir = base_dir.join("pr-99");
    std::fs::create_dir_all(&wt_dir).unwrap();

    assert!(
        engine
            .create_worktree_for_existing_branch("pr-99", &wt_dir)
            .is_err()
    );
}

#[test]
fn 無いローカルブランチは作られる() {
    // origin は設定済みだが main をまだ fetch していないローカル
    // リポジトリ。"ローカルブランチが無い" 経路を確認するために使う。
    let origin_tmp = tempfile::tempdir().unwrap();
    let origin_repo = Repository::init_bare(origin_tmp.path()).unwrap();
    {
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let tree_oid = origin_repo.treebuilder(None).unwrap().write().unwrap();
        let tree = origin_repo.find_tree(tree_oid).unwrap();
        origin_repo
            .commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }

    let local_tmp = tempfile::tempdir().unwrap();
    let local_repo = Repository::init(local_tmp.path()).unwrap();
    // HEAD を先に main から移す理由は temp_repo_with_origin() を参照。
    local_repo.set_head("refs/heads/scratch").unwrap();
    local_repo
        .remote("origin", &origin_tmp.path().display().to_string())
        .unwrap();
    let engine = GitEngine::open(local_tmp.path()).unwrap();

    assert!(
        engine
            .repo
            .find_branch("main", git2::BranchType::Local)
            .is_err()
    );
    engine.ensure_base_ref_available("main").unwrap();
    assert!(
        engine
            .repo
            .find_branch("main", git2::BranchType::Local)
            .is_ok()
    );
}

#[test]
fn 既にあるローカルブランチは早送りされる() {
    let (origin_tmp, _local_tmp, origin_repo, engine) = temp_repo_with_origin();
    let before = engine
        .repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();

    // ローカルのクローンがすでに main を持った後で、origin の main が
    // さらに先へ進む。
    commit_on_branch(&origin_repo, "main", "a later commit on main");
    let _ = origin_tmp;

    engine.ensure_base_ref_available("main").unwrap();

    let after = engine
        .repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();
    assert_ne!(before, after, "local main should fast-forward");
    assert_eq!(after.to_string().len(), 40);
}

#[test]
fn 分岐したブランチには触らない() {
    let (_origin_tmp, _local_tmp, origin_repo, engine) = temp_repo_with_origin();

    // ローカルのみのコミットで、ローカルの main を origin の main から
    // 分岐させる。
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    let parent = engine
        .repo
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    let tree = parent.tree().unwrap();
    let local_only_oid = engine
        .repo
        .commit(
            Some("refs/heads/main"),
            &sig,
            &sig,
            "local-only commit",
            &tree,
            &[&parent],
        )
        .unwrap();

    // origin も独立してさらに進む — 真の分岐になる。
    commit_on_branch(&origin_repo, "main", "origin-only commit");

    engine.ensure_base_ref_available("main").unwrap();

    let after = engine
        .repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();
    assert_eq!(
        after, local_only_oid,
        "diverged local branch must not be force-updated"
    );
}
