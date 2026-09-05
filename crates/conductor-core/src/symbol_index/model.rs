//! 索引が返すデータ型。

/// シンボルの種類。
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
/// 種別とは別の軸。関数の中で宣言された型も外からは引けない。
/// SCIP が `local 4` の綴りで表すものと同じ軸で、sheaf_core の is_local に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Local,
}

/// 構文解析で見つかったシンボル定義。
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub scope: Scope,
    /// 索引ルートからの相対パス。
    pub file_path: String,
    /// 1 始まり。
    pub line: usize,
    /// impl ブロックなら対象の型名。
    pub parent: Option<String>,
}

/// テキスト検索で見つかった参照。
#[derive(Debug, Clone)]
pub struct Reference {
    /// 索引ルートからの相対パス。
    pub file_path: String,
    /// 1 始まり。
    pub line: usize,
    pub content: String,
}
