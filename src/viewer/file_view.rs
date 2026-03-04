//! Unified diff view types.

use crate::diff_state::{DiffLineTag, InlineSegment};

/// An entry in the unified diff view.
#[derive(Debug, Clone)]
pub enum UnifiedDiffEntry {
    /// A separator between hunks.
    HunkSeparator {
        func_header: Option<String>,
    },
    /// A single line (context, addition, or deletion).
    Line {
        tag: DiffLineTag,
        /// Line number in the new file. `Some` for Equal/Insert, `None` for Delete.
        new_line_no: Option<usize>,
        /// The text content of this line.
        content: String,
        /// Intra-line change segments (word diff).
        inline_segments: Vec<InlineSegment>,
    },
}
