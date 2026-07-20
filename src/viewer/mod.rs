//! Viewer state — file tree model and file content buffer.
//!
//! Manages the state for the Viewer mode: a hierarchical file tree built from
//! the filesystem (skipping `.git` directories) and the content of the
//! currently selected file.
//!
//! [`ViewerState`] and its sub-structs live in [`state`]; behavior is split
//! by concern across the other submodules ([`content`] for opening files,
//! [`tree`] for walking/expanding the file tree, [`search`] for in-file and
//! filename search, [`diff_view`] for the unified diff view, [`highlight`]
//! for syntect highlighting, [`selection`] for gutter line selection).

mod content;
mod diff_view;
mod file_tree;
mod file_view;
mod highlight;
mod search;
mod selection;
mod state;
mod tree;

pub use file_tree::{FileTreeEntry, file_icon};
pub use file_view::UnifiedDiffEntry;
pub use state::*;
