//! revidere: git diff を入口にした 3 段階のレビュー支援。
//!
//! 成果物は JSON 1 枚 (review.rs)。読む側が最初に触るのは
//! [annotate::Annotations] で、「この行の重要度は」を引ける形にしたもの。
//!
//! 作る側の入口は [analyze::analyze] 1 つ。AI をどう呼ぶかは持たず、ホストが
//! [analyze::Ai] を実装して渡す。モデルの選択もキャンセルもホストの関心事。

pub mod analyze;
pub mod annotate;
mod cache;
pub mod coverage;
pub mod diff;
pub mod forest;
pub mod git;
pub mod order;
mod parse;
mod prompt;
pub mod review;

pub use analyze::{analyze, Ai, AnalyzeError, Options};
pub use annotate::{Annotations, LoadError};
pub use diff::{Diff, DiffLine, FileDiff, FileKind, Hunk, Tag};
pub use forest::Forest;
pub use order::{Block, OrderedLine, PlacedSection, ReadingOrder};
pub use review::{
    Confidence, Coverage, Impact, Importance, Overview, Position, Range, Relation, Review, Section,
    Side,
};
