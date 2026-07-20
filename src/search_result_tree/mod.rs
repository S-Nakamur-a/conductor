//! Tree-structured search result model for grep search.
//!
//! Converts a flat list of `GrepMatch` results into a directory→file→match
//! hierarchy with expand/collapse support.
//!
//! Split by responsibility: [`model`] holds the row/node types, [`helpers`]
//! the flat-paths-to-nested-tree builder, and [`tree`] the [`SearchResultTree`]
//! itself (construction, row flattening, and expand/collapse/navigation).

mod helpers;
mod model;
mod tree;

pub use model::SearchTreeRow;
pub use tree::SearchResultTree;

#[cfg(test)]
mod tests;
