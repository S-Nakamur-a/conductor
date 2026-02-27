//! Diff state — data model for the Diff mode.
//!
//! Holds the parsed file-level diffs, hunk information, and line-level changes
//! produced by comparing HEAD against a base branch using `git2` and `similar`.
//! Files are split into two sections: committed (merge-base..HEAD) and
//! uncommitted (HEAD vs workdir+index).

use std::path::Path;

use anyhow::{Context, Result};
use git2::Repository;
use similar::{ChangeTag, TextDiff};

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
    /// A collapsible section header.
    SectionHeader {
        section: DiffSection,
        count: usize,
        collapsed: bool,
    },
    /// A file within a section.
    File {
        section: DiffSection,
        file_index: usize,
    },
}

// ---------------------------------------------------------------------------
// Internal diff range (replaces the old public DiffScope)
// ---------------------------------------------------------------------------

/// Which range to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffRange {
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
    /// Whether this file is newly created in HEAD.
    pub is_new: bool,
    /// Whether this file was deleted in HEAD.
    pub is_deleted: bool,
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
    /// Whether the Committed section is collapsed.
    pub committed_collapsed: bool,
    /// Whether the Uncommitted section is collapsed.
    pub uncommitted_collapsed: bool,
    /// Vertical scroll offset inside the diff content pane.
    pub scroll: usize,
    /// Current presentation mode.
    pub view_mode: DiffViewMode,
    /// The base branch we are diffing against (e.g. `"main"`).
    pub base_branch: String,
    /// Human-readable error message if the diff could not be loaded.
    pub error: Option<String>,
}

impl DiffState {
    /// Create a new, empty `DiffState`.
    pub fn new(base_branch: &str, view_mode: DiffViewMode) -> Self {
        let mut state = Self {
            committed_files: Vec::new(),
            uncommitted_files: Vec::new(),
            display_list: Vec::new(),
            committed_collapsed: false,
            uncommitted_collapsed: false,
            scroll: 0,
            view_mode,
            base_branch: base_branch.to_string(),
            error: None,
        };
        state.rebuild_display_list();
        state
    }

    /// Load the diff between `base_branch` and HEAD for the repository at
    /// `worktree_path`, replacing any previously stored diff data.
    ///
    /// Computes both committed (merge-base..HEAD) and uncommitted (HEAD vs
    /// workdir+index) diffs.
    pub fn load_diff(&mut self, worktree_path: &Path, base_branch: &str, word_diff: bool) {
        self.base_branch = base_branch.to_string();
        self.error = None;

        // Compute committed diff.
        match Self::compute_diff_range(worktree_path, base_branch, DiffRange::Committed, word_diff)
        {
            Ok(mut files) => {
                files.sort_by(|a, b| a.path.cmp(&b.path));
                self.committed_files = files;
            }
            Err(e) => {
                self.committed_files.clear();
                self.uncommitted_files.clear();
                self.error = Some(format!("{e:#}"));
                self.rebuild_display_list();
                return;
            }
        }

        // Compute uncommitted diff.
        match Self::compute_diff_range(
            worktree_path,
            base_branch,
            DiffRange::Uncommitted,
            word_diff,
        ) {
            Ok(mut files) => {
                files.sort_by(|a, b| a.path.cmp(&b.path));
                self.uncommitted_files = files;
            }
            Err(e) => {
                self.uncommitted_files.clear();
                // Non-fatal: committed diff was loaded successfully.
                log::warn!("failed to compute uncommitted diff: {e:#}");
            }
        }

        self.rebuild_display_list();
        self.scroll = 0;
    }

    /// Rebuild the flattened display list from the current file lists and
    /// collapse states.
    pub fn rebuild_display_list(&mut self) {
        self.display_list.clear();

        // Committed section.
        self.display_list.push(DiffListEntry::SectionHeader {
            section: DiffSection::Committed,
            count: self.committed_files.len(),
            collapsed: self.committed_collapsed,
        });
        if !self.committed_collapsed {
            for i in 0..self.committed_files.len() {
                self.display_list.push(DiffListEntry::File {
                    section: DiffSection::Committed,
                    file_index: i,
                });
            }
        }

        // Uncommitted section.
        self.display_list.push(DiffListEntry::SectionHeader {
            section: DiffSection::Uncommitted,
            count: self.uncommitted_files.len(),
            collapsed: self.uncommitted_collapsed,
        });
        if !self.uncommitted_collapsed {
            for i in 0..self.uncommitted_files.len() {
                self.display_list.push(DiffListEntry::File {
                    section: DiffSection::Uncommitted,
                    file_index: i,
                });
            }
        }
    }

