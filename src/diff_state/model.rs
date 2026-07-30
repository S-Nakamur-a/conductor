//! Data types for the Diff mode: view mode, the flattened explorer display
//! list, line/hunk/file diff structures, and the top-level `DiffState`.

use std::collections::HashSet;

use crate::config::DiffView;

// ---------------------------------------------------------------------------
// View mode
// ---------------------------------------------------------------------------

/// How the diff content is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewMode {
    Unified,
    SideBySide,
}

impl From<DiffView> for DiffViewMode {
    fn from(v: DiffView) -> Self {
        match v {
            DiffView::Unified => DiffViewMode::Unified,
            DiffView::SideBySide => DiffViewMode::SideBySide,
        }
    }
}

// ---------------------------------------------------------------------------
// Section / display list
// ---------------------------------------------------------------------------

/// Which section a diff file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSection {
    Committed,
    Uncommitted,
}

/// An entry in the flattened display list shown in the explorer panel.
#[derive(Debug, Clone)]
pub enum DiffListEntry {
    /// A directory node in the merged change tree (collapsible).
    Directory {
        /// The directory path (e.g. "src/ui").
        path: String,
        /// Display name (last component).
        name: String,
        /// Nesting depth (0 = top-level).
        depth: usize,
        /// Whether this directory is collapsed.
        collapsed: bool,
    },
    /// A changed file. `section` records its origin (committed vs uncommitted)
    /// both for the row's C/U marker and to resolve back into the right list.
    File {
        section: DiffSection,
        file_index: usize,
        /// Nesting depth (0 = top-level file).
        depth: usize,
    },
    /// The branch change-summary pseudo-file, pinned at the very top of the
    /// list. Selecting it opens the full summary in the Viewer. Struct-variant
    /// form so future metadata (e.g. freshness) can be added without breaking
    /// existing match arms.
    Summary {},
}

// ---------------------------------------------------------------------------
// Internal diff range (replaces the old public DiffScope)
// ---------------------------------------------------------------------------

/// Which range to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffRange {
    /// merge-base(base, HEAD)..HEAD — committed changes only.
    Committed,
    /// HEAD..workdir+index — uncommitted changes only.
    Uncommitted,
}

// ---------------------------------------------------------------------------
// Line-level types
// ---------------------------------------------------------------------------

/// Tag indicating whether a diff line is context, an addition, or a deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineTag {
    Equal,
    Insert,
    Delete,
}

/// A segment within a diff line, distinguishing changed vs unchanged portions.
#[derive(Debug, Clone)]
pub struct InlineSegment {
    /// The text content of this segment.
    pub text: String,
    /// Whether this segment is emphasized (i.e., the actual intra-line change).
    pub emphasized: bool,
}

/// A single line inside a hunk.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffLineTag,
    /// Line number in the old (base) file, if applicable.
    pub old_line_no: Option<usize>,
    /// Line number in the new (HEAD) file, if applicable.
    pub new_line_no: Option<usize>,
    /// Intra-line change segments. Empty vec = fallback to whole-line rendering.
    pub inline_segments: Vec<InlineSegment>,
    /// The text content of this line (tab-expanded).
    pub content: String,
}

// ---------------------------------------------------------------------------
// Hunk
// ---------------------------------------------------------------------------

/// A contiguous group of diff lines (context + changes).
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// The lines that make up this hunk.
    pub lines: Vec<DiffLine>,
    /// Function context header (e.g. "fn some_function()"), if detected.
    pub func_header: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-file diff
// ---------------------------------------------------------------------------

/// Diff information for a single file.
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// File path (relative to the worktree root).
    pub path: String,
    /// Number of added lines across all hunks.
    pub added_lines: usize,
    /// Number of deleted lines across all hunks.
    pub deleted_lines: usize,
    /// Parsed hunks with context.
    pub hunks: Vec<DiffHunk>,
}

// ---------------------------------------------------------------------------
// Top-level diff state
// ---------------------------------------------------------------------------

/// All state for the Diff mode UI.
#[derive(Debug, Clone)]
pub struct DiffState {
    /// Committed changes (merge-base..HEAD).
    pub committed_files: Vec<FileDiff>,
    /// Uncommitted changes (HEAD vs workdir+index).
    pub uncommitted_files: Vec<FileDiff>,
    /// Flattened display list for the explorer panel.
    pub display_list: Vec<DiffListEntry>,
    /// Set of collapsed directory paths (keyed by plain repo-relative path).
    pub collapsed_dirs: HashSet<String>,
    /// Vertical scroll offset inside the diff content pane.
    pub scroll: usize,
    /// Current presentation mode.
    pub view_mode: DiffViewMode,
    /// The base branch we are diffing against (e.g. `"main"`).
    pub base_branch: String,
    /// Human-readable error message if the diff could not be loaded.
    pub error: Option<String>,
    /// Whether the current branch has a change summary. When `true`, a
    /// `DiffListEntry::Summary` pseudo-file is pinned at the top of the display
    /// list. Synced by the App from `ReviewState::change_summary` (the diff
    /// model can't reach review state directly, so it caches just this flag).
    pub has_summary: bool,
}
