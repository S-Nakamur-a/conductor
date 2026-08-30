//! grep 検索用の、ツリー構造を持つ検索結果モデル。
//!
//! GrepMatch のフラットなリストを、ディレクトリ→ファイル→マッチという
//! 階層構造に変換し、展開/折りたたみをサポートする。

mod helpers;
mod model;
mod tree;

pub use model::SearchTreeRow;
pub use tree::SearchResultTree;

#[cfg(test)]
mod tests;
