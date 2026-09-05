// unified diff の型。パース (parser.rs) と変更一覧 (ledger.rs) が共有する。
//
// 変更一覧と描画で別々に diff を解釈すると、色を塗れない行が「どちらの都合か」を
// 追えなくなるので、パーサは 1 つに保つ。

pub mod ledger;
pub mod parser;

pub use parser::parse;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Context,
    Add,
    Del,
}

/// ハンク内の 1 行。
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: Tag,
    /// 前像の行番号（1 始まり）。追加行では None。
    pub old_line: Option<u32>,
    /// 後像の行番号（1 始まり）。削除行では None。
    pub new_line: Option<u32>,
    /// 行の内容（先頭の +/-/空白は除いてある）。
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    /// @@ 行の後ろに付く関数コンテキスト（無ければ空）。
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// 後ろ 2 つは行を持たない変更。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Binary,
    ModeOnly,
}

/// ファイル 1 つ分の変更。
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// 後像のパス。削除ファイルでは前像のパスを入れる（呼ぶ側が必ず名前を得られるように）。
    pub path: String,
    /// rename のときだけ前像のパス。
    pub old_path: Option<String>,
    pub kind: FileKind,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub files: Vec<FileDiff>,
}
