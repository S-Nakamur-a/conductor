//! Explorer walkthrough-view methods for [`App`]: step selection, jumping to
//! a step's location in the diff pane, and the "viewed" file toggle. These
//! back the Explorer's `Walkthrough` bottom-pane view (see
//! `viewer::ExplorerBottomView`) and the diff list's per-file viewed mark.

use super::*;

impl App {
    /// Move the walkthrough step cursor by `delta` rows (`j`/`k`), clamped to
    /// the step list's bounds. Selection only — no jump, unlike `n`/`N`.
    pub fn walkthrough_move(&mut self, delta: isize) {
        let Some(len) = self
            .current_walkthrough
            .as_ref()
            .map(|(_, steps)| steps.len())
        else {
            return;
        };
        if len == 0 {
            return;
        }
        let cur = self.viewer_state.explorer.walkthrough_selected as isize;
        self.viewer_state.explorer.walkthrough_selected =
            (cur + delta).clamp(0, len as isize - 1) as usize;
    }

    /// Jump to the currently selected walkthrough step (`Enter`): open its
    /// file in the diff pane and move focus to the Viewer, mirroring the diff
    /// list's own Enter handling so the two views feel consistent.
    pub fn walkthrough_jump_selected(&mut self) {
        let idx = self.viewer_state.explorer.walkthrough_selected;
        if !self.jump_to_walkthrough_step(idx) {
            return;
        }
        self.set_focus(Focus::Viewer);
    }

    /// Move the walkthrough selection by `delta` and jump immediately
    /// (`n`/`N` while the Walkthrough view is focused). Stays on the
    /// Walkthrough view, unlike `walkthrough_jump_selected`, so repeated
    /// presses keep paging through steps without the diff pane taking
    /// keyboard focus away from the step list.
    pub fn walkthrough_step(&mut self, delta: isize) {
        let Some(len) = self
            .current_walkthrough
            .as_ref()
            .map(|(_, steps)| steps.len())
        else {
            return;
        };
        if len == 0 {
            return;
        }
        let cur = self.viewer_state.explorer.walkthrough_selected as isize;
        let next = (cur + delta).clamp(0, len as isize - 1) as usize;
        self.jump_to_walkthrough_step(next);
    }

    /// Shared implementation for jumping to walkthrough step `idx`: opens its
    /// file in the diff pane, scrolls to its starting line, and marks it
    /// viewed. Returns `false` (a no-op otherwise) if there's no walkthrough,
    /// the index is out of range, or the step's file isn't part of the
    /// current diff.
    fn jump_to_walkthrough_step(&mut self, idx: usize) -> bool {
        let Some((_, steps)) = &self.current_walkthrough else {
            return false;
        };
        let Some(step) = steps.get(idx) else {
            return false;
        };
        let file_path = step.file_path.clone();
        let line_start = step.line_start;
        let step_id = step.id.clone();

        let Some((file_diff, _)) = self
            .diff_state
            .display_index_for_path(&file_path)
            .and_then(|i| self.diff_state.resolve_file(i))
        else {
            self.set_status(
                format!("Walkthrough step references a file not in this diff: {file_path}"),
                StatusLevel::Warning,
            );
            return false;
        };
        let file_diff_clone = file_diff.clone();
        let Some(wt) = self.worktrees.get(self.selected_worktree) else {
            return false;
        };
        let wt_path = wt.path.clone();
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&wt_path, &file_path, tab_width);
        self.viewer_state.reveal_file_in_tree(&file_path, &wt_path);
        self.rehighlight_viewer();
        self.review_state.build_file_comment_cache(&file_path);
        self.expand_threads_for_file(&file_path);
        self.viewer_state.build_unified_diff_view(&file_diff_clone);
        if let Some(list_idx) = self.diff_state.display_index_for_path(&file_path) {
            self.viewer_state.explorer.diff_list_selected = list_idx;
        }

