//! In-file text search and fuzzy filename search.

use super::file_view::UnifiedDiffEntry;
use super::state::ViewerState;

/// The new-file line number a diff entry represents, for entries that map to
/// one: a concrete `Line` (`None` for a deletion, which has no new-file
/// line), or an `ExpandableContext`'s first hidden line. `HunkSeparator`
/// carries no line number.
fn diff_entry_new_line_no(entry: &UnifiedDiffEntry) -> Option<usize> {
    match entry {
        UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
        UnifiedDiffEntry::ExpandableContext { new_line_start, .. } => Some(*new_line_start),
        UnifiedDiffEntry::HunkSeparator { .. } => None,
    }
}

impl ViewerState {
    /// Execute a search over the file content and populate search_matches.
    pub fn execute_search(&mut self) {
        self.search.search_matches.clear();
        self.search.search_match_idx = 0;

        if self.search.search_query.is_empty() {
            return;
        }

        let query_lower = self.search.search_query.to_lowercase();
        for (i, line) in self.content.file_content.iter().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                self.search.search_matches.push(i);
            }
        }

        // Jump to first match at or after current scroll.
        if !self.search.search_matches.is_empty() {
            self.search.search_match_idx = self
                .search
                .search_matches
                .iter()
                .position(|&line| line >= self.content.file_scroll)
                .unwrap_or(0);
            self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        }
        self.sync_diff_scroll_to_file_scroll();
    }

    /// Jump to the next search match.
    pub fn next_search_match(&mut self) {
        if self.search.search_matches.is_empty() {
            return;
        }
        self.search.search_match_idx =
            (self.search.search_match_idx + 1) % self.search.search_matches.len();
        self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        self.sync_diff_scroll_to_file_scroll();
    }

    /// Jump to the previous search match.
    pub fn prev_search_match(&mut self) {
        if self.search.search_matches.is_empty() {
            return;
        }
        self.search.search_match_idx = if self.search.search_match_idx == 0 {
            self.search.search_matches.len() - 1
        } else {
            self.search.search_match_idx - 1
        };
        self.content.file_scroll = self.search.search_matches[self.search.search_match_idx];
        self.sync_diff_scroll_to_file_scroll();
    }

    /// Resolve the file line (0-indexed, matching `content.file_scroll`) that
    /// the diff view's current scroll position corresponds to, from the
    /// nearest concrete new-file line number at or after `diff_view_scroll`
    /// (falling back to the nearest one before it — e.g. when the cursor
    /// sits on a deleted line, which has no new-file line number).
    fn diff_scroll_file_line(&self) -> Option<usize> {
        let lines = &self.diff_view.diff_view_lines;
        let scroll = self.diff_view.diff_view_scroll.min(lines.len());
        lines[scroll..]
            .iter()
            .find_map(diff_entry_new_line_no)
            .or_else(|| {
                lines[..scroll]
                    .iter()
                    .rev()
                    .find_map(diff_entry_new_line_no)
            })
            .map(|n| n.saturating_sub(1))
    }

    /// Keep `content.file_scroll` in sync with the diff view's scroll
    /// position. Symbol lookup and search operate on `content.file_scroll`
    /// unconditionally (they predate diff mode), so anything reached while
    /// browsing a diff needs this synced first, or it would act on whatever
    /// line plain-view browsing last left `file_scroll` at. A no-op outside
    /// diff mode.
    pub fn sync_file_scroll_to_diff_scroll(&mut self) {
        if !self.diff_view.diff_mode {
            return;
        }
        if let Some(line) = self.diff_scroll_file_line() {
            self.content.file_scroll = line;
        }
    }

    /// Keep the diff view's scroll position in sync with `content.file_scroll`
    /// after it moves on its own (e.g. a search match) so the diff pane
    /// visibly follows along instead of staying put while the underlying
    /// cursor moves. A no-op outside diff mode.
    fn sync_diff_scroll_to_file_scroll(&mut self) {
        if !self.diff_view.diff_mode {
            return;
        }
        let target_line = self.content.file_scroll + 1; // new_line_no is 1-indexed
        if let Some(idx) = self
            .diff_view
            .diff_view_lines
            .iter()
            .position(|entry| diff_entry_new_line_no(entry).is_some_and(|n| n >= target_line))
        {
            self.diff_view.diff_view_scroll = idx;
        }
    }

    // -- Filename fuzzy search ------------------------------------------------

    /// Run fuzzy filename search over the cached file list and populate results.
    pub fn execute_filename_search(&mut self) {
        self.filename_search.filename_search_results.clear();

        let query = self.filename_search.filename_search_query.to_lowercase();

        for path in &self.filename_search.filename_search_all_files {
            let path_lower = path.to_lowercase();
            let name_lower = path.rsplit('/').next().unwrap_or(path).to_lowercase();

            // If query is empty, include all files with score 0.
            if query.is_empty() {
                self.filename_search
                    .filename_search_results
                    .push(super::file_tree::ScoredFile {
                        path: path.clone(),
                        score: 0,
                    });
                continue;
            }

            // Check fuzzy subsequence match first — skip non-matching files.
            if !Self::is_fuzzy_match(&query, &path_lower) {
                continue;
            }

            let mut score: i32 = 10; // Base score for fuzzy match.

            // Bonus: consecutive character matches.
            score += Self::consecutive_bonus(&query, &path_lower);

            // Bonus: filename exact prefix.
            if name_lower.starts_with(&query) {
                score += 100;
            }

            // Bonus: path substring match.
            if path_lower.contains(&query) {
                score += 50;
            }

            // Bonus: filename substring match.
            if name_lower.contains(&query) {
                score += 30;
            }

            // Bonus: word boundary match (char after '/', '_', '-', '.').
            if Self::has_word_boundary_match(&query, &path_lower) {
                score += 20;
            }

            self.filename_search
                .filename_search_results
                .push(super::file_tree::ScoredFile {
                    path: path.clone(),
                    score,
                });
        }

        // Sort by score descending, then path ascending for stability.
        self.filename_search
            .filename_search_results
            .sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    }

    /// Check if all characters of `query` appear in `haystack` in order.
    fn is_fuzzy_match(query: &str, haystack: &str) -> bool {
        let mut haystack_chars = haystack.chars();
        for qc in query.chars() {
            if !haystack_chars.any(|hc| hc == qc) {
                return false;
            }
        }
        true
    }

    /// Award bonus points for consecutive matching characters.
    fn consecutive_bonus(query: &str, haystack: &str) -> i32 {
        let mut bonus = 0i32;
        let mut consecutive = 0;
        let mut hay_iter = haystack.chars().peekable();

        for qc in query.chars() {
            let mut found = false;
            for hc in hay_iter.by_ref() {
                if hc == qc {
                    consecutive += 1;
                    if consecutive > 1 {
                        bonus += consecutive;
                    }
                    found = true;
                    break;
                }
                consecutive = 0;
            }
            if !found {
                break;
            }
        }
        bonus
    }

    /// Check if query characters match at word boundaries in the haystack
    /// (after '/', '_', '-', '.', or at position 0).
    fn has_word_boundary_match(query: &str, haystack: &str) -> bool {
        let boundary_chars: Vec<char> = haystack
            .char_indices()
            .filter(|&(i, _)| {
                if i == 0 {
                    return true;
                }
                let prev = haystack.as_bytes().get(i - 1).copied().unwrap_or(0);
                matches!(prev, b'/' | b'_' | b'-' | b'.')
            })
            .map(|(_, c)| c)
            .collect();

        if boundary_chars.len() < query.len() {
            return false;
        }

        let mut bi = 0;
        for qc in query.chars() {
            let mut found = false;
            while bi < boundary_chars.len() {
                if boundary_chars[bi] == qc {
                    bi += 1;
                    found = true;
                    break;
                }
                bi += 1;
            }
            if !found {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_state::DiffLineTag;

    /// Builds a `Line` entry with the given new-file line number (`None` for
    /// a deletion, which has no new-file line).
    fn diff_line(new_line_no: Option<usize>) -> UnifiedDiffEntry {
        UnifiedDiffEntry::Line {
            tag: DiffLineTag::Equal,
            new_line_no,
            content: String::new(),
            inline_segments: Vec::new(),
        }
    }

    #[test]
    fn sync_file_scroll_to_diff_scroll_resolves_deletion_lines_forward() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = true;
        vs.diff_view.diff_view_lines = vec![
            UnifiedDiffEntry::HunkSeparator { func_header: None },
            diff_line(Some(10)), // idx 1
            diff_line(None),     // idx 2 — a deletion, no new-file line
            diff_line(Some(11)), // idx 3
        ];

        // Scrolled onto the deletion: no new-file line at this exact index,
        // so the cursor resolves forward to the next concrete line (11).
        vs.diff_view.diff_view_scroll = 2;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 10); // line 11, 0-indexed

        // Scrolled directly onto a concrete line: resolves to that line.
        vs.diff_view.diff_view_scroll = 1;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 9); // line 10, 0-indexed
    }

    #[test]
    fn sync_file_scroll_to_diff_scroll_is_noop_outside_diff_mode() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = false;
        vs.diff_view.diff_view_lines = vec![diff_line(Some(5))];
        vs.diff_view.diff_view_scroll = 0;
        vs.content.file_scroll = 42;
        vs.sync_file_scroll_to_diff_scroll();
        assert_eq!(vs.content.file_scroll, 42);
    }

    #[test]
    fn sync_diff_scroll_to_file_scroll_follows_a_search_jump() {
        let mut vs = ViewerState::default();
        vs.diff_view.diff_mode = true;
        vs.diff_view.diff_view_lines = vec![
            diff_line(Some(1)), // idx 0
            diff_line(Some(2)), // idx 1
            diff_line(Some(3)), // idx 2
        ];
        vs.diff_view.diff_view_scroll = 0;

        // A search match landed on file_scroll = 2 (line 3, 0-indexed).
        vs.content.file_scroll = 2;
        vs.sync_diff_scroll_to_file_scroll();
        assert_eq!(vs.diff_view.diff_view_scroll, 2);
    }
}
