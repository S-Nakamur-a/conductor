//! libgit2 による git 操作。
//!
//! worktree の一覧、ステータス件数、コミット情報、diff 生成など、
//! リポジトリ調査のための git2 の高レベルインターフェースを提供する。
//! いずれのサブモジュールも共有の [GitEngine] にメソッドを実装している。

mod branch_lineage;
mod cherry_pick;
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

pub use recently_modified::recently_modified_files;

/// このリポジトリの .conductor ディレクトリ。
///
/// linked worktree から起動されても、メインワークツリー側を指す。ここに置く
/// データベース・ソケット・ロックはリポジトリ単位で 1 つなので、worktree ごとに
/// 分かれると相手が見えなくなる。
pub fn conductor_dir(repo_path: &Path) -> PathBuf {
    GitEngine::open(repo_path)
        .and_then(|e| e.main_worktree_path())
        .unwrap_or_else(|_| repo_path.to_path_buf())
        .join(".conductor")
}

/// 1つの worktree に関する情報。
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub is_main: bool,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    /// added/modified/deleted と意図的に重複する。あちらは各ファイルを1回だけ
    /// 数えるので、git add しても3つの合計は動かない。staged を別に数えないと
    /// ステージを観測できる信号が無くなり、Explorer の色分けが更新されなくなる
    /// (ファイルウォッチャーは .git/ を無視し、3秒ポーリングはこの数値しか見ない)。
    pub staged: usize,
    pub is_clean: bool,
    /// upstream に対して ahead / behind なコミット数。upstream が無ければ None。
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    /// HEAD コミットの OID(16進)。worktree ごとに2回目の Repository::open を
    /// させないため、repo を開いているタイミングで取っている。
    pub head_oid: Option<String>,
    /// HEAD コミットの committer 時刻 (Unix 秒)。
    ///
    /// revidere の成果物が HEAD より古いかの判定に使う。oid で突き合わせないのは、
    /// 解析時の oid をどこにも書き残していないため。
    pub head_time: Option<i64>,
}

/// 1つのコミットの要約情報。
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// 先頭8文字。
    pub short_oid: String,
    pub oid: String,
    /// コミットメッセージの1行目。
    pub message: String,
    pub author: String,
    pub time_ago: String,
}

/// 詳細パネル用の、ブランチの系譜と PR 情報。
#[derive(Debug, Clone, Default)]
pub struct BranchDetails {
    /// このブランチの作成元となったブランチ。
    pub initial_branch: Option<String>,
    pub derived_branches: Vec<String>,
    pub pr_url: Option<String>,
    pub pr_loading: bool,
}

/// conductor 固有のヘルパーを公開する、git2::Repository のラッパー。
pub struct GitEngine {
    repo: Repository,
}

impl GitEngine {
    /// 既存のリポジトリを開く。main worktree、linked worktree、そのサブ
    /// ディレクトリのいずれを指していても動く。
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::discover(path).with_context(|| {
            format!("failed to discover git repository from {}", path.display())
        })?;
        Ok(Self { repo })
    }
}
