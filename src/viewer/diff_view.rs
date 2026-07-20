//! Unified diff view — building the entry list from a `FileDiff`, expanding
//! hidden context regions, and the diff-mode / summary-view toggles.

use crate::diff_state::{DiffHunk, DiffLineTag, FileDiff};

use super::file_view::UnifiedDiffEntry;
use super::state::ViewerState;

impl ViewerState {
    /// Return the total gutter width (in columns) used by the line-number
    /// area in the viewer panel.  The gutter consists of:
    ///   prefix(1) + digits(gutter_width) + space(1) + '│'(1) + space(1)
    /// = gutter_width + 4
    pub fn gutter_total_width(&self) -> u16 {
        let digit_w = if self.diff_view.diff_mode {
            // Must match the renderer's gutter width exactly, or mouse hit-testing
            // (badge/thread toggles, symbol jumps) drifts off by a column. The
            // renderer uses `diff_view_max_line_no`, which also counts the
            // `new_line_end` of collapsed (ExpandableContext) regions — those can
            // out-digit every *visible* line, so recomputing from `Line` entries
            // alone here would under-count and shift every click target left.
            digit_count(self.diff_view.diff_view_max_line_no)
        } else {
            digit_count(self.content.file_content.len())
        };
        (digit_w + 4) as u16
    }

    // -- Unified diff view ----------------------------------------------------

    /// Build the unified diff view entries from a `FileDiff`.
    ///
    /// Inserts `ExpandableContext` entries between hunks to represent hidden
    /// context lines that can be expanded on demand.
    pub fn build_unified_diff_view(&mut self, file_diff: &FileDiff) {
        self.diff_view.diff_view_lines.clear();

        let total_new_lines = self.content.file_content.len();

        // Helper: find the max new_line_no in a hunk.
        let hunk_max_new_line = |hunk: &DiffHunk| -> usize {
            hunk.lines
                .iter()
                .filter_map(|l| l.new_line_no)
                .max()
                .unwrap_or(0)
        };
        // Helper: find the min new_line_no in a hunk.
        let hunk_min_new_line = |hunk: &DiffHunk| -> usize {
            hunk.lines
                .iter()
                .filter_map(|l| l.new_line_no)
                .min()
                .unwrap_or(0)
        };

        for (hunk_idx, hunk) in file_diff.hunks.iter().enumerate() {
            if hunk_idx == 0 {
                // Before the first hunk: check for hidden lines at top of file.
                let first_new = hunk_min_new_line(hunk);
                if first_new > 1 {
                    let hidden_start = 1;
                    let hidden_end = first_new - 1;
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::ExpandableContext {
                            hidden_count: hidden_end - hidden_start + 1,
                            new_line_start: hidden_start,
                            new_line_end: hidden_end,
                            func_header: hunk.func_header.clone(),
                        });
                }
            } else {
                // Between hunks: compute hidden range.
                let prev_hunk = &file_diff.hunks[hunk_idx - 1];
                let prev_end = hunk_max_new_line(prev_hunk);
                let curr_start = hunk_min_new_line(hunk);
                let hidden_start = prev_end + 1;
                let hidden_end = curr_start.saturating_sub(1);
                if hidden_start <= hidden_end {
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::ExpandableContext {
                            hidden_count: hidden_end - hidden_start + 1,
                            new_line_start: hidden_start,
                            new_line_end: hidden_end,
                            func_header: hunk.func_header.clone(),
                        });
                } else {
                    // No hidden lines — keep a visual separator.
                    self.diff_view
                        .diff_view_lines
                        .push(UnifiedDiffEntry::HunkSeparator {
                            func_header: hunk.func_header.clone(),
                        });
                }
            }

