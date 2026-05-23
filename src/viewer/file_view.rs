//! Unified diff view types.

use crate::diff_state::{DiffLineTag, InlineSegment};

/// An entry in the unified diff view.
#[derive(Debug, Clone)]
pub enum UnifiedDiffEntry {
    /// A separator between hunks (used when no lines are hidden).
    HunkSeparator { func_header: Option<String> },
    /// An expandable context block representing hidden lines between hunks.
    ExpandableContext {
        /// Number of currently hidden lines.
        hidden_count: usize,
        /// First hidden line number in the new file (1-indexed).
        new_line_start: usize,
        /// Last hidden line number in the new file (1-indexed, inclusive).
        new_line_end: usize,
        /// Function context header for the next hunk (displayed alongside).
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
