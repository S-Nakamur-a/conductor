//! revidere: git diff を入口にした 3 段階のレビュー支援。
//!
//! 成果物は JSON 1 枚 (review.rs)。読む側が最初に触るのは
//! [annotate::Annotations] で、「この行の重要度は」を引ける形にしたもの。
//!
//! 解析 (AI の呼び出し・プロンプト・応答の解釈) は revidere-cli 側にあり、
//! ここには入っていない。読む側が解析の都合を背負わずに済むようにするため。

pub mod annotate;
pub mod coverage;
pub mod diff;
pub mod forest;
pub mod git;
pub mod order;
pub mod review;

pub use annotate::{Annotations, LoadError};
pub use diff::{Diff, DiffLine, FileDiff, FileKind, Hunk, Tag};
pub use forest::Forest;
pub use order::{Block, OrderedLine, PlacedSection, ReadingOrder};
pub use review::{
    Confidence, Coverage, Impact, Importance, Overview, Position, Range, Relation, Review, Section,
    Side,
};
