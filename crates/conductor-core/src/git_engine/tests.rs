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
    use super::ChangeCounts;
    let want = |added, modified, deleted, staged| ChangeCounts {
        added,
        modified,
        deleted,
        staged,
    };
    type Case = (&'static str, fn(&Tree), ChangeCounts);
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

        let counts = GitEngine::change_counts_at(&repo.path).unwrap();

        assert_eq!(counts, want, "{label}");
        assert!(!counts.is_clean(), "{label}");
        assert!(
            repo.engine().list_worktrees().unwrap()[0].counts.is_none(),
            "一覧が件数まで数えている: {label}"
        );
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
fn fetch_branchesはリモート追跡refだけを進める() {
    let origin = Origin::new();
    let repo = TestRepo::with_remote_and_main(&origin);
    let ahead = origin.commit_on("feature", "pushed after the picker fetched");

    repo.engine()
        .fetch_branches("origin", &["feature", "main"])
        .unwrap();

    assert_eq!(
        repo.repo
            .refname_to_id("refs/remotes/origin/feature")
            .unwrap(),
        ahead
    );
    assert!(
        repo.repo
            .find_branch("feature", git2::BranchType::Local)
            .is_err(),
        "ローカルブランチまで作ってはならない"
    );
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
fn 既定ブランチはorigin_headから順に落ちる() {
    let head_points_elsewhere = TestRepo::with_base_commit();
    let tip = head_points_elsewhere.tip("main");
    head_points_elsewhere
        .remote_ref("origin/main", tip)
        .remote_ref("origin/staging", tip);
    head_points_elsewhere
        .repo
        .reference_symbolic(
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/staging",
            true,
            "test",
        )
        .unwrap();

    let remote_only = TestRepo::with_base_commit();
    remote_only.remote_ref("origin/master", remote_only.tip("main"));

    let local_only = TestRepo::with_base_commit();
    let local_tip = local_only.tip("main");
    local_only.branch_at("master", local_tip).checkout("master");
    local_only
        .repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .delete()
        .unwrap();

    let dangling = TestRepo::with_base_commit();
    dangling.remote_ref("origin/main", dangling.tip("main"));
    dangling
        .repo
        .reference_symbolic(
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/master",
            true,
            "test",
        )
        .unwrap();

    for (label, repo, want) in [
        (
            "origin/HEAD は origin/main に勝つ",
            &head_points_elsewhere,
            "staging",
        ),
        ("ぶら下がった origin/HEAD は飛ばす", &dangling, "main"),
        ("origin/HEAD が無ければリモート", &remote_only, "master"),
        ("リモートが無ければローカル", &local_only, "master"),
        ("何も無ければ main", &TestRepo::new(), "main"),
    ] {
        assert_eq!(repo.engine().default_branch(), want, "{label}");
    }
}

#[test]
fn リモートブランチの取り込みはローカルに同名があっても作れる() {
    let repo = TestRepo::with_base_commit();
    repo.repo.remote("origin", "/nowhere").unwrap();
    repo.branch_at("topic", repo.tip("main"));
    repo.file("b.txt", "1\n").add("b.txt");
    let ahead = repo.commit("ahead");
    repo.remote_ref("origin/topic", ahead);
    let dir = tempfile::tempdir().unwrap();

    let wt = repo
        .engine()
        .create_worktree_from_remote("origin/topic", Some(dir.path()))
        .unwrap();

    assert!(wt.join("b.txt").exists(), "origin に追いついていない");
    assert_eq!(repo.tip("topic"), ahead);
    assert_eq!(
        repo.repo
            .find_branch("topic", git2::BranchType::Local)
            .unwrap()
            .upstream()
            .unwrap()
            .name()
            .unwrap(),
        Some("origin/topic")
    );
}

#[test]
fn 分岐したローカルブランチはリモートに合わせて巻き戻らない() {
    let repo = TestRepo::with_base_commit();
    repo.repo.remote("origin", "/nowhere").unwrap();
    let base = repo.tip("main");
    repo.file("b.txt", "1\n").add("b.txt");
    repo.remote_ref("origin/topic", repo.commit("remote side"));
    repo.checkout_at("topic", base);
    repo.file("c.txt", "1\n").add("c.txt");
    let local_side = repo.commit("local side");
    repo.checkout("main");
    let dir = tempfile::tempdir().unwrap();

    repo.engine()
        .create_worktree_from_remote("origin/topic", Some(dir.path()))
        .unwrap();

    assert_eq!(repo.tip("topic"), local_side);
}

/// post-checkout がそのブランチにコミットを足すと、worktree を作ったあとでは
/// もう早送りできない。取り込みはリモートの内容で始まっていなければならない。
#[test]
fn post_checkoutがコミットしてもリモートの内容で作られる() {
    let repo = TestRepo::with_base_commit();
    repo.repo.remote("origin", "/nowhere").unwrap();
    repo.branch_at("topic", repo.tip("main"));
    repo.file("b.txt", "1\n").add("b.txt");
    repo.remote_ref("origin/topic", repo.commit("ahead"));
    write_hook(
        &repo.path,
        "post-checkout",
        "#!/bin/sh\n\
         echo stamp > stamp.txt\n\
         git add stamp.txt\n\
         git -c user.email=t@t -c user.name=t commit -q -m stamp\n",
    );
    let dir = tempfile::tempdir().unwrap();

    let wt = repo
        .engine()
        .create_worktree_from_remote("origin/topic", Some(dir.path()))
        .unwrap();

    assert!(
        wt.join("stamp.txt").exists(),
        "post-checkout が走っていない"
    );
    assert!(wt.join("b.txt").exists(), "リモートの内容が入っていない");
}

#[test]
fn 別のworktreeでチェックアウト中のブランチは取り込まない() {
    let repo = TestRepo::with_base_commit();
    repo.repo.remote("origin", "/nowhere").unwrap();
    let live = repo.linked_worktree("topic");
    let before = repo.tip("topic");
    repo.file("b.txt", "1\n").add("b.txt");
    repo.remote_ref("origin/topic", repo.commit("ahead"));
    let dir = tempfile::tempdir().unwrap();

    let err = repo
        .engine()
        .create_worktree_from_remote("origin/topic", Some(dir.path()))
        .unwrap_err();

    assert!(err.to_string().contains("already checked out"), "{err}");
    assert!(live.path.exists(), "生きている worktree が消えた");
    assert_eq!(repo.tip("topic"), before, "ref だけ進んでいる");
}

/// worktree のディレクトリ名は feature/topic と topic を同じ "topic" に潰す。
/// 別の場所で生きている worktree を、名前が当たっただけで消してはいけない。
#[test]
fn 置き場所の違う同名worktreeはpruneしない() {
    let repo = TestRepo::with_base_commit();
    repo.repo.remote("origin", "/nowhere").unwrap();
    let live = tempfile::tempdir().unwrap();
    let live_wt = live.path().join("topic");
    run_git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-b",
            "feature/topic",
            &live_wt.display().to_string(),
        ],
    );
    fs::write(live_wt.join("wip.txt"), "uncommitted\n").unwrap();
    repo.remote_ref("origin/topic", repo.tip("main"));
    let dir = tempfile::tempdir().unwrap();

    repo.engine()
        .create_worktree_from_remote("origin/topic", Some(dir.path()))
        .unwrap();

    assert!(live_wt.join("wip.txt").exists(), "未コミットごと消された");
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// 本家 (upstream) のブランチを覗くだけで、自分のフォーク (origin) を向いていた
/// push 先が書き換わってはいけない。
#[test]
fn 別のリモートを追跡中のブランチはupstreamを奪われない() {
    let repo = TestRepo::with_base_commit();
    repo.repo.remote("origin", "/nowhere").unwrap();
    repo.repo.remote("upstream", "/nowhere").unwrap();
    repo.remote_ref("origin/topic", repo.tip("main"));
    repo.branch_at("topic", repo.tip("main"));
    repo.repo
        .find_branch("topic", git2::BranchType::Local)
        .unwrap()
        .set_upstream(Some("origin/topic"))
        .unwrap();
    repo.file("b.txt", "1\n").add("b.txt");
    repo.remote_ref("upstream/topic", repo.commit("upstream side"));
    let dir = tempfile::tempdir().unwrap();

    repo.engine()
        .create_worktree_from_remote("upstream/topic", Some(dir.path()))
        .unwrap();

    assert_eq!(
        repo.repo
            .find_branch("topic", git2::BranchType::Local)
            .unwrap()
            .upstream()
            .unwrap()
            .name()
            .unwrap(),
        Some("origin/topic")
    );
}

#[test]
fn origin以外のリモートでもローカルブランチ名にリモート名は残らない() {
    let repo = TestRepo::with_base_commit();
    repo.repo.remote("upstream", "/nowhere").unwrap();
    repo.remote_ref("upstream/topic", repo.tip("main"));
    let dir = tempfile::tempdir().unwrap();

    repo.engine()
        .create_worktree_from_remote("upstream/topic", Some(dir.path()))
        .unwrap();

    assert_eq!(
        repo.engine().list_local_branches().unwrap(),
        vec!["main".to_string(), "topic".to_string()]
    );
}

fn write_hook(repo_path: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = repo_path.join(".git/hooks").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
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
fn head_logはskipで次のページを続きから出す() {
    let repo = TestRepo::with_base_commit();
    for n in ["second", "third", "fourth"] {
        repo.file("a.txt", n).add("a.txt").commit(n);
    }
    let engine = repo.engine();
    let page = |skip| -> Vec<String> {
        engine
            .head_log(skip, 2)
            .unwrap()
            .into_iter()
            .map(|c| c.message)
            .collect()
    };
    assert_eq!(page(0), ["fourth", "third"]);
    assert_eq!(page(2), ["second", "base"]);
    assert!(page(4).is_empty());
}

#[test]
fn head_logはリンクされたworktreeのheadを歩く() {
    let repo = TestRepo::with_base_commit();
    let linked = repo.linked_worktree("wt");
    linked.file("b.txt", "b").add("b.txt").commit("only on wt");

    let messages: Vec<String> = linked
        .engine()
        .head_log(0, 10)
        .unwrap()
        .into_iter()
        .map(|c| c.message)
        .collect();
    assert_eq!(messages, ["only on wt", "base"]);
    assert_eq!(repo.engine().head_log(0, 10).unwrap().len(), 1);
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

/// 各パスの期待値は「そのフィクスチャをどう作ったか」(add したか、.gitignore に載せたか、
/// 触っていないか) から決めていて、実装をなぞらない。
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

/// untracked cache を有効にしたリポジトリと、index を書き戻したかを mtime で見る道具。
fn untracked_cache_repo() -> (TestRepo, impl Fn(), impl Fn() -> bool) {
    use std::time::{Duration, UNIX_EPOCH};

    let repo = TestRepo::with_base_commit();
    assert!(
        std::process::Command::new("git")
            .args([
                "-C",
                repo.path.to_str().unwrap(),
                "config",
                "core.untrackedCache",
                "true"
            ])
            .status()
            .unwrap()
            .success()
    );
    repo.file("newdir/a.txt", "a\n");
    let index = repo.path.join(".git").join("index");
    let stale = UNIX_EPOCH + Duration::from_secs(1000);
    let touch_stale = {
        let index = index.clone();
        move || {
            std::fs::File::options()
                .write(true)
                .open(&index)
                .unwrap()
                .set_modified(stale)
                .unwrap();
        }
    };
    let index_written = move || std::fs::metadata(&index).unwrap().modified().unwrap() != stale;
    (repo, touch_stale, index_written)
}

#[test]
fn 遅かったstatusの次の1回だけがindexを書き戻してuntracked_cacheを温める() {
    let (repo, touch_stale, index_written) = untracked_cache_repo();
    let never_fast_enough = std::time::Duration::ZERO;

    touch_stale();
    GitStatusMap::load_with(&repo.path, never_fast_enough).unwrap();
    assert!(!index_written(), "遅さが分かる前は index に触らない");

    touch_stale();
    GitStatusMap::load_with(&repo.path, never_fast_enough).unwrap();
    assert!(
        index_written(),
        "遅かった次の 1 回は素の status で cache を書き戻す"
    );

    touch_stale();
    GitStatusMap::load_with(&repo.path, never_fast_enough).unwrap();
    assert!(
        !index_written(),
        "温めた直後も遅ければ効かなかった印で、間隔が空くまで書き戻さない"
    );
}

#[test]
fn 温めが効いた後にまた冷えれば待たずに温め直す() {
    let (repo, touch_stale, index_written) = untracked_cache_repo();
    let never_fast_enough = std::time::Duration::ZERO;
    let always_fast_enough = std::time::Duration::from_secs(3600);

    GitStatusMap::load_with(&repo.path, never_fast_enough).unwrap();
    GitStatusMap::load_with(&repo.path, never_fast_enough).unwrap();
    GitStatusMap::load_with(&repo.path, always_fast_enough).unwrap();
    GitStatusMap::load_with(&repo.path, never_fast_enough).unwrap();

    touch_stale();
    GitStatusMap::load_with(&repo.path, never_fast_enough).unwrap();
    assert!(
        index_written(),
        "温めの後に速い status を見たら効いた印で、次に冷えたときは間隔を待たない (pull や merge が cache を捨てた後)"
    );
}

#[test]
fn 速いリポジトリのstatusはindexに触らない() {
    let (repo, touch_stale, index_written) = untracked_cache_repo();

    for _ in 0..3 {
        touch_stale();
        GitStatusMap::load(&repo.path).unwrap();
        assert!(!index_written(), "速ければ素の status は一度も打たない");
    }
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
fn 短い名前からのリネームは隣のファイルのstatusを消さない() {
    let repo = TestRepo::new();
    repo.file("foo", "a\n").add("foo");
    repo.file("keep.txt", "a\n").add("keep.txt");
    repo.commit("initial");

    fs::rename(repo.path.join("foo"), repo.path.join("bar")).unwrap();
    let mut index = repo.repo.index().unwrap();
    index.remove_path(Path::new("foo")).unwrap();
    index.add_path(Path::new("bar")).unwrap();
    index.write().unwrap();
    repo.file("keep.txt", "b\n");

    let map = GitStatusMap::load(&repo.path).unwrap();

    assert!(map.status("bar").is_some(), "リネーム先が消えた");
    assert!(
        map.status("foo").is_some_and(|s| s.is_index_deleted()),
        "リネーム元が消えた: {:?}",
        map.status("foo")
    );
    assert!(
        map.status("keep.txt").is_some_and(|s| s.is_wt_modified()),
        "隣のファイルの変更が消えた: {:?}",
        map.status("keep.txt")
    );
}

#[test]
fn 強制追加したファイルはignored扱いにしない() {
    let repo = TestRepo::new();
    repo.file(".gitignore", "*.log\nbuild/\n").add(".gitignore");
    repo.file("forced.log", "a\n").add("forced.log");
    repo.commit("initial");
    repo.file("plain.log", "a\n");
    repo.file("build/deep/x.txt", "a\n");

    let oracle = status_map::IgnoreOracle::open(&repo.path);

    for (path, want) in [
        ("forced.log", TreeGitState::Tracked),
        (".gitignore", TreeGitState::Tracked),
        ("plain.log", TreeGitState::Ignored),
        ("build", TreeGitState::Ignored),
        ("build/deep/x.txt", TreeGitState::Ignored),
    ] {
        assert_eq!(oracle.classify(path), want, "{path}");
    }
}

#[test]
fn 同じパスに2レコード来てもビットを落とさない() {
    let repo = TestRepo::new();
    repo.file("foo", "a\n").add("foo");
    repo.commit("initial");
    let mut index = repo.repo.index().unwrap();
    index.remove_path(Path::new("foo")).unwrap();
    index.write().unwrap();

    let status = GitStatusMap::load(&repo.path).unwrap().status("foo");

    assert!(
        status.is_some_and(|s| s.is_index_deleted() && s.is_wt_new()),
        "{status:?}"
    );
}

#[test]
fn 畳まれた未追跡ディレクトリの配下もstatusを答える() {
    let map = GitStatusMap::load(&status_fixture().path).unwrap();

    for path in ["newdir/a.txt", "newdir/sub/b.txt"] {
        assert!(
            map.status(path).is_some_and(|s| s.is_wt_new()),
            "{path}: {:?}",
            map.status(path)
        );
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
