//! ファイルツリーの型定義 — FileTreeEntry と ScoredFile。

use crate::git_engine::status_map::TreeGitState;
use crate::icons::FileIcon;

/// ファイル名のあいまい検索でマッチしたファイルと、そのスコア。
#[derive(Debug, Clone)]
pub struct ScoredFile {
    /// ファイルの相対パス。
    pub path: String,
    /// あいまい検索のスコア（高いほどマッチ度が高い）。
    pub score: i32,
}

/// フラット化されたファイルツリー中の1エントリ。
#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    /// worktree ルートからの相対パス（例: "src/main.rs"）。
    pub path: String,
    /// 表示名 — パスの最後の要素。
    pub name: String,
    /// ネストの深さ（トップレベルのエントリは0）。
    pub depth: usize,
    /// このエントリがディレクトリかどうか。
    pub is_dir: bool,
    /// ディレクトリエントリが現在展開されているかどうか（ファイルでは無視される）。
    pub is_expanded: bool,
    /// このディレクトリの子要素がツリーに読み込み済みかどうか。
    /// ファイルでは常に false。ディレクトリは false から始まり、ファイルシステムから
    /// 子要素を読み込んだ後に true になる。
    pub children_loaded: bool,
    /// このエントリのアイコン（生成時に一度だけ計算する）。字形の選択は描画時まで
    /// 遅延するので、これは文字セットに依存しない。
    pub icon: FileIcon,
    /// tracked/untracked/ignored の別。ツリーを（再）構築した時点の git status
    /// スナップショットに基づく — Explorer の減光表示に使う。
    pub git_state: TreeGitState,
}
