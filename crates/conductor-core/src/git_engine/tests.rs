use std::fs;
use std::path::{Path, PathBuf};

use git2::Repository;

use super::commit_log::format_duration_ago;
use super::*;
use crate::test_support::{TestRepo, Tree, signature};

impl TestRepo {
    /// origin を登録したリポジトリ。HEAD を unborn の scratch に逃がしてあるのは、
    /// git が「チェックアウト中のブランチ」への fetch を拒むため。
    fn with_remote(origin: &Origin) -> Self {
        let repo = Self::new();
        repo.repo.set_head("refs/heads/scratch").unwrap();
        repo.repo.remote("origin", &origin.url()).unwrap();
        repo
    }

    fn with_remote_and_main(origin: &Origin) -> Self {
        let repo = Self::with_remote(origin);
        repo.engine().fetch_refspec("main:refs/heads/main").unwrap();
        repo
    }

    fn with_origin_url(url: &str) -> Self {
        let repo = Self::new();
        repo.repo.remote("origin", url).unwrap();
        repo
    }
}

/// bare の origin。main に初期コミットが 1 つある。
struct Origin {
    dir: tempfile::TempDir,
    repo: Repository,
}

impl Origin {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(dir.path()).unwrap();
        {
            let sig = signature();
            let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }
        Self { dir, repo }
    }

    fn url(&self) -> String {
        self.dir.path().display().to_string()
    }

    /// main の tip を親にして branch へコミットする。
    fn commit_on(&self, branch: &str, message: &str) -> git2::Oid {
        let sig = signature();
        let parent = self
            .repo
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap();
        self.repo
            .commit(
                Some(&format!("refs/heads/{branch}")),
                &sig,
                &sig,
                message,
                &parent.tree().unwrap(),
                &[&parent],
            )
            .unwrap()
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

#[test]
fn どこから開いてもmainのパスとconductor_dirはmain側を指す() {
    let repo = TestRepo::with_base_commit();
    let linked = repo.linked_worktree("wt");
    let main_sub = repo.path.join("src/deep");
    let linked_sub = linked.path.join("src/deep");
    fs::create_dir_all(&main_sub).unwrap();
    fs::create_dir_all(&linked_sub).unwrap();
    let expected = canonical(&repo.path);

    for (label, from) in [
        ("main", repo.path.clone()),
        ("linked", linked.path.clone()),
        ("main のサブディレクトリ", main_sub),
        ("linked のサブディレクトリ", linked_sub),
    ] {
        let main_path = GitEngine::open(&from)
            .unwrap()
            .main_worktree_path()
            .unwrap();
        assert_eq!(canonical(&main_path), expected, "{label}");

        let dir = conductor_dir(&from);
        assert_eq!(dir.file_name().unwrap(), ".conductor", "{label}");
        assert_eq!(canonical(dir.parent().unwrap()), expected, "{label}");
    }
}

#[test]
fn worktree一覧はmainを先頭にlinkedを続ける() {
    let repo = TestRepo::with_base_commit();
    repo.linked_worktree("wt");

    let listed: Vec<(bool, String)> = repo
        .engine()
        .list_worktrees()
        .unwrap()
        .into_iter()
        .map(|w| (w.is_main, w.branch))
        .collect();

    assert_eq!(
        listed,
        vec![(true, "main".to_string()), (false, "wt".to_string())]
    );
}

#[test]
fn コミットの無いリポジトリでも一覧が取れる() {
    let repo = TestRepo::new();
    repo.file("hello.txt", "hi\n");

    let worktrees = repo.engine().list_worktrees().unwrap();

    assert_eq!(worktrees.len(), 1);
    assert!(worktrees[0].is_main);
    assert!(worktrees[0].head_oid.is_none());
}

#[test]
fn 変更件数はファイルを1回だけ数えstagedだけ重複して数える() {
    struct Counts {
        added: usize,
        modified: usize,
        deleted: usize,
        staged: usize,
    }
    let want = |added, modified, deleted, staged| Counts {
        added,
        modified,
        deleted,
        staged,
    };
    type Case = (&'static str, fn(&Tree), Counts);
    let cases: [Case; 6] = [
        (
            "未追跡",
            |t| {
                t.file("b.txt", "1\n");
            },
            want(1, 0, 0, 0),
        ),
        (
            "変更",
            |t| {
                t.file("a.txt", "2\n");
            },
            want(0, 1, 0, 0),
        ),
        (
            "変更を add",
            |t| {
                t.file("a.txt", "2\n").add("a.txt");
            },
            want(0, 1, 0, 1),
        ),
        (
            "add 後にさらに変更",
            |t| {
                t.file("a.txt", "2\n").add("a.txt").file("a.txt", "3\n");
            },
            want(0, 1, 0, 1),
        ),
        (
            "削除",
            |t| {
                fs::remove_file(t.path.join("a.txt")).unwrap();
            },
            want(0, 0, 1, 0),
        ),
        (
            "新規を add",
            |t| {
                t.file("b.txt", "1\n").add("b.txt");
            },
            want(1, 0, 0, 1),
        ),
    ];
    for (label, setup, want) in cases {
        let repo = TestRepo::with_base_commit();
        setup(&repo);

        let info = repo.engine().list_worktrees().unwrap().remove(0);

        assert_eq!(
            (info.added, info.modified, info.deleted, info.staged),
            (want.added, want.modified, want.deleted, want.staged),
            "{label}"
        );
        assert!(!info.is_clean, "{label}");
    }
}

#[test]
fn ブランチprefixは先頭の1つだけ落とす() {
    for (branch, want) in [
        ("feature/foo", "foo"),
        ("fix/bar", "bar"),
        ("feature/fix/nested", "fix/nested"),
        ("main", "main"),
        ("release/1.0", "1.0"),
    ] {
        assert_eq!(GitEngine::strip_branch_prefix(branch), want, "{branch}");
    }
}

#[test]
fn pr_urlはリモートの綴りに依らずgithubとgitlabで形が分かれる() {
    let github = "https://github.com/owner/repo/pull/feature-x";
    let gitlab =
        "https://gitlab.com/owner/repo/-/merge_requests/new?merge_request[source_branch]=feature-x";
    for (url, want) in [
        ("git@github.com:owner/repo.git", Some(github)),
        ("https://github.com/owner/repo.git", Some(github)),
        ("https://github.com/owner/repo", Some(github)),
        ("ssh://git@github.com/owner/repo.git", Some(github)),
        ("git@gitlab.com:owner/repo.git", Some(gitlab)),
        ("file:///srv/repo.git", None),
    ] {
        let repo = TestRepo::with_origin_url(url);
        assert_eq!(
            repo.engine().pr_url_for_branch("feature-x").as_deref(),
            want,
            "{url}"
        );
    }
}

#[test]
fn grab状態は往復しlinked_worktreeからも同じファイルを見る() {
    let repo = TestRepo::with_base_commit();
    let linked = repo.linked_worktree("wt");
    let state = |session: Option<&str>| GrabState {
        branch: "feature-x".to_string(),
        source_worktree: PathBuf::from("/tmp/test-worktree"),
        stash_branch: "feature-x__grab".to_string(),
        claude_session_id: session.map(String::from),
    };

    for (label, saved) in [
        ("セッション無し", state(None)),
        ("セッション付き", state(Some("abc12345-6789-0def"))),
    ] {
        linked.engine().save_grab_state(&saved).unwrap();
        assert_eq!(
            repo.engine().load_grab_state().unwrap(),
            Some(saved),
            "{label}"
        );
        repo.engine().remove_grab_state().unwrap();
        assert_eq!(linked.engine().load_grab_state().unwrap(), None, "{label}");
    }

    let legacy_file = repo.engine().git_common_dir().unwrap().join("wt-grab");
    fs::write(
        &legacy_file,
        "feature-x\n/tmp/test-worktree\nfeature-x__grab\n",
    )
    .unwrap();
    assert_eq!(
        repo.engine().load_grab_state().unwrap(),
        Some(state(None)),
        "zsh の wt が書く 3 行形式"
    );
}

#[test]
fn fetch_refspecはローカルブランチを作る() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote_and_main(&origin);
    origin.commit_on("feature", "a PR head");

    repo.engine()
        .fetch_refspec("refs/heads/feature:refs/heads/pr-99")
        .unwrap();

    let message = repo
        .repo
        .find_branch("pr-99", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .message()
        .map(String::from);
    assert_eq!(message.as_deref(), Some("a PR head"));
}

#[test]
fn 知らないrefへのfetch_refspecは失敗を返す() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote_and_main(&origin);

    let err = repo
        .engine()
        .fetch_refspec("refs/heads/does-not-exist:refs/heads/pr-1")
        .unwrap_err();

    assert!(err.to_string().contains("git fetch"), "{err}");
}

#[test]
fn 既存ブランチのworktree作成はそのブランチを出す() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote_and_main(&origin);
    origin.commit_on("feature", "a PR head");
    repo.engine()
        .fetch_refspec("refs/heads/feature:refs/heads/pr-99")
        .unwrap();
    let wt_dir = repo.worktrees_dir().join("pr-99");

    let created = repo
        .engine()
        .create_worktree_for_existing_branch("pr-99", &wt_dir)
        .unwrap();

    assert_eq!(created, wt_dir);
    assert_eq!(Tree::open(wt_dir).head_branch(), "pr-99");
}

