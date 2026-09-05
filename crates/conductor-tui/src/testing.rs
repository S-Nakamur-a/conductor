//! テストから svc の往復を待つための足回り。

use std::path::{Path, PathBuf};
use std::time::Duration;

use conductor_core::git_engine::WorktreeInfo;
use conductor_svc::{EventKind, Services};

use crate::effect::apply;
use crate::task::TaskResult;
use crate::workspace::Workspace;

/// 何も届かなくなるまで svc の結果を消費する。
///
/// ワーカーは本物のスレッドなので、届く順も時刻も決めうちにしない。静かになってから
/// 少し待つのは、1 つの結果が次の Task を生む経路 (worktree 選択 → 走査) があるため。
pub fn pump(ws: &mut Workspace, svc: &mut Services<TaskResult>) {
    let mut quiet = 0;
    for _ in 0..500 {
        let mut got = false;
        while let Some(event) = svc.try_recv() {
            got = true;
            let effects = match event.kind {
                EventKind::Task(result) => ws.accept(result),
                EventKind::Watch(_) => Vec::new(),
            };
            apply(ws, svc, effects);
        }
        quiet = if got { 0 } else { quiet + 1 };
        if quiet > 20 {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// worktree 一覧に 1 つだけ載せて、そこを選ばせる。
pub fn select_only_worktree(ws: &mut Workspace, svc: &mut Services<TaskResult>, path: &Path) {
    let info = WorktreeInfo {
        path: PathBuf::from(path),
        branch: "main".into(),
        is_main: true,
        added: 0,
        modified: 0,
        deleted: 0,
        staged: 0,
        is_clean: true,
        ahead: None,
        behind: None,
        head_oid: None,
        head_time: None,
    };
    let effects = ws.accept(TaskResult::Worktrees(Ok(vec![info])));
    apply(ws, svc, effects);
    pump(ws, svc);
}

/// 一時ディレクトリの git リポジトリ。origin は隣に置いた bare。
///
/// git を実際に触る経路は libgit2 と git CLI の両方を通るので、用意も CLI で揃える。
pub struct TestRepo {
    _dir: tempfile::TempDir,
    /// canonicalize 済み。macOS の /var は /private/var への symlink なので、
    /// git が返すパスと突き合わせるには実体側で持つ。
    base: PathBuf,
}

impl TestRepo {
    /// main に 1 コミットあり、origin へ push 済みのリポジトリ。
    pub fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let repo = Self { _dir: dir, base };
        let (root, origin) = (repo.root(), repo.origin());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "--bare", "-b", "main"]);
        run_git(&root, &["init", "-b", "main"]);
        run_git(&root, &["config", "user.email", "test@test.com"]);
        run_git(&root, &["config", "user.name", "Test"]);
        repo.commit_in(&root, "a.txt", "alpha\n", "first");
        run_git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        run_git(&root, &["push", "-u", "origin", "main"]);
        repo
    }

    pub fn root(&self) -> PathBuf {
        self.base.join("repo")
    }

    fn origin(&self) -> PathBuf {
        self.base.join("origin")
    }

    pub fn commit_in(&self, at: &Path, file: &str, body: &str, message: &str) {
        std::fs::write(at.join(file), body).unwrap();
        run_git(at, &["add", file]);
        run_git(at, &["commit", "-m", message]);
    }

    /// origin にだけあるブランチ。
    pub fn remote_branch(&self, name: &str) {
        run_git(
            &self.root(),
            &["push", "origin", &format!("main:refs/heads/{name}")],
        );
    }

    pub fn worktree(&self, branch: &str) -> PathBuf {
        let path = self.base.join(branch.replace('/', "-"));
        run_git(
            &self.root(),
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
        );
        path
    }

    pub fn git(&self, args: &[&str]) -> String {
        run_git(&self.root(), args)
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 一時リポジトリを開いた Workspace。worktree の一覧まで揃えて返す。
pub fn workspace_for(repo: &TestRepo) -> (Workspace, Services<TaskResult>) {
    let mut ws = Workspace::for_test();
    ws.repo.root = repo.root();
    ws.repo.main_branch = "main".into();
    let mut svc = Services::new();
    apply(
        &mut ws,
        &mut svc,
        vec![crate::effect::Effect::Spawn(
            crate::task::Task::ListWorktrees,
        )],
    );
    pump(&mut ws, &mut svc);
    (ws, svc)
}