    /// Resolve a display list index to a file reference and its section.
    ///
    /// Returns `None` for section headers or out-of-range indices.
    pub fn resolve_file(&self, display_idx: usize) -> Option<(&FileDiff, DiffSection)> {
        match self.display_list.get(display_idx)? {
            DiffListEntry::File {
                section,
                file_index,
            } => {
                let files = match section {
                    DiffSection::Committed => &self.committed_files,
                    DiffSection::Uncommitted => &self.uncommitted_files,
                };
                files.get(*file_index).map(|f| (f, *section))
            }
            DiffListEntry::SectionHeader { .. } => None,
        }
    }

    /// Toggle the collapsed state of the section at the given display index.
    ///
    /// Returns `true` if a toggle was performed.
    pub fn toggle_section(&mut self, display_idx: usize) -> bool {
        if let Some(DiffListEntry::SectionHeader { section, .. }) =
            self.display_list.get(display_idx)
        {
            match section {
                DiffSection::Committed => {
                    self.committed_collapsed = !self.committed_collapsed;
                }
                DiffSection::Uncommitted => {
                    self.uncommitted_collapsed = !self.uncommitted_collapsed;
                }
            }
            self.rebuild_display_list();
            true
        } else {
            false
        }
    }

    /// Collapse the section at the given display index.
    pub fn collapse_section(&mut self, display_idx: usize) {
        if let Some(entry) = self.display_list.get(display_idx) {
            let section = match entry {
                DiffListEntry::SectionHeader { section, .. } => Some(*section),
                DiffListEntry::File { section, .. } => Some(*section),
            };
            if let Some(section) = section {
                let collapsed = match section {
                    DiffSection::Committed => &mut self.committed_collapsed,
                    DiffSection::Uncommitted => &mut self.uncommitted_collapsed,
                };
                if !*collapsed {
                    *collapsed = true;
                    self.rebuild_display_list();
                }
            }
        }
    }

