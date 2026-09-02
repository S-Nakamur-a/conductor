//! パネル。1 パネル 1 ディレクトリで、状態・update・render を同居させる。
//!
//! update は自分の状態しか `&mut` で受け取らないので、他パネルへの影響は
//! [crate::effect::Effect] でしか表現できない。消費しなかった Action は `None` を
//! 返し、[crate::route::global_effects] の既定の解釈に落ちる。

pub mod explorer;
pub mod terminal;
pub mod viewer;
pub mod worktree;
