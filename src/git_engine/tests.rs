//! Unit and smoke tests for the `git_engine` module.

use super::*;
use std::env;
use std::path::Path;

/// Smoke test: open the repository that contains this very source file.
#[test]
fn open_this_repo() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let _engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
}

/// Smoke test: list worktrees (should include at least the main one).
#[test]
fn list_worktrees_includes_main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
    let worktrees = engine.list_worktrees().expect("list_worktrees() failed");
    assert!(!worktrees.is_empty(), "expected at least one worktree");
    assert!(
        worktrees.iter().any(|w| w.is_main),
        "expected one worktree to be marked as main"
    );
}

/// `git init` した直後 (コミットが 1 つも無く HEAD が未生成) のリポジトリでも
/// メインの worktree を 1 件返すこと。
///
/// ここが空を返すと、worktree を引いてからでないとファイルを開けない画面側は
/// まるごと動かなくなる。HEAD を解決できないだけで一覧ごと落とさない、という
/// のが `worktree_info_at` の設計 (head_oid は Option) なので、それを固定する。
#[test]
fn list_worktrees_handles_repo_without_commits() {
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

/// Verify that `main_worktree_path()` returns the correct path even when
/// opened from a linked worktree.
#[test]
fn main_worktree_path_from_linked_worktree() {
    use std::fs;

    // Create a temporary bare-bones git repo and a linked worktree.
    let tmp = tempfile::tempdir().expect("create temp dir");
    let main_repo_path = tmp.path().join("main-repo");
    fs::create_dir_all(&main_repo_path).unwrap();

    // Init the main repo and create an initial commit.
    let repo = Repository::init(&main_repo_path).expect("init repo");
    {
        let mut index = repo.index().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
    }

    // Create a linked worktree.
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

    // Open from the linked worktree and verify main_worktree_path().
    let engine = GitEngine::open(&wt_path).expect("open from linked worktree");
    let main_path = engine.main_worktree_path().expect("main_worktree_path()");

    // Canonicalize both paths for comparison (temp dirs may use symlinks).
    let expected = main_repo_path.canonicalize().unwrap();
    let actual = main_path.canonicalize().unwrap();
    assert_eq!(
        actual, expected,
        "main_worktree_path() should return main repo, not linked worktree"
    );
}

/// Verify that `main_worktree_path()` works from the main repo too.
#[test]
fn main_worktree_path_from_main_repo() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
    let main_path = engine.main_worktree_path().expect("main_worktree_path()");
    // The main worktree path should exist and contain a .git directory.
    assert!(main_path.exists(), "main worktree path should exist");
    assert!(
        main_path.join(".git").exists(),
        "main worktree should contain .git"
    );
}

#[test]
fn remote_url_to_https_base_ssh() {
    assert_eq!(
        GitEngine::remote_url_to_https_base("git@github.com:owner/repo.git"),
        Some("https://github.com/owner/repo".to_string()),
    );
}

#[test]
fn remote_url_to_https_base_https() {
    assert_eq!(
        GitEngine::remote_url_to_https_base("https://github.com/owner/repo.git"),
        Some("https://github.com/owner/repo".to_string()),
    );
}

#[test]
fn remote_url_to_https_base_no_suffix() {
    assert_eq!(
        GitEngine::remote_url_to_https_base("https://github.com/owner/repo"),
        Some("https://github.com/owner/repo".to_string()),
    );
}

#[test]
fn remote_url_to_https_base_ssh_prefix() {
    assert_eq!(
        GitEngine::remote_url_to_https_base("ssh://git@github.com/owner/repo.git"),
        Some("https://github.com/owner/repo".to_string()),
    );
}

/// Helper: create a temporary git repo and return its engine.
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

/// Test that grab state round-trips correctly without a session ID.
#[test]
fn grab_state_roundtrip_without_session() {
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

/// Test that grab state round-trips correctly with a session ID.
#[test]
fn grab_state_roundtrip_with_session() {
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

/// Test backward compatibility: loading a 3-line wt-grab file (no session ID).
#[test]
fn grab_state_load_legacy_format() {
    let (_tmp, engine) = temp_repo_engine();

    // Write a legacy 3-line file directly.
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

/// Create a bare "origin" repo with a `main` branch, plus a local repo
/// that has it configured as `origin` and has fetched `main`. Returns
/// `(origin_tmp, local_tmp, origin_repo, local_engine)` — `origin_repo`
/// lets a test add more commits/branches to push into the "remote".
fn temp_repo_with_origin() -> (tempfile::TempDir, tempfile::TempDir, Repository, GitEngine) {
    let origin_tmp = tempfile::tempdir().expect("create origin temp dir");
    let origin_repo = Repository::init_bare(origin_tmp.path()).expect("init bare origin");
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    {
        // A bare repo has no index/workdir, so build the (empty) tree
        // directly rather than going through Repository::index().
        let tree_oid = origin_repo.treebuilder(None).unwrap().write().unwrap();
        let tree = origin_repo.find_tree(tree_oid).unwrap();
        origin_repo
            .commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }

    let local_tmp = tempfile::tempdir().expect("create local temp dir");
    let local_repo = Repository::init(local_tmp.path()).expect("init local repo");
    // A fresh repo's HEAD points at (an unborn) refs/heads/main, which
    // git fetch treats as "checked out" and refuses to fetch into.
    // Point HEAD elsewhere first so fetching `main` directly is allowed.
    local_repo.set_head("refs/heads/scratch").unwrap();
    local_repo
        .remote("origin", &origin_tmp.path().display().to_string())
        .unwrap();
    let engine = GitEngine::open(local_tmp.path()).expect("open local repo");
    engine.fetch_refspec("main:refs/heads/main").unwrap();

    (origin_tmp, local_tmp, origin_repo, engine)
}

/// Add a commit to `refs/heads/<branch_name>` in `repo`, parented on
/// `main`'s current tip. Used to simulate a PR head landing on "origin".
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
fn fetch_refspec_creates_local_branch() {
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
fn fetch_refspec_reports_failure_for_unknown_ref() {
    let (_origin_tmp, _local_tmp, _origin_repo, engine) = temp_repo_with_origin();
    let err = engine
        .fetch_refspec("refs/heads/does-not-exist:refs/heads/pr-1")
        .unwrap_err();
    assert!(err.to_string().contains("git fetch"));
}

#[test]
fn create_worktree_for_existing_branch_checks_out_branch() {
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
fn create_worktree_for_existing_branch_rejects_existing_dir() {
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
fn ensure_base_ref_available_creates_missing_local_branch() {
    // A local repo with `origin` configured but that hasn't fetched
    // `main` yet, to exercise the "no local branch" path.
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
    // See temp_repo_with_origin() for why HEAD must move off main first.
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
fn ensure_base_ref_available_fast_forwards_existing_local_branch() {
    let (origin_tmp, _local_tmp, origin_repo, engine) = temp_repo_with_origin();
    let before = engine
        .repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();

    // Origin's main moves forward after the local clone already has it.
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
fn ensure_base_ref_available_leaves_diverged_branch_untouched() {
    let (_origin_tmp, _local_tmp, origin_repo, engine) = temp_repo_with_origin();

    // Diverge local main from origin's main with a local-only commit.
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

    // Origin also moves forward independently — a true divergence.
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
