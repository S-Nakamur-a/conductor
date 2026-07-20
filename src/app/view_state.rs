//! Viewer/diff refresh and the persisted "where the user was" view state
//! (open file + scroll position) per worktree branch.

use super::focus::Focus;
use super::{App, PendingViewRestore, StatusLevel};

impl App {
    /// Reload the viewer file tree for the currently selected worktree.
    ///
    /// Preserves the currently open file and scroll position so that
    /// file-watcher refreshes don't disrupt the user's view.
    ///
    /// Returns `true` when the file tree's visible entries changed. Uses
    /// [`Self::selected_worktree_path`], which falls back to `repo_path` when
    /// there is no worktree, so the Explorer still shows the current folder's
    /// contents in a plain (non-git) directory.
    pub fn refresh_viewer(&mut self) -> bool {
        let path = self.selected_worktree_path();
        let tab_width = self.config.viewer.tab_width;
        let changed = self.viewer_state.load_file_tree(&path, tab_width);
        // Startup restore: this is the lazy (synchronous) tree-load path
        // (e.g. first time the viewer is focused), so re-open any pending
        // file here. The async worktree-switch path does this in
        // `poll_worktree_switch_ops`.
        self.consume_pending_view_restore();
        self.rehighlight_viewer();
        changed
    }

    /// Restore the previously selected worktree and seed its saved view
    /// (open file + scroll) for the current repo. Safe to call when nothing
    /// was persisted — it just leaves the defaults in place.
    ///
    /// Used at startup and when switching repos. The worktree list is already
    /// populated synchronously by [`App::refresh_worktrees`], so the selection
    /// is restored without a frame of flicker. The file itself is restored
    /// lazily once its tree loads (see [`App::consume_pending_view_restore`]).
    pub fn restore_selected_worktree_and_view(&mut self) {
        // Restore which worktree was selected (fall back to current on miss).
        let saved_branch = self
            .review_store
            .as_ref()
            .and_then(|s| s.get_selected_worktree().ok().flatten());
        if let Some(branch) = saved_branch
            && let Some(idx) = self.worktrees.iter().position(|w| w.branch == branch)
        {
            self.selected_worktree = idx;
        }

        // Point the worktree-list cursor at the restored worktree.
        self.rebuild_worktree_list_rows();
        let sel = self.selected_worktree;
        if let Some(pos) = self
            .worktree_list_rows
            .iter()
            .position(|r| matches!(r, super::WorktreeListRow::Worktree(i) if *i == sel))
        {
            self.worktree_list_selected = pos;
        }

        // Track the loaded worktree and seed its saved file/scroll.
        let branch = self.selected_worktree_branch();
        self.pending_view_restore = None;
        if branch.is_empty() {
            self.current_view_branch = None;
            return;
        }
        self.current_view_branch = Some(branch.clone());
        if let Some(store) = &self.review_store
            && let Ok(Some((Some(file), line))) = store.get_view_state(&branch)
        {
            self.pending_view_restore = Some(PendingViewRestore {
                file,
                scroll: line.max(0) as usize,
            });
        }
    }

    /// Persist the in-memory view (open file + scroll) for `branch`.
    ///
    /// If a restore is still pending (the user never opened the viewer for this
    /// worktree this session), the unconsumed pending value is written back
    /// unchanged so we don't clobber the saved state with an empty view.
    pub(super) fn save_view_for(&self, branch: &str) {
        let Some(store) = &self.review_store else {
            return;
        };
        let (file, line) = match &self.pending_view_restore {
            Some(r) => (Some(r.file.clone()), r.scroll as i64),
            None => (
                self.viewer_state.content.current_file.clone(),
                self.viewer_state.content.file_scroll as i64,
            ),
        };
        let _ = store.save_view_state(branch, file.as_deref(), line);
    }

    /// Save the current worktree's view and selection. Called before exit /
    /// restart and before switching repos.
    pub fn persist_view_state(&self) {
        if let Some(branch) = &self.current_view_branch {
            self.save_view_for(branch);
            if let Some(store) = &self.review_store {
                let _ = store.set_selected_worktree(branch);
            }
        }
    }

