//! Diff state — data model for the Diff mode.
//!
//! Holds the parsed file-level diffs, hunk information, and line-level changes
//! produced by comparing HEAD against a base branch using `git2` and `similar`.
//! Files are split into two sections: committed (merge-base..HEAD) and
//! uncommitted (HEAD vs workdir+index).
//!
//! Split by responsibility: [`model`] holds the data types (`DiffState` and
//! its building blocks), [`display_list`] builds/navigates the flattened
//! explorer display list, and [`compute`] does the `git2`/`similar`-based
//! diff computation.

mod compute;
mod display_list;
mod model;
#[cfg(test)]
mod tests;

pub use model::{
    DiffHunk, DiffLineTag, DiffListEntry, DiffSection, DiffState, DiffViewMode, FileDiff,
    InlineSegment,
};
// Referenced only from #[cfg(test)] code (review_publish tests), so the plain
// build sees it as unused.
#[allow(unused_imports)]
pub use model::DiffLine;
