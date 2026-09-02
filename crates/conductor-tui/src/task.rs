//! svc に投げる仕事と、その結果の語彙。svc は中身を知らない。

use std::path::{Path, PathBuf};

use conductor_core::diff_state::DiffState;
use conductor_core::git_engine::{GitEngine, WorktreeInfo};
use conductor_svc::Services;

use crate::panels::explorer::tree;
use crate::panels::viewer::content;

/// Task が git を触るのに要るもの。Workspace が持っているので、Task 自身は運ばない。
#[derive(Debug, Clone)]
pub struct TaskEnv {
    pub root: PathBuf,
    pub main_branch: String,
    pub worktree_dir: Option<PathBuf>,
    pub word_diff: bool,
    pub tab_width: usize,
}

#[derive(Debug)]
pub enum Task {
    ListWorktrees,
    CreateWorktree {
        branch: String,
    },
    DeleteWorktree {
        path: PathBuf,
        branch: String,
    },
    LoadTree {
        root: PathBuf,
        expanded: Vec<String>,
    },
    ComputeDiff {
        worktree: PathBuf,
    },
    /// `seq` は結果の照合に使う。同じファイルを続けて開いても古い結果が新しい本文を
    /// 上書きしない。
    LoadFile {
        root: PathBuf,
        path: String,
        seq: u64,
    },
}

#[derive(Debug)]
pub enum TaskResult {
    Worktrees(Result<Vec<WorktreeInfo>, String>),
    /// 作成できた worktree のパスと、そのブランチ。
    WorktreeCreated(Result<(PathBuf, String), String>),
    WorktreeDeleted(Result<String, String>),
    Tree(Box<tree::Snapshot>),
    /// DiffState は失敗の理由を自分の中に持つので Result にしない。
    Diff(Box<DiffState>),
    FileLoaded {
        seq: u64,
        loaded: Result<content::Loaded, String>,
    },
}

impl Task {
    pub fn spawn(self, svc: &mut Services<TaskResult>, env: &TaskEnv) {
        let env = env.clone();
        match self {
            Task::ListWorktrees => {
                svc.spawn(move || list_worktrees(&env), TaskResult::Worktrees);
            }
            Task::CreateWorktree { branch } => {
                svc.spawn(
                    move || create_worktree(&env, &branch),
                    TaskResult::WorktreeCreated,
                );
            }
            Task::DeleteWorktree { path, branch } => {
                svc.spawn(
                    move || delete_worktree(&env, &path, &branch),
                    TaskResult::WorktreeDeleted,
                );
            }
            Task::LoadTree { root, expanded } => {
                svc.spawn(
                    move || Box::new(tree::survey(&root, &expanded)),
                    TaskResult::Tree,
                );
            }
            Task::ComputeDiff { worktree } => {
                svc.spawn(
                    move || Box::new(compute_diff(&env, &worktree)),
                    TaskResult::Diff,
                );
            }
            Task::LoadFile { root, path, seq } => {
                svc.spawn(
                    move || (seq, content::read(&root, &path, env.tab_width)),
                    |(seq, loaded)| TaskResult::FileLoaded { seq, loaded },
                );
            }
        }
    }
}

fn list_worktrees(env: &TaskEnv) -> Result<Vec<WorktreeInfo>, String> {
    GitEngine::open(&env.root)
        .and_then(|git| git.list_worktrees())
        .map_err(|e| e.to_string())
}

fn create_worktree(env: &TaskEnv, branch: &str) -> Result<(PathBuf, String), String> {
    let git = GitEngine::open(&env.root).map_err(|e| e.to_string())?;
    let base = git.resolve_base_ref(&env.main_branch);
    // 分岐していれば触らないので、失敗しても作成そのものは続けられる。
    if let Err(e) = git.ensure_base_ref_available(&env.main_branch) {
        log::warn!("could not fast-forward the base ref: {e:#}");
    }
    git.create_worktree_from_base(branch, &base, env.worktree_dir.as_deref())
        .map(|path| (path, branch.to_string()))
        .map_err(|e| e.to_string())
}

fn delete_worktree(env: &TaskEnv, path: &Path, branch: &str) -> Result<String, String> {
    let git = GitEngine::open(&env.root).map_err(|e| e.to_string())?;
    git.remove_worktree(path).map_err(|e| e.to_string())?;
    // ブランチが消せなくても worktree は消えている。報告するのは worktree の方。
    if let Err(e) = git.delete_branch(branch, true) {
        log::warn!("removed the worktree but could not delete branch '{branch}': {e:#}");
    }
    Ok(branch.to_string())
}

fn compute_diff(env: &TaskEnv, worktree: &Path) -> DiffState {
    let mut diff = DiffState::new(&env.main_branch);
    diff.load_diff(worktree, &env.main_branch, env.word_diff, env.tab_width);
    diff
}
