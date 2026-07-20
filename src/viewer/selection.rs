//! Line selection for comments — click / shift-click range selection over
//! the gutter.

use super::state::{LineSelection, ViewerState};

impl ViewerState {
    /// Clear the current line selection.
    pub fn clear_selection(&mut self) {
        self.selection = LineSelection::None;
    }

    /// Return the selected range as `(start, end)` (both 1-indexed, inclusive,
    /// normalized so start <= end). Returns `None` if no line is selected.
    pub fn selected_range(&self) -> Option<(usize, usize)> {
        match self.selection {
            LineSelection::None => None,
            LineSelection::Pending { start } => Some((start, start)),
            LineSelection::Selected { start, end } => Some(if start <= end {
                (start, end)
            } else {
                (end, start)
            }),
        }
    }

    /// Check whether a 1-indexed line number falls within the current
    /// selection range.
    pub fn is_line_selected(&self, line_1indexed: usize) -> bool {
        if let Some((start, end)) = self.selected_range() {
            line_1indexed >= start && line_1indexed <= end
        } else {
            false
        }
    }

    /// Whether the selection is in the pending state (first click done, waiting
    /// for second).
    pub fn is_selection_pending(&self) -> bool {
        matches!(self.selection, LineSelection::Pending { .. })
    }

    /// Handle a click on the gutter "+" button (GitHub-style commenting).
    ///
    /// A plain click selects just `line_1indexed`; a shift-click extends a
    /// range from the previously clicked line (the anchor, kept fixed so
    /// successive shift-clicks grow from the same origin). The caller then
    /// opens the comment input, which reads the resulting `selection`.
    pub fn gutter_comment_click(&mut self, line_1indexed: usize, extend: bool) {
        let anchor = self.click.last_line_click_line;
        if extend && anchor != 0 {
            let (start, end) = if anchor <= line_1indexed {
                (anchor, line_1indexed)
            } else {
                (line_1indexed, anchor)
            };
            self.selection = LineSelection::Selected { start, end };
        } else {
            self.selection = LineSelection::Selected {
                start: line_1indexed,
                end: line_1indexed,
            };
            self.click.last_line_click_line = line_1indexed;
            self.click.last_line_click_time = std::time::Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_click_selects_single_line() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(7, false);
        assert_eq!(vs.selected_range(), Some((7, 7)));
    }

    #[test]
    fn shift_gutter_click_extends_range_from_anchor() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(5, false); // anchor at 5
        vs.gutter_comment_click(9, true); // shift-click extends to 9
        assert_eq!(vs.selected_range(), Some((5, 9)));
    }

    #[test]
    fn shift_gutter_click_normalizes_upward_range() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(9, false); // anchor at 9
        vs.gutter_comment_click(4, true); // shift-click above the anchor
        assert_eq!(vs.selected_range(), Some((4, 9)));
    }

    #[test]
    fn shift_gutter_click_without_anchor_falls_back_to_single_line() {
        let mut vs = ViewerState::default();
        // No prior click → anchor is the default 0, so this is just a single line.
        vs.gutter_comment_click(3, true);
        assert_eq!(vs.selected_range(), Some((3, 3)));
    }
}
