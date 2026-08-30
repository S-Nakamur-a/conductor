//! Diff モードのデータ型: ビューモード、フラット化した explorer 表示リスト、
//! 行/ハンク/ファイル単位の diff 構造体、そしてトップレベルの DiffState。

use std::collections::HashSet;

use crate::config::DiffView;

// ビューモード

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewMode {
    Unified,
    SideBySide,
}

impl From<DiffView> for DiffViewMode {
    fn from(v: DiffView) -> Self {
        match v {
            DiffView::Unified => DiffViewMode::Unified,
            DiffView::SideBySide => DiffViewMode::SideBySide,
        }
    }
}

// 表示リスト

/// explorer パネルに表示するフラット化リストの1エントリ。depth は 0 がトップレベル。
#[derive(Debug, Clone)]
pub enum DiffListEntry {
    Directory {
        path: String,
        /// パスの最後の要素。
        name: String,
        depth: usize,
        collapsed: bool,
    },
    /// file_index は DiffState::files への添字。
    File { file_index: usize, depth: usize },
    /// リストの最上部に固定される、ブランチの変更サマリー用の疑似ファイル。
    /// 将来メタデータを足しても既存の match アームを壊さないよう struct variant にしてある。
    Summary {},
}

// 行レベルの型

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineTag {
    Equal,
    Insert,
    Delete,
}

/// diff 行内のセグメント。変更箇所と非変更箇所を区別する。
#[derive(Debug, Clone)]
pub struct InlineSegment {
    pub text: String,
    /// 実際に行内変更があった箇所かどうか。
    pub emphasized: bool,
}

/// ハンク内の1行。
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffLineTag,
    /// 旧(ベース)側と新(HEAD)側の行番号。
    pub old_line_no: Option<usize>,
    pub new_line_no: Option<usize>,
    /// 空 Vec なら行全体をそのまま描画する。
    pub inline_segments: Vec<InlineSegment>,
    /// タブ展開済み。
    pub content: String,
}

// ハンク

/// diff 行の連続したまとまり(コンテキスト + 変更)。
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub lines: Vec<DiffLine>,
    /// 検出できた場合の関数コンテキストヘッダー(例: "fn some_function()")。
    pub func_header: Option<String>,
}

// ファイル単位の diff

/// 単一ファイルの diff 情報。行数は全ハンクの合計。
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// worktree ルートからの相対パス。
    pub path: String,
    pub added_lines: usize,
    pub deleted_lines: usize,
    pub hunks: Vec<DiffHunk>,
}

// トップレベルの diff state

/// Diff モード UI の全状態。
#[derive(Debug, Clone)]
pub struct DiffState {
    /// merge-base..workdir+index。コミット済みと未コミットを1つの diff にまとめて
    /// あるので、コミット後に再編集したファイルも1エントリのままになる。
    pub files: Vec<FileDiff>,
    pub display_list: Vec<DiffListEntry>,
    /// 素のリポジトリ相対パスをキーにする。
    pub collapsed_dirs: HashSet<String>,
    pub scroll: usize,
    pub view_mode: DiffViewMode,
    /// 例: "main"。
    pub base_branch: String,
    pub error: Option<String>,
    /// diff モデルは review state に触れないので、このフラグだけを App が
    /// ReviewState::change_summary から同期している。
    pub has_summary: bool,
}
