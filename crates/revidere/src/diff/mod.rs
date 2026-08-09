// unified diff の型。パース (parser.rs) と台帳 (ledger.rs) が共有する。
//
// 台帳と描画で別々に diff を解釈すると、色を塗れない行が「どちらの都合か」を
// 追えなくなるので、パーサは 1 つに保つ。

pub mod ledger;
pub mod parser;

pub use parser::parse;

/// 行の種別。
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

/// ハンク 1 つ。
#[derive(Debug, Clone)]
pub struct Hunk {
    /// @@ 行の後ろに付く関数コンテキスト（無ければ空）。
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// ファイルの変更の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// 行を持たない変更。バイナリ、モードのみ。
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

/// diff 全体。
#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub files: Vec<FileDiff>,
}

impl Diff {
    pub fn file(&self, path: &str) -> Option<&FileDiff> {
        self.files.iter().find(|f| f.path == path)
    }
}