    /// Consume a one-shot [`PendingViewRestore`]: open the saved file and
    /// scroll to the saved line. No-op if nothing is pending or the file no
    /// longer exists. The scroll target is clamped to the file length so a
    /// shrunken file doesn't leave a blank viewer.
    pub fn consume_pending_view_restore(&mut self) {
        let Some(restore) = self.pending_view_restore.take() else {
            return;
        };
        let wt_path = self.selected_worktree_path();
        if !wt_path.join(&restore.file).is_file() {
            return;
        }
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&wt_path, &restore.file, tab_width);
        let max = self.viewer_state.content.file_content.len().saturating_sub(1);
        self.viewer_state.content.file_scroll = restore.scroll.min(max);
    }

    /// Run syntect highlighting on the currently loaded file content.
    pub fn rehighlight_viewer(&mut self) {
        // Use disjoint field borrows to satisfy the borrow checker.
        let syntax_set = &self.syntax_set;
        let theme = &self.syntect_theme;
        self.viewer_state.highlight_content(syntax_set, theme);
    }

    /// Load (or reload) the diff for the currently selected worktree
    /// against the configured main branch.
    pub fn refresh_diff(&mut self) {
        let word_diff = self.config.diff.word_diff;
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let path = wt.path.clone();
            // A PR-review worktree may target a base other than the configured
            // main branch (e.g. a release/develop branch); prefer the base ref
            // recorded at intake time and only fall back to main_branch when
            // none was saved (regular worktrees, or DB unavailable).
            let saved_base = self
                .review_store
                .as_ref()
                .and_then(|store| store.get_worktree_base_branch(&wt.branch).ok().flatten());
            let base_branch =
                resolve_diff_base_branch(saved_base, &self.config.general.main_branch);
            let tab_width = self.config.viewer.tab_width;
            self.diff_state
                .load_diff(&path, &base_branch, word_diff, tab_width);
            self.viewer_state.invalidate_diff_annotations();
        }
    }

    /// Open a file path (relative to the current worktree) in the Viewer panel.
    ///
    /// Optionally jumps to `line` (1-indexed). Reveals the file in the explorer
    /// tree, switches focus to Viewer, and shows a status message.
    pub fn open_file_in_viewer(&mut self, relative_path: &str, line: Option<usize>) {
        let wt_path = self.selected_worktree_path();
        let tab_width = self.config.viewer.tab_width;

        self.viewer_state
            .open_file(&wt_path, relative_path, tab_width);
        self.viewer_state
            .reveal_file_in_tree(relative_path, &wt_path);

        if let Some(ln) = line {
            let max = self
                .viewer_state
                .content
                .file_content
                .len()
                .saturating_sub(1);
            self.viewer_state.content.file_scroll = (ln.saturating_sub(1)).min(max);
        }

        self.set_focus(Focus::Viewer);

        let msg = if let Some(ln) = line {
            format!("Opened {relative_path}:{ln} in Viewer")
        } else {
            format!("Opened {relative_path} in Viewer")
        };
        self.set_status(msg, StatusLevel::Success);
    }
}

/// Resolve the base branch a diff should be computed against: a worktree's
/// saved base ref (recorded at PR-intake time — see `save_worktree_base_branch`)
/// takes priority, since a PR may target something other than the configured
/// main branch (e.g. release/develop); `main_branch` is only used as a
/// fallback for worktrees with no saved base (regular worktrees, or when the
/// review DB is unavailable).
fn resolve_diff_base_branch(saved_base: Option<String>, main_branch: &str) -> String {
    saved_base.unwrap_or_else(|| main_branch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_diff_base_branch_prefers_saved_base_over_main() {
        assert_eq!(
            resolve_diff_base_branch(Some("release/1.0".to_string()), "main"),
            "release/1.0"
        );
    }

    #[test]
    fn resolve_diff_base_branch_falls_back_to_main_when_unsaved() {
        assert_eq!(resolve_diff_base_branch(None, "main"), "main");
    }
}
