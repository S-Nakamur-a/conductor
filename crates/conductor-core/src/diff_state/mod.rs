//! ある出どころ ([DiffSource]) の diff のデータモデル。
//!
//! コミット済みと未コミットを 1 本の diff にまとめるので、1 ファイルは常に 1 エントリ。
//! 変更ファイルは explorer に出すディレクトリツリー (折りたたみ付き) に平坦化される。

mod compute;
mod display_list;
mod source;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub use source::{DiffSource, short_oid};

/// diff の表示スタイル。config の `[diff] default_view` と viewer の切り替えが共有する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffView {
    Unified,
    SideBySide,
}

/// explorer に出す平坦化リストの 1 行。depth は 0 がトップレベル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffListEntry {
    Directory {
        path: String,
        /// パスの最後の要素。
        name: String,
        depth: usize,
        collapsed: bool,
    },
    /// file_index は [DiffState::files] への添字。
    File { file_index: usize, depth: usize },
    /// リストの最上部に固定される、ブランチの変更サマリーを開くための疑似ファイル。
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineTag {
    Equal,
    Insert,
    Delete,
}

/// 単語 diff で分割した行内の一片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSegment {
    pub text: String,
    /// 行内で実際に変わった箇所か。
    pub emphasized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub tag: DiffLineTag,
    pub old_line_no: Option<usize>,
    pub new_line_no: Option<usize>,
    /// 空なら行全体をそのまま描く。
    pub inline_segments: Vec<InlineSegment>,
    /// タブ展開済み。
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub lines: Vec<DiffLine>,
    /// 検出できたときの関数コンテキスト (例: "fn some_function()")。
    pub func_header: Option<String>,
}

/// 1 ファイルの diff。行数は全ハンクの合計で、バイナリは 0 のまま一覧に残る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// worktree ルートからの相対パス。
    pub path: String,
    pub added_lines: usize,
    pub deleted_lines: usize,
    pub hunks: Vec<DiffHunk>,
}

/// Viewer が差分として開く 1 ファイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDiff {
    pub source: DiffSource,
    pub file: FileDiff,
}

#[derive(Debug, Clone)]
pub struct DiffState {
    /// パス順。
    pub files: Vec<FileDiff>,
    pub display_list: Vec<DiffListEntry>,
    /// リポジトリ相対のディレクトリパス。
    pub collapsed_dirs: HashSet<String>,
    pub source: DiffSource,
    /// diff 全体を計算できなかった理由、またはベースを解決できず HEAD 基準に落ちた理由。
    pub error: Option<String>,
    /// 変更サマリーがあるときだけ [DiffListEntry::Summary] を先頭に出す。
    pub has_summary: bool,
}

impl DiffState {
    pub fn new(source: DiffSource) -> Self {
        Self {
            files: Vec::new(),
            display_list: Vec::new(),
            collapsed_dirs: HashSet::new(),
            source,
            error: None,
            has_summary: false,
        }
    }
}
