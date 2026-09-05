//! libgit2 による git 操作。
//!
//! worktree の列挙とステータス、ブランチの系譜、cherry-pick / merge / pull、
//! grab によるブランチの入れ替えを [GitEngine] のメソッドとして提供する。
//! 認証が要る fetch と worktree の作成だけは git CLI に委ねる (各ファイルの理由を参照)。

mod branch_lineage;
mod cherry_pick;
mod commit_log;
mod fetch;
mod grab;
mod merge;
mod recently_modified;
pub mod status_map;
#[cfg(test)]
mod tests;
mod worktree_create;
mod worktree_delete;
mod worktree_ops;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::Repository;

pub use grab::GrabState;
pub use recently_modified::recently_modified_files;
pub use status_map::{GitStatusMap, TreeGitState};

/// このリポジトリの .conductor ディレクトリ。
///
/// linked worktree から起動されてもメイン worktree 側を指す。ここに置く DB・
/// ソケット・ロックはリポジトリ単位で 1 つなので、worktree ごとに分かれると
/// 相手が見えなくなる。
pub fn conductor_dir(repo_path: &Path) -> PathBuf {
    GitEngine::open(repo_path)
        .and_then(|e| e.main_worktree_path())
        .unwrap_or_else(|_| repo_path.to_path_buf())
        .join(".conductor")
}

/// 1 つの worktree のブランチと変更件数。
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub is_main: bool,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    /// added/modified/deleted と意図的に重複する。あちらはファイルを 1 回だけ
    /// 数えるので git add しても合計が動かず、これが無いとステージ操作を
    /// 観測できる信号が消える (ファイルウォッチャーは .git/ を見ない)。
    pub staged: usize,
    pub is_clean: bool,
    /// upstream に対する ahead / behind。upstream が無ければ None。
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub head_oid: Option<String>,
    /// HEAD の committer 時刻 (Unix 秒)。revidere の成果物が HEAD より古いかの
    /// 判定に使う。
    pub head_time: Option<i64>,
}

/// 1 つのコミットの要約。
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// 先頭 8 文字。
    pub short_oid: String,
    pub oid: String,
    /// コミットメッセージの 1 行目。
    pub message: String,
    pub author: String,
    pub time_ago: String,
}

/// ブランチの系譜と PR 情報。
#[derive(Debug, Clone, Default)]
pub struct BranchDetails {
    /// このブランチの作成元。
    pub initial_branch: Option<String>,
    pub derived_branches: Vec<String>,
    pub pr_url: Option<String>,
    pub pr_loading: bool,
}

/// conductor 向けの操作をまとめた git2::Repository のラッパー。
pub struct GitEngine {
    repo: Repository,
}

impl GitEngine {
    /// main worktree、linked worktree、そのサブディレクトリのどれを指していても開ける。
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::discover(path).with_context(|| {
            format!("failed to discover git repository from {}", path.display())
        })?;
        Ok(Self { repo })
    }
}