    /// Expand the section at the given display index.
    pub fn expand_section(&mut self, display_idx: usize) {
        if let Some(entry) = self.display_list.get(display_idx) {
            let section = match entry {
                DiffListEntry::SectionHeader { section, .. } => Some(*section),
                DiffListEntry::File { section, .. } => Some(*section),
            };
            if let Some(section) = section {
                let collapsed = match section {
                    DiffSection::Committed => &mut self.committed_collapsed,
                    DiffSection::Uncommitted => &mut self.uncommitted_collapsed,
                };
                if *collapsed {
                    *collapsed = false;
                    self.rebuild_display_list();
                }
            }
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────

    /// Expand tab characters to spaces, matching the viewer's tab expansion.
    pub fn expand_tabs(line: &str, tab_width: usize) -> String {
        if !line.contains('\t') {
            return line.to_string();
        }
        let mut result = String::with_capacity(line.len());
        let mut col = 0;
        for ch in line.chars() {
            if ch == '\t' {
                let spaces = tab_width - (col % tab_width);
                for _ in 0..spaces {
                    result.push(' ');
                }
                col += spaces;
            } else {
                result.push(ch);
                col += 1;
            }
        }
        result
    }

    // ── Private helpers ─────────────────────────────────────────────────

    /// Use `git2` + `similar` to compute file-level diffs for a given range.
    fn compute_diff_range(
        worktree_path: &Path,
        base_branch: &str,
        range: DiffRange,
        word_diff: bool,
    ) -> Result<Vec<FileDiff>> {
        let repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open repo at {}", worktree_path.display()))?;

        // Resolve base branch OID.
        let base_ref = repo
            .find_branch(base_branch, git2::BranchType::Local)
            .with_context(|| format!("branch '{base_branch}' not found"))?;
        let base_oid = base_ref
            .get()
            .peel_to_commit()
            .with_context(|| format!("cannot peel '{base_branch}' to commit"))?
            .id();

        // Resolve HEAD.
        let head_commit = repo
            .head()
            .with_context(|| "cannot resolve HEAD")?
            .peel_to_commit()
            .with_context(|| "cannot peel HEAD to commit")?;
        let head_oid = head_commit.id();

        // Build the git2 diff depending on range.
        let diff = match range {
            DiffRange::Committed => {
                // merge-base(base, HEAD)..HEAD
                let merge_base_oid = repo.merge_base(base_oid, head_oid).with_context(|| {
                    format!("cannot find merge-base between '{base_branch}' and HEAD")
                })?;
                let merge_base_tree = repo
                    .find_commit(merge_base_oid)?
                    .tree()
                    .with_context(|| "cannot get merge-base tree")?;
                let head_tree = head_commit.tree()?;
                repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&head_tree), None)?
            }
            DiffRange::Uncommitted => {
                // HEAD..workdir+index
                let head_tree = head_commit.tree()?;
                let mut opts = git2::DiffOptions::new();
                opts.include_untracked(true);
                opts.recurse_untracked_dirs(true);
                repo.diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))?
            }
        };

        // Determine if we need to read from workdir (for unstaged/untracked files).
        let use_workdir = range == DiffRange::Uncommitted;

        let mut file_diffs = Vec::new();

        let num_deltas = diff.deltas().len();
        for delta_idx in 0..num_deltas {
            let delta = diff.get_delta(delta_idx).unwrap();

            let status = delta.status();
            let is_new = status == git2::Delta::Added || status == git2::Delta::Untracked;
            let is_deleted = status == git2::Delta::Deleted;

            // Determine file path.
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "(unknown)".to_string());

            // Get old content from the blob.
            let old_content = Self::blob_content(&repo, &delta.old_file());

            // Get new content: for workdir diffs, read from disk when the
            // blob id is zero (unstaged / untracked).
            let new_content = if use_workdir && delta.new_file().id().is_zero() {
                let full_path = worktree_path.join(&path);
                std::fs::read_to_string(&full_path).unwrap_or_default()
            } else {
                Self::blob_content(&repo, &delta.new_file())
            };

            // Use `similar` to compute line-level diff with context.
            let text_diff = TextDiff::from_lines(&old_content, &new_content);

            let context_radius = 3;
            let mut hunks = Vec::new();
            let mut total_added = 0usize;
            let mut total_deleted = 0usize;

            for group in text_diff.grouped_ops(context_radius) {
                let mut hunk_lines = Vec::new();

                for op in &group {
                    if word_diff {
                        for inline_change in text_diff.iter_inline_changes(op) {
                            let tag = match inline_change.tag() {
                                ChangeTag::Equal => DiffLineTag::Equal,
                                ChangeTag::Insert => {
                                    total_added += 1;
                                    DiffLineTag::Insert
                                }
                                ChangeTag::Delete => {
                                    total_deleted += 1;
                                    DiffLineTag::Delete
                                }
                            };

                            let old_line_no = inline_change.old_index().map(|i| i + 1);
                            let new_line_no = inline_change.new_index().map(|i| i + 1);

                            let segments: Vec<InlineSegment> = inline_change
                                .iter_strings_lossy()
                                .map(|(emphasized, value)| InlineSegment {
                                    text: value.into_owned(),
                                    emphasized,
                                })
                                .collect();

                            // Build content by joining segment texts.
                            let content: String = segments.iter()
                                .map(|s| s.text.trim_end_matches('\n').trim_end_matches('\r'))
                                .collect::<Vec<_>>()
                                .join("");
                            let content = Self::expand_tabs(&content, 4);

                            let has_emphasis = segments.iter().any(|s| s.emphasized);
                            let inline_segments =
                                if has_emphasis { segments } else { Vec::new() };

                            hunk_lines.push(DiffLine {
                                tag,
                                old_line_no,
                                new_line_no,
                                inline_segments,
                                content,
                            });
                        }
                    } else {
                        for change in text_diff.iter_changes(op) {
                            let tag = match change.tag() {
                                ChangeTag::Equal => DiffLineTag::Equal,
                                ChangeTag::Insert => {
                                    total_added += 1;
                                    DiffLineTag::Insert
                                }
                                ChangeTag::Delete => {
                                    total_deleted += 1;
                                    DiffLineTag::Delete
                                }
                            };

                            let old_line_no = change.old_index().map(|i| i + 1);
                            let new_line_no = change.new_index().map(|i| i + 1);

                            let raw = change.value().trim_end_matches('\n').trim_end_matches('\r');
                            let content = Self::expand_tabs(raw, 4);

                            hunk_lines.push(DiffLine {
                                tag,
                                old_line_no,
                                new_line_no,
                                inline_segments: Vec::new(),
                                content,
                            });
                        }
                    }
                }

                hunks.push(DiffHunk {
                    lines: hunk_lines,
                });
            }

            file_diffs.push(FileDiff {
                path,
                added_lines: total_added,
                deleted_lines: total_deleted,
                is_new,
                is_deleted,
                hunks,
            });
        }

        Ok(file_diffs)
    }

    /// Read blob content for a diff file entry, returning an empty string if
    /// the blob is absent (new or deleted file).
    fn blob_content(repo: &Repository, file: &git2::DiffFile<'_>) -> String {
        if file.id().is_zero() {
            return String::new();
        }
        match repo.find_blob(file.id()) {
            Ok(blob) => {
                // Attempt UTF-8; fall back to lossy conversion.
                String::from_utf8(blob.content().to_vec())
                    .unwrap_or_else(|_| String::from_utf8_lossy(blob.content()).to_string())
            }
            Err(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use similar::{ChangeTag, TextDiff};

    #[test]
    fn test_inline_segments_populated_for_replace() {
        let old = "hello world\n";
        let new = "hello rust\n";
        let diff = TextDiff::from_lines(old, new);

        for op in diff.ops() {
            for change in diff.iter_inline_changes(op) {
                if change.tag() == ChangeTag::Insert {
                    let has_emphasis = change.values().iter().any(|(e, _)| *e);
                    assert!(has_emphasis, "Insert line should have emphasized segments");
                }
            }
        }
    }
}