#[test]
fn 既にあるディレクトリへのworktree作成は拒む() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote_and_main(&origin);
    let wt_dir = repo.worktrees_dir().join("pr-99");
    fs::create_dir_all(&wt_dir).unwrap();

    let result = repo
        .engine()
        .create_worktree_for_existing_branch("main", &wt_dir);

    assert!(result.is_err());
}

#[test]
fn base_refは無ければ作られる() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote(&origin);
    let has_main = || {
        repo.repo
            .find_branch("main", git2::BranchType::Local)
            .is_ok()
    };
    assert!(!has_main());

    repo.engine().ensure_base_ref_available("main").unwrap();

    assert!(has_main());
}

#[test]
fn base_refはoriginが先に進んでいれば早送りされる() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote_and_main(&origin);
    let later = origin.commit_on("main", "a later commit on main");

    repo.engine().ensure_base_ref_available("main").unwrap();

    assert_eq!(repo.tip("main"), later);
}

#[test]
fn base_refは分岐していれば触らない() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote_and_main(&origin);
    repo.checkout("main");
    let local_only = repo
        .file("local.txt", "1\n")
        .add("local.txt")
        .commit("local-only");
    origin.commit_on("main", "origin-only commit");

    repo.engine().ensure_base_ref_available("main").unwrap();

    assert_eq!(repo.tip("main"), local_only);
}

