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

/// そのシンボルを、定義しているファイルの外から引けるか。
///
/// 種別とは別の軸になる。関数の中で宣言された型も、外からは引けない。
/// SCIP は符号の綴り (`local 4`) でこれを表し、ローカルなものを Document の
/// 中に閉じ込める (sheaf_core の is_local)。ここでも同じ軸を持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// ファイルをまたいで引ける。
    Global,
    /// 定義しているファイルの中でしか意味がない。
    Local,
}

/// tree-sitter による構文解析で見つかったシンボル定義。
#[derive(Debug, Clone)]
pub struct Symbol {
    /// シンボル名（例: "MyStruct", "my_function"）。
    pub name: String,
    /// シンボルの種類。
    pub kind: SymbolKind,
    /// ファイルの外から引けるか。
    pub scope: Scope,
    /// リポジトリルートからの相対ファイルパス。
    pub file_path: String,
    /// 1始まりの行番号。
    pub line: usize,
    /// 親の構造体/モジュール名、取得できた場合。
    pub parent: Option<String>,
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
