//! grep 検索用の、ツリー構造を持つ検索結果モデル。
//!
//! GrepMatch のフラットなリストを、ディレクトリ→ファイル→マッチという
//! 階層構造に変換し、展開/折りたたみをサポートする。
//!
//! 責務ごとに分割している。[model] は行/ノードの型を、[helpers] は
//! フラットなパス群からネストしたツリーを組み立てる処理を、[tree] は
//! [SearchResultTree] 自体(構築、行のフラット化、展開/折りたたみ/移動)を
//! 持つ。

mod helpers;
mod model;
mod tree;

pub use model::SearchTreeRow;
pub use tree::SearchResultTree;

#[cfg(test)]
mod tests;
