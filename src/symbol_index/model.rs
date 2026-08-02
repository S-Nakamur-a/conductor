//! シンボルインデックスが生成する公開データ型: シンボル定義、その種類、
//! テキスト検索による参照。

/// シンボルの種類（関数、構造体、トレイトなど）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Struct,
    Trait,
    Impl,
    Type,
    Const,
    Module,
    Enum,
    EnumVariant,
    Field,
    Method,
    Macro,
    Static,
    Interface,
}

/// tree-sitter による構文解析で見つかったシンボル定義。
#[derive(Debug, Clone)]
pub struct Symbol {
    /// シンボル名（例: "MyStruct", "my_function"）。
    pub name: String,
    /// シンボルの種類。
    pub kind: SymbolKind,
    /// リポジトリルートからの相対ファイルパス。
    pub file_path: String,
    /// 1始まりの行番号。
    pub line: usize,
    /// スコープ（親の構造体/モジュール名など）、取得できた場合。
    pub scope: Option<String>,
}

/// テキスト検索で見つかったシンボルの参照（使用箇所）。
#[derive(Debug, Clone)]
pub struct Reference {
    /// リポジトリルートからの相対ファイルパス。
    pub file_path: String,
    /// 1始まりの行番号。
    pub line: usize,
    /// その行の全文。
    pub content: String,
}
