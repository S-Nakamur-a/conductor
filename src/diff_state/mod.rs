//! Diff state — Diff モードのためのデータモデル。
//!
//! git2 と similar を使って HEAD とベースブランチを比較した結果として得られる、
//! ファイル単位の diff、ハンク情報、行単位の変更を保持する。
//! ファイルはコミット済み(merge-base..HEAD)と未コミット(HEAD vs workdir+index)の
//! 2セクションに分かれる。
//!
//! 責務ごとに分割している: [model] はデータ型(DiffState とその構成要素)、
//! [display_list] はフラット化した explorer 表示リストの構築とナビゲーション、
//! [compute] は git2/similar ベースの diff 計算を担う。

mod compute;
mod display_list;
mod model;
#[cfg(test)]
mod tests;

pub use model::{
    DiffHunk, DiffLineTag, DiffListEntry, DiffSection, DiffState, DiffViewMode, FileDiff,
    InlineSegment,
};
// #[cfg(test)] のコード(review_publish のテスト)からしか参照されないため、
// 通常ビルドでは未使用と判定される。
#[allow(unused_imports)]
pub use model::DiffLine;