#[test]
fn base_refの解決はorigin_ローカル_headの順() {
    let origin = Origin::new();
    let unborn = TestRepo::new();
    let local_only = TestRepo::with_base_commit();
    let with_origin = TestRepo::with_remote_and_main(&origin);
    with_origin.engine().fetch_origin().unwrap();

    for (label, repo, want) in [
        ("何も無い", &unborn, "HEAD"),
        ("ローカルだけ", &local_only, "main"),
        ("origin あり", &with_origin, "origin/main"),
    ] {
        assert_eq!(repo.engine().resolve_base_ref("main"), want, "{label}");
    }
    assert_eq!(
        with_origin.engine().list_remote_branches().unwrap(),
        vec!["origin/main".to_string()]
    );
}

#[test]
fn is_branch_merged_intoは同一か祖先なら真() {
    let repo = TestRepo::with_base_commit();
    let feature = repo.linked_worktree("feature");
    feature
        .file("f.txt", "1\n")
        .add("f.txt")
        .commit("feature work");
    let engine = repo.engine();

    assert!(!engine.is_branch_merged_into("feature", "main").unwrap());
    assert!(engine.is_branch_merged_into("main", "feature").unwrap());

    engine.merge_into_main("feature", "main").unwrap();
    assert!(engine.is_branch_merged_into("feature", "main").unwrap());
}

#[test]
fn merge_into_mainはfast_forwardだけ行う() {
    for diverged in [false, true] {
        let repo = TestRepo::with_base_commit();
        let feature = repo.linked_worktree("feature");
        feature
            .file("f.txt", "1\n")
            .add("f.txt")
            .commit("feature work");
        if diverged {
            repo.file("m.txt", "1\n").add("m.txt").commit("main work");
        }
        let main_before = repo.tip("main");

        let message = repo.engine().merge_into_main("feature", "main").unwrap();

        if diverged {
            assert!(message.contains("Manual merge needed"), "{message}");
            assert_eq!(repo.tip("main"), main_before);
        } else {
            assert!(message.starts_with("Fast-forward merged"), "{message}");
            assert_eq!(repo.tip("main"), repo.tip("feature"));
            assert_eq!(repo.read("f.txt"), "1\n", "working tree も更新される");
        }
    }
}

#[test]
fn cherry_pickは成功なら新コミットを作りコンフリクトならheadに戻す() {
    for conflicting in [false, true] {
        let repo = TestRepo::with_base_commit();
        let target = repo.linked_worktree("target");
        repo.branch("feature")
            .file("a.txt", "feature\n")
            .add("a.txt");
        let picked = repo.commit("feature change");
        if conflicting {
            target
                .file("a.txt", "other\n")
                .add("a.txt")
                .commit("target change");
        }

        let message = repo
            .engine()
            .cherry_pick_to_worktree(&target.path, &picked.to_string())
            .unwrap();

        assert_eq!(target.repo.state(), git2::RepositoryState::Clean);
        if conflicting {
            assert!(message.contains("aborted due to conflicts"), "{message}");
            assert_eq!(target.read("a.txt"), "other\n");
            assert!(!target.repo.index().unwrap().has_conflicts());
        } else {
            assert!(message.contains("feature change"), "{message}");
            assert_eq!(target.read("a.txt"), "feature\n");
            let head = target.repo.head().unwrap().peel_to_commit().unwrap();
            assert_eq!(head.message().map(str::trim_end), Some("feature change"));
        }
    }
}

