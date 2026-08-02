//! 検索結果ツリーの行型と内部ノード型。

use std::collections::BTreeMap;

/// 検索結果ツリーで表示される1行。
#[derive(Debug, Clone)]
pub enum SearchTreeRow {
    /// ディレクトリノード(例: src/ui/)。
    Dir {
        /// 表示パス(例: "src", "ui")。
        name: String,
        /// ツリー内の深さ(0 = トップレベル)。
        depth: usize,
        /// このディレクトリが展開されているか。
        expanded: bool,
        /// このディレクトリ配下の合計マッチ数(再帰的)。
        match_count: usize,
    },
    /// ファイルノード(例: app.rs (3 matches))。
    File {
        /// 表示名(リーフ部分)。
        name: String,
        /// ファイルを開くための完全な相対パス。
        path: String,
        /// ツリー内の深さ。
        depth: usize,
        /// このファイルが展開されている(マッチ行を表示中)か。
        expanded: bool,
        /// このファイル内のマッチ数。
        match_count: usize,
    },
    /// ファイル内の1マッチ行。
    Match {
        /// ツリー内の深さ。
        depth: usize,
        /// 元の GrepMatch リストへのインデックス。
        match_index: usize,
    },
}

/// あるディレクトリのファイル群。ファイル名をキーに、
/// [SearchResultTree::matches](super::tree::SearchResultTree) 内の
/// マッチインデックス一覧へマッピングする。構築と直接の読み取りを行う
/// [tree](super::tree) と共有する。
pub(crate) struct DirNode {
    pub(crate) files: BTreeMap<String, Vec<usize>>, // ファイル名 → マッチインデックス
}