            for line in &hunk.lines {
                self.diff_view.diff_view_lines.push(UnifiedDiffEntry::Line {
                    tag: line.tag,
                    new_line_no: line.new_line_no,
                    content: line.content.clone(),
                    inline_segments: line.inline_segments.clone(),
                });
            }
        }

        // After the last hunk: check for hidden lines at bottom of file.
        if let Some(last_hunk) = file_diff.hunks.last() {
            let last_new = hunk_max_new_line(last_hunk);
            if last_new < total_new_lines {
                let hidden_start = last_new + 1;
                let hidden_end = total_new_lines;
                self.diff_view
                    .diff_view_lines
                    .push(UnifiedDiffEntry::ExpandableContext {
                        hidden_count: hidden_end - hidden_start + 1,
                        new_line_start: hidden_start,
                        new_line_end: hidden_end,
                        func_header: None,
                    });
            }
        }

        self.recalc_diff_max_line_no();

        if !self.diff_view.diff_view_lines.is_empty() {
            self.diff_view.diff_mode = true;
            self.diff_view.diff_view_scroll = 0;
        }
    }

    /// Recalculate the cached max line number from current diff view lines.
    fn recalc_diff_max_line_no(&mut self) {
        self.diff_view.diff_view_max_line_no = self
            .diff_view
            .diff_view_lines
            .iter()
            .filter_map(|e| match e {
                UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
                UnifiedDiffEntry::ExpandableContext { new_line_end, .. } => Some(*new_line_end),
                _ => None,
            })
            .max()
            .unwrap_or(0);
    }

    /// Return the maximum line width (in characters) of the current content.
    ///
    /// In diff mode this scans `diff_view_lines`; otherwise it scans
    /// `file_content`. Returns 0 when there is nothing to display.
    pub fn max_content_width(&self) -> usize {
        if self.diff_view.diff_mode {
            self.diff_view
                .diff_view_lines
                .iter()
                .map(|entry| match entry {
                    UnifiedDiffEntry::Line { content, .. } => content.chars().count(),
                    UnifiedDiffEntry::HunkSeparator { func_header }
                    | UnifiedDiffEntry::ExpandableContext { func_header, .. } => {
                        func_header.as_ref().map_or(0, |h| h.chars().count())
                    }
                })
                .max()
                .unwrap_or(0)
        } else {
            self.content
                .file_content
                .iter()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
        }
    }

    /// Increase `h_scroll` by `delta`, clamping so the view never scrolls
    /// past the longest line in the current content.
    pub fn scroll_right(&mut self, delta: usize) {
        let max_w = self.max_content_width();
        // Allow scrolling until only a few characters remain visible.
        let limit = max_w.saturating_sub(4);
        self.content.h_scroll = (self.content.h_scroll + delta).min(limit);
    }

    /// Exit unified diff mode and reset related state. Also leaves the summary
    /// pseudo-file view — every file-open path funnels through here, so this is
    /// the single place that guarantees `show_summary` and `diff_mode` are never
    /// both set.
    pub fn exit_diff_mode(&mut self) {
        self.diff_view.diff_mode = false;
        self.diff_view.diff_view_lines.clear();
        self.diff_view.diff_view_scroll = 0;
        self.diff_view.diff_view_max_line_no = 0;
        self.show_summary = false;
        self.summary_scroll = 0;
    }

    /// Whether the viewer is currently showing the summary pseudo-file.
    pub fn is_summary(&self) -> bool {
        self.show_summary
    }

    /// Enter the summary pseudo-file view, leaving any diff/file content. Kept
    /// mutually exclusive with diff mode via `exit_diff_mode`.
    pub fn enter_summary_view(&mut self) {
        self.exit_diff_mode();
        self.show_summary = true;
        self.summary_scroll = 0;
    }

    /// Expand hidden context lines at the given index in `diff_view_lines`.
    ///
    /// If `expand_all` is true, all hidden lines are revealed. Otherwise,
    /// up to 10 lines are revealed — 5 from the top and 5 from the bottom
    /// of the hidden range (GitHub-style bidirectional expansion).
    /// Returns `true` if expansion occurred.
    pub fn expand_context_at(&mut self, idx: usize, expand_all: bool) -> bool {
        let entry = match self.diff_view.diff_view_lines.get(idx) {
            Some(UnifiedDiffEntry::ExpandableContext { .. }) => {
                self.diff_view.diff_view_lines[idx].clone()
            }
            _ => return false,
        };

        let (hidden_count, new_line_start, new_line_end, func_header) = match entry {
            UnifiedDiffEntry::ExpandableContext {
                hidden_count,
                new_line_start,
                new_line_end,
                func_header,
            } => (hidden_count, new_line_start, new_line_end, func_header),
            _ => unreachable!(),
        };

        if expand_all || hidden_count <= 10 {
            // Reveal all hidden lines.
            let mut new_entries: Vec<UnifiedDiffEntry> = Vec::with_capacity(hidden_count);
            for line_no in new_line_start..=new_line_end {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            let added = new_entries.len();
            self.diff_view
                .diff_view_lines
                .splice(idx..=idx, new_entries);

            if idx < self.diff_view.diff_view_scroll {
                let delta = added.saturating_sub(1);
                self.diff_view.diff_view_scroll += delta;
            }
        } else {
            // Bidirectional: reveal 5 from top + 5 from bottom.
            let top_count = 5usize;
            let bottom_count = 5usize;

            let mut new_entries: Vec<UnifiedDiffEntry> =
                Vec::with_capacity(top_count + bottom_count + 1);

            // Top lines (immediately after previous hunk).
            for line_no in new_line_start..new_line_start + top_count {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            // Smaller ExpandableContext for the remaining middle.
            let remaining_start = new_line_start + top_count;
            let remaining_end = new_line_end - bottom_count;
            new_entries.push(UnifiedDiffEntry::ExpandableContext {
                hidden_count: remaining_end - remaining_start + 1,
                new_line_start: remaining_start,
                new_line_end: remaining_end,
                func_header,
            });

            // Bottom lines (immediately before next hunk).
            for line_no in (new_line_end - bottom_count + 1)..=new_line_end {
                let content = self
                    .content
                    .file_content
                    .get(line_no - 1)
                    .cloned()
                    .unwrap_or_default();
                new_entries.push(UnifiedDiffEntry::Line {
                    tag: DiffLineTag::Equal,
                    new_line_no: Some(line_no),
                    content,
                    inline_segments: Vec::new(),
                });
            }

            let added = new_entries.len();
            self.diff_view
                .diff_view_lines
                .splice(idx..=idx, new_entries);

            if idx < self.diff_view.diff_view_scroll {
                let delta = added.saturating_sub(1);
                self.diff_view.diff_view_scroll += delta;
            }
        }

        self.recalc_diff_max_line_no();
        true
    }

    /// Find the first `ExpandableContext` entry visible in the current viewport
    /// and return its index.
    pub fn find_visible_expandable(&self, viewport_height: usize) -> Option<usize> {
        let start = self.diff_view.diff_view_scroll;
        let end = (start + viewport_height).min(self.diff_view.diff_view_lines.len());
        for i in start..end {
            if matches!(
                self.diff_view.diff_view_lines.get(i),
                Some(UnifiedDiffEntry::ExpandableContext { .. })
            ) {
                return Some(i);
            }
        }
        None
    }
}

/// Count the number of decimal digits in `n` (minimum 1).
fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}