#[test]
fn list_branch_commitsは新しい順にlimit件() {
    let repo = TestRepo::with_base_commit();
    repo.file("a.txt", "2\n")
        .add("a.txt")
        .commit("second\n\nbody");
    repo.file("a.txt", "3\n").add("a.txt").commit("third");

    let commits = repo.engine().list_branch_commits("main", 2).unwrap();

    let messages: Vec<&str> = commits.iter().map(|c| c.message.as_str()).collect();
    assert_eq!(messages, ["third", "second"]);
    assert!(
        commits
            .iter()
            .all(|c| c.short_oid.len() == 8 && c.oid.starts_with(&c.short_oid))
    );
    assert_eq!(commits[0].author, "Test");
}

#[test]
fn 経過時間は最大の単位で丸める() {
    for (seconds, want) in [
        (-5, "just now"),
        (5, "5s ago"),
        (61, "1m ago"),
        (3600, "1h ago"),
        (86_400, "1d ago"),
        (7 * 86_400, "1w ago"),
        (35 * 86_400, "1mo ago"),
    ] {
        assert_eq!(
            format_duration_ago(chrono::Duration::seconds(seconds)),
            want,
            "{seconds}s"
        );
    }
}

#[test]
fn 親ブランチは作成元を答え派生はその逆() {
    let repo = TestRepo::with_base_commit();
    repo.branch("feature")
        .file("f.txt", "1\n")
        .add("f.txt")
        .commit("f1");
    repo.branch("feature2")
        .file("g.txt", "1\n")
        .add("g.txt")
        .commit("f2");
    repo.checkout("main");
    let engine = repo.engine();
    let all = ["feature".to_string(), "feature2".to_string()];

    assert_eq!(
        engine
            .detect_parent_branch("feature2", "main", &all)
            .as_deref(),
        Some("feature")
    );
    assert_eq!(
        engine
            .detect_parent_branch("feature", "main", &all)
            .as_deref(),
        Some("main")
    );
    assert_eq!(engine.detect_parent_branch("main", "main", &all), None);
    assert_eq!(
        engine
            .find_derived_branches("feature", "main", &all)
            .unwrap(),
        vec!["feature2".to_string()]
    );
}

#[test]
fn 最近触ったファイルはdirtyを先にコミット分を後に重複なく返す() {
    let repo = TestRepo::new();
    repo.file("a.txt", "1\n")
        .file("b.txt", "1\n")
        .add("a.txt")
        .add("b.txt");
    repo.commit("both");
    repo.file("a.txt", "2\n").file("c.txt", "1\n");

    assert_eq!(
        recently_modified_files(&repo.path, 10).unwrap(),
        ["a.txt", "c.txt", "b.txt"]
    );
    assert_eq!(
        recently_modified_files(&repo.path, 2).unwrap(),
        ["a.txt", "c.txt"]
    );
}

#[test]
fn remove_worktreeはエントリとディレクトリを消す() {
    let repo = TestRepo::with_base_commit();
    repo.linked_worktree("wt");
    let engine = repo.engine();
    let listed_path = engine.list_worktrees().unwrap().remove(1).path;

    engine.remove_worktree(&listed_path).unwrap();

    assert!(!listed_path.exists());
    assert_eq!(engine.list_worktrees().unwrap().len(), 1);
}

#[test]
fn 消えたworktreeはstaleとして見つかりpruneできる() {
    let repo = TestRepo::with_base_commit();
    let linked = repo.linked_worktree("wt");
    fs::remove_dir_all(&linked.path).unwrap();
    let engine = repo.engine();

    assert_eq!(
        engine.find_stale_worktrees().unwrap(),
        vec!["wt".to_string()]
    );

    engine.prune_stale_worktree("wt").unwrap();
    assert!(engine.find_stale_worktrees().unwrap().is_empty());
    assert_eq!(engine.list_worktrees().unwrap().len(), 1);
}

