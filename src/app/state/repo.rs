//! いまどのリポジトリを見ているか。

use std::path::PathBuf;

/// 開いているリポジトリの同一性と、切り替え先の候補。
pub struct RepoState {
    /// 見ているリポジトリの作業ディレクトリ。
    ///
    /// 常に worktree のルート (.conductor/conductor.db はここを基準に
    /// 探されて、無ければここに作られる)。
    pub path: PathBuf,
    /// メインリポジトリの表示名 (メイン worktree のディレクトリ名)。
    pub main_name: String,
    /// 既知のリポジトリパス一覧 (いま開いているものを含む)。
    pub known: Vec<PathBuf>,
    /// known のうち、いま開いているものの添字。
    pub known_index: usize,
}
