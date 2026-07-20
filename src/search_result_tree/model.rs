//! Row and internal-node types for the search result tree.

use std::collections::BTreeMap;

/// A single visible row in the search result tree.
#[derive(Debug, Clone)]
pub enum SearchTreeRow {
    /// A directory node (e.g. `src/ui/`).
    Dir {
        /// Display path (e.g. `"src"`, `"ui"`).
        name: String,
        /// Depth in the tree (0 = top-level).
        depth: usize,
        /// Whether this directory is expanded.
        expanded: bool,
        /// Total match count under this directory (recursive).
        match_count: usize,
    },
    /// A file node (e.g. `app.rs (3 matches)`).
    File {
        /// Display name (leaf component).
        name: String,
        /// Full relative path (for opening the file).
        path: String,
        /// Depth in the tree.
        depth: usize,
        /// Whether this file is expanded (showing match lines).
        expanded: bool,
        /// Number of matches in this file.
        match_count: usize,
    },
    /// A match line within a file.
    Match {
        /// Depth in the tree.
        depth: usize,
        /// Index into the original `GrepMatch` list.
        match_index: usize,
    },
}

/// A directory's files, keyed by filename, each mapping to its match indices
/// in [`SearchResultTree::matches`](super::tree::SearchResultTree). Shared
/// with [`tree`](super::tree) which builds and reads it directly.
pub(crate) struct DirNode {
    pub(crate) files: BTreeMap<String, Vec<usize>>, // filename → match indices
}