#[test]
fn delete_branchはチェックアウトされていないブランチを消す() {
    let repo = TestRepo::with_base_commit();
    repo.branch("feature").checkout("main");
    let engine = repo.engine();

    engine.delete_branch("feature", false).unwrap();

    assert_eq!(
        engine.list_local_branches().unwrap(),
        vec!["main".to_string()]
    );
}

/// classify() が区別すべき全組み合わせ。各パスの期待値は「そのフィクスチャをどう作ったか」
/// (add したか、.gitignore に載せたか、触っていないか) から決めていて、実装をなぞらない。
fn status_fixture() -> TestRepo {
    let repo = TestRepo::new();
    for name in [
        "untouched.txt",
        "staged_only.txt",
        "unstaged_only.txt",
        "both.txt",
    ] {
        repo.file(name, "a\n").add(name);
    }
    repo.file(".gitignore", "build/\n").add(".gitignore");
    repo.commit("initial");

    repo.file("staged_only.txt", "b\n").add("staged_only.txt");
    repo.file("unstaged_only.txt", "b\n");
    repo.file("both.txt", "b\n")
        .add("both.txt")
        .file("both.txt", "c\n");
    repo.file("untracked.txt", "a\n");
    repo.file("build/deep/x.txt", "a\n");
    repo.file("build2/y.txt", "a\n");
    repo.file("newdir/a.txt", "a\n")
        .file("newdir/sub/b.txt", "a\n");
    repo
}

#[test]
fn 分類は各フィクスチャが作られた状態を答える() {
    let repo = status_fixture();
    let map = GitStatusMap::load(&repo.path).unwrap();

    for (path, want) in [
        ("untouched.txt", TreeGitState::Tracked),
        ("staged_only.txt", TreeGitState::Tracked),
        ("unstaged_only.txt", TreeGitState::Tracked),
        ("both.txt", TreeGitState::Tracked),
        ("untracked.txt", TreeGitState::Untracked),
        // libgit2 は build/ を末尾スラッシュ付きの 1 エントリに折りたたむ (実測)。
        ("build", TreeGitState::Ignored),
        ("build/deep/x.txt", TreeGitState::Ignored),
        // 接頭辞を共有するだけの兄弟は ignored ではない。
        ("build2/y.txt", TreeGitState::Untracked),
        // git が見たことのないディレクトリは中身と同じ扱い。
        ("newdir", TreeGitState::Untracked),
        ("newdir/sub", TreeGitState::Untracked),
        ("newdir/a.txt", TreeGitState::Untracked),
        ("newdir/sub/b.txt", TreeGitState::Untracked),
        ("nonexistent-dir", TreeGitState::Tracked),
    ] {
        assert_eq!(map.classify(path), want, "{path}");
    }
}

#[test]
fn 本当の削除はちゃんと報告する() {
    let repo = status_fixture();
    fs::remove_file(repo.path.join("untouched.txt")).unwrap();

    let map = GitStatusMap::load(&repo.path).unwrap();

    assert!(
        map.status("untouched.txt")
            .is_some_and(|s| s.is_wt_deleted()),
        "{:?}",
        map.status("untouched.txt")
    );
}

/// リグレッション: ケース違いの 2 エントリに実ファイルが 1 つしか無い状態を git 本体は
/// clean と報告する。cfg で外してあるのは、走らない環境で「テストが無い」ことを一覧から
/// 見えるようにするため。
#[cfg(target_os = "macos")]
#[test]
fn 大小が衝突するエントリは削除扱いにしない() {
    let repo = TestRepo::new();
    if !crate::test_support::fs_ignores_case(&repo.path) {
        eprintln!("skipped: 大文字小文字を区別するファイルシステムでは再現しない");
        return;
    }
    let sig = signature();
    let blob = repo.repo.blob(b"image data\n").unwrap();
    let mut builder = repo.repo.treebuilder(None).unwrap();
    builder.insert("Instagram.png", blob, 0o100644).unwrap();
    builder.insert("instagram.png", blob, 0o100644).unwrap();
    let tree = repo.repo.find_tree(builder.write().unwrap()).unwrap();
    repo.repo
        .commit(Some("HEAD"), &sig, &sig, "both cases", &tree, &[])
        .unwrap();
    repo.checkout("main");
    // libgit2 の checkout は衝突する 2 エントリを index 上で 1 つに畳む。git 本体が
    // clone したリポジトリは両方を保持するので書き戻す。
    let mut index = repo.repo.index().unwrap();
    index.read_tree(&tree).unwrap();
    index.write().unwrap();

    let map = GitStatusMap::load(&repo.path).unwrap();

    assert_eq!(map.status("Instagram.png"), None);
    assert_eq!(map.status("instagram.png"), None);
}
