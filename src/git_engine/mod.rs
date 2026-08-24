//! libgit2 による git 操作。
//!
//! worktree の一覧、ステータス件数、コミット情報、diff 生成など、
//! リポジトリ調査のための git2 の高レベルインターフェースを提供する。
//!
//! 機能は責務ごとに兄弟サブモジュールへ分割されており、いずれも共有の
//! [GitEngine] ハンドルにメソッドを実装している:
//!
//! - `worktree_ops`: worktree/ブランチの列挙とステータススナップショット
//! - `worktree_create`: worktree の作成(ベース ref、リモートブランチ、
//!   または fetch 済みブランチから)とベース ref の鮮度確認
//! - `worktree_delete`: ブランチ削除と worktree の削除/整理
//! - `grab`: wt grab/wt ungrab のブランチ入れ替えワークフロー
//! - `branch_lineage`: 親/派生ブランチの検出と PR URL の構築
//! - `fetch`: git fetch へのシェルアウト
//! - `merge`: pull、main へのマージ、origin へのハードリセット
//! - `cherry_pick`: コミットの一覧取得と worktree への cherry-pick
//! - `recently_modified`: worktree の最近変更されたファイルパス
//! - `status_map`: Explorer のファイルツリー(薄暗い表示)と Changed
//!   files 一覧(ステージ状態の色分け)で共有される、パス -> git status
//!   のルックアップ

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
/// linked worktree から起動されても、メインワークツリー側の 1 つを指す。
/// ここに置くリソース (データベース、ソケット、ロック) はリポジトリ単位で
/// 1 つなので、worktree ごとに分かれてしまうと相手が見えなくなる。
pub fn conductor_dir(repo_path: &Path) -> PathBuf {
    GitEngine::open(repo_path)
        .and_then(|e| e.main_worktree_path())
        .unwrap_or_else(|_| repo_path.to_path_buf())
        .join(".conductor")
}

/// 1つの worktree に関する情報。
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// worktree のルートディレクトリの絶対パス。
    pub path: PathBuf,
    /// この worktree でチェックアウト中のブランチ名(例: "main", "feature-x")。
    pub branch: String,
    /// main(bare/primary)worktree かどうか。
    pub is_main: bool,
    /// 新規追加された(untracked または index-new な)ファイルの数。
    pub added: usize,
    /// 変更されたファイルの数(index または working directory)。
    pub modified: usize,
    /// 削除されたファイルの数(index または working directory)。
    pub deleted: usize,
    /// index に何かステージされているファイルの数。
    ///
    /// added/modified/deleted と意図的に重複する。あちらは各ファイルを
    /// 1回だけ数え、index 側を先にチェックするので、変更済みファイルに
    /// git add してもそのファイルは working-directory 側の分岐から
    /// index 側の分岐へ移るだけで、3つの合計値はすべて同じままになる。
    /// staged を別に数えなければ、ステージが起きたことを観測できる信号が
    /// なくなり、Explorer のステージ状態の色分けが更新されなくなる:
    /// ファイルウォッチャーは .git/ を無視するので git add はイベントを
    /// 一切発生させず、3秒ごとのポーリングはこの3つの数値しか比較しない。
    pub staged: usize,
    /// working directory にコミットされていない変更が無いとき true。
    pub is_clean: bool,
    /// upstream に対して ahead なコミット数(まだ push していないローカル
    /// コミット)。upstream が無ければ None。
    pub ahead: Option<usize>,
    /// upstream に対して behind なコミット数(まだ pull していないリモート
    /// コミット)。upstream が無ければ None。
    pub behind: Option<usize>,
    /// HEAD コミットの OID(16進)。unborn ブランチでは None。呼び出し側が
    /// 新しいコミットを検出するためだけに worktree ごとに2回目の
    /// Repository::open をしなくて済むよう、repo をすでに開いている
    /// タイミングで取得する。
    pub head_oid: Option<String>,
    /// HEAD コミットの committer 時刻 (Unix 秒)。unborn ブランチでは None。
    ///
    /// revidere の成果物がいまの HEAD より古いかの判定に使う。oid を突き
    /// 合わせないのは、解析時の oid をどこにも書き残していないため。時刻
    /// なら成果物のファイル自身が持っていて、conductor を再起動しても、
    /// 端末から直接 revidere を走らせても同じように判定できる。
    pub head_time: Option<i64>,
}

/// 1つのコミットの要約情報。
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// 短い16進 OID(先頭8文字)。
    pub short_oid: String,
    /// 完全な16進 OID。
    pub oid: String,
    /// コミットメッセージの1行目。
    pub message: String,
    /// コミット作者名。
    pub author: String,
    /// 人が読める形式のタイムスタンプ。
    pub time_ago: String,
}

/// 詳細パネル用の、ブランチの系譜と PR 情報。
#[derive(Debug, Clone, Default)]
pub struct BranchDetails {
    /// このブランチの作成元となったベース(最初の)ブランチ。
    pub initial_branch: Option<String>,
    /// このブランチから分岐したブランチ。
    pub derived_branches: Vec<String>,
    /// このブランチの GitHub PR URL(gh 経由で取得)。
    pub pr_url: Option<String>,
    /// PR URL の検索が現在進行中かどうか。
    pub pr_loading: bool,
}

/// conductor 固有のヘルパーを公開する、git2::Repository のラッパー。
pub struct GitEngine {
    repo: Repository,
}

impl GitEngine {
    // 構築

    /// 指定したパスから探索して、既存のリポジトリを開く。
    ///
    /// path が main worktree、linked worktree、あるいはそのどちらかの
    /// サブディレクトリのいずれを指していても動作する。
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::discover(path).with_context(|| {
            format!("failed to discover git repository from {}", path.display())
        })?;
        Ok(Self { repo })
    }
}