        let target = line_start
            .and_then(|line| {
                self.viewer_state.diff_view.diff_view_lines.iter().position(|e| {
                    matches!(e, crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(n), .. } if *n as i64 == line)
                })
            })
            .or_else(|| {
                self.viewer_state.diff_view.diff_view_lines.iter().position(|e| {
                    matches!(e, crate::viewer::UnifiedDiffEntry::Line { tag, .. } if *tag != crate::diff_state::DiffLineTag::Equal)
                })
            });
        if let Some(pos) = target {
            self.viewer_state.diff_view.diff_view_scroll = pos.saturating_sub(3);
        }

        self.viewer_state.explorer.walkthrough_selected = idx;
        // The Viewer now reflects this step: its banner and line-range
        // underline follow `walkthrough_viewing`, not the list cursor, so a
        // later `j`/`k` that only moves the cursor won't disturb the Viewer.
        self.viewer_state.explorer.walkthrough_viewing = Some(idx);
        self.viewer_state.explorer.viewed_steps.insert(step_id);
        true
    }

    /// Toggle the "viewed" mark for a file path — used by the diff list's `v`
    /// key and the Viewer's diff-mode `v` key (Section C).
    pub fn toggle_path_viewed(&mut self, path: &str) {
        let viewed = &mut self.viewer_state.explorer.viewed;
        if !viewed.remove(path) {
            viewed.insert(path.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    /// A manually-saved walkthrough (mirroring what the headless generator
    /// writes) round-trips through the store with the shape the walkthrough
    /// UI and jump logic depend on: `WalkthroughStep::kind` parses back to
    /// the same variant, line ranges survive as `Some`, and the file path
    /// resolves through `DiffState::display_index_for_path` — the lookup
    /// `jump_to_walkthrough_step` uses to find the step's file in the diff.
    #[test]
    fn saved_walkthrough_round_trips_for_ui_consumption() {
        use crate::diff_state::{DiffListEntry, DiffSection, DiffState, DiffViewMode, FileDiff};
        use crate::review_store::ReviewStore;
        use crate::walkthrough::{NewWalkthroughStep, WalkthroughStatus, WalkthroughStepKind};

        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(&dir.path().join("conductor.db")).unwrap();
        store.begin_walkthrough("feature-x", None).unwrap();
        store
            .save_walkthrough(
                "feature-x",
                "Add feature X",
                "Wires up feature X end to end.",
                &[
                    NewWalkthroughStep {
                        file_path: "src/a.rs".to_string(),
                        line_start: None,
                        line_end: None,
                        kind: WalkthroughStepKind::Intent,
                        title: "Why".to_string(),
                        body: "Motivation.".to_string(),
                    },
                    NewWalkthroughStep {
                        file_path: "src/a.rs".to_string(),
                        line_start: Some(10),
                        line_end: Some(12),
                        kind: WalkthroughStepKind::Core,
                        title: "Core change".to_string(),
                        body: "What changed.".to_string(),
                    },
                ],
            )
            .unwrap();

        let (walkthrough, steps) = store.get_walkthrough("feature-x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Ready);
        assert_eq!(steps.len(), 2);
        let core = &steps[1];
        assert_eq!(core.kind, WalkthroughStepKind::Core);
        assert_eq!(core.line_start, Some(10));
        assert_eq!(core.line_end, Some(12));
        assert!(!core.id.is_empty());

        // The jump path's file lookup: the step's file must resolve through
        // the diff list exactly like `jump_to_walkthrough_step` requires.
        let mut ds = DiffState::new("main", DiffViewMode::Unified);
        ds.committed_files = vec![FileDiff {
            path: core.file_path.clone(),
            added_lines: 3,
            deleted_lines: 0,
            is_new: false,
            is_deleted: false,
            hunks: Vec::new(),
        }];
        ds.display_list = vec![DiffListEntry::File {
            section: DiffSection::Committed,
            file_index: 0,
            depth: 0,
        }];
        assert_eq!(ds.display_index_for_path(&core.file_path), Some(0));
    }
}
