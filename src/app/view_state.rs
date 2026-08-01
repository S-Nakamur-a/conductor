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
            self.worktrees.select(idx);
        }

        // Point the worktree-list cursor at the restored worktree.
        self.rebuild_worktree_list_rows();
        let sel = self.worktrees.selected_index();
        if let Some(pos) = self
            .worktrees.rows
            .iter()
            .position(|r| matches!(r, super::WorktreeListRow::Worktree(i) if *i == sel))
        {
            self.worktrees.row_selected = pos;
        }

        // Track the loaded worktree and seed its saved file/scroll.
        let branch = self.selected_worktree_branch();
        self.view_restore.pending = None;
        if branch.is_empty() {
            self.view_restore.current_branch = None;
            return;
        }
        self.view_restore.current_branch = Some(branch.clone());
        if let Some(store) = &self.review_store
            && let Ok(Some((Some(file), line))) = store.get_view_state(&branch)
        {
            self.view_restore.pending = Some(PendingViewRestore {
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
        let (file, line) = match &self.view_restore.pending {
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
        if let Some(branch) = &self.view_restore.current_branch {
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
        let Some(restore) = self.view_restore.pending.take() else {
            return;
        };
        match restore_disposition(
            self.viewer_state.content.current_file.is_some(),
            self.viewer_state.is_summary(),
        ) {
            RestoreDisposition::Apply => {}
            RestoreDisposition::Drop => return,
            RestoreDisposition::Keep => {
                self.view_restore.pending = Some(restore);
                return;
            }
        }
        // 復元先の存在確認は Viewer の根で行う。ここは「ツリーが揃った直後」に
        // 呼ばれる (同期の refresh_viewer と、非同期の worktree 切り替えの両方)
        // ので、そのツリーと同じ根で見ないと確認と実際に開く先がずれる。
        if !self.viewer_state.root().join(&restore.file).is_file() {
            return;
        }
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&restore.file, tab_width);
        let max = self.viewer_state.content.file_content.len().saturating_sub(1);
        self.viewer_state.content.file_scroll = restore.scroll.min(max);
    }

    /// Run syntect highlighting on the currently loaded file content.
    pub fn rehighlight_viewer(&mut self) {
        // Use disjoint field borrows to satisfy the borrow checker.
        let syntax_set = &self.highlight.syntax_set;
        let theme = &self.highlight.theme;
        self.viewer_state.highlight_content(syntax_set, theme);
    }

    /// The ref `branch`'s diff should be computed against.
    ///
    /// Every diff path must go through here. There are two of them — this one,
    /// reached by `refresh_diff`, and the background computation on worktree
    /// switch — and they used to decide the base differently, so the same
    /// worktree showed one file list right after the switch and a different one
    /// after the next refresh. Keeping the decision in a single method is what
    /// stops that from silently coming back.
    pub(super) fn diff_base_for(&self, branch: &str) -> String {
        // A PR-review worktree may target a base other than the configured main
        // branch (e.g. a release/develop branch); prefer the base ref recorded
        // at intake time and only fall back to main_branch when none was saved
        // (regular worktrees, or DB unavailable).
        let saved_base = self
            .review_store
            .as_ref()
            .and_then(|store| store.get_worktree_base_branch(branch).ok().flatten());
        resolve_diff_base_branch(saved_base, &self.config.general.main_branch)
    }

    /// Load (or reload) the diff for the currently selected worktree
    /// against its resolved base ref.
    pub fn refresh_diff(&mut self) {
        let word_diff = self.config.diff.word_diff;
        if let Some(wt) = self.worktrees.selected() {
            let path = wt.path.clone();
            let base_branch = self.diff_base_for(&wt.branch);
            let tab_width = self.config.viewer.tab_width;
            self.diff_state
                .load_diff(&path, &base_branch, word_diff, tab_width);
            self.viewer_state.invalidate_diff_annotations();
        }
    }

    /// Switch the Viewer between raw markdown source and rendered prose.
    ///
    /// Only meaningful for a markdown file in the plain-file view; anywhere else
    /// it flashes a hint rather than silently latching a mode the user can't
    /// see, since the header toggle is hidden in exactly those cases.
    pub fn cmd_toggle_markdown_render(&mut self) {
        if !self.viewer_state.markdown_toggle_available() {
            self.set_status(
                "Raw/Rendered applies to a markdown file in the Viewer".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        self.viewer_state.toggle_markdown_rendered();
        let msg = if self.viewer_state.md_rendered {
            "Markdown: Rendered"
        } else {
            "Markdown: Raw"
        };
        self.set_status(msg.to_string(), StatusLevel::Info);
    }

    /// Open a file path (relative to the current worktree) in the Viewer panel.
    ///
    /// Optionally jumps to `line` (1-indexed). Reveals the file in the explorer
    /// tree, switches focus to Viewer, and shows a status message.
    pub fn open_file_in_viewer(&mut self, relative_path: &str, line: Option<usize>) {
        let tab_width = self.config.viewer.tab_width;

        self.viewer_state.open_file(relative_path, tab_width);
        self.viewer_state.reveal_file_in_tree(relative_path);

        if let Some(ln) = line {
            let max = self
                .viewer_state
                .content
                .file_content
                .len()
                .saturating_sub(1);
            self.viewer_state.content.file_scroll = (ln.saturating_sub(1)).min(max);
            self.viewer_state.show_raw_for_line_target();
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

/// What to do with a pending [`PendingViewRestore`] that has come due.
#[derive(Debug, PartialEq, Eq)]
enum RestoreDisposition {
    /// Nothing is showing — open the saved file as intended.
    Apply,
    /// The user opened a real file during the window between the worktree
    /// switch and the tree finishing its walk. The saved view is obsolete, so
    /// forget it; keeping it armed would make [`App::save_view_for`] persist
    /// the stale pending path instead of the file the user ended up on.
    Drop,
    /// Only the SUMMARY pseudo-file is showing, with no file behind it. Don't
    /// open over it, but stay armed: the view-state schema has no way to say
    /// "was viewing SUMMARY", so dropping here would persist an empty view and
    /// lose the saved file outright. The caller re-runs this on every later
    /// consume, so the restore can still land once the viewer is empty again.
    Keep,
}

/// Decide the fate of a due view restore from what the viewer is showing.
///
/// Split out from [`App::consume_pending_view_restore`] because both wrong
/// answers are silent: `Drop` where `Keep` belongs quietly erases a branch's
/// saved file, and `Keep` where `Drop` belongs quietly freezes it at a stale
/// value. Neither surfaces as a crash, so the truth table is pinned by tests.
fn restore_disposition(has_open_file: bool, showing_summary: bool) -> RestoreDisposition {
    if has_open_file {
        RestoreDisposition::Drop
    } else if showing_summary {
        RestoreDisposition::Keep
    } else {
        RestoreDisposition::Apply
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

    /// The full truth table. Startup and worktree switch both reset the viewer
    /// before arming a restore, so `Apply` is the ordinary path; the other two
    /// rows only occur when the user got there first during the tree walk.
    #[test]
    fn restore_disposition_truth_table() {
        use RestoreDisposition::*;
        // Viewer empty: the restore does its job.
        assert_eq!(restore_disposition(false, false), Apply);
        // Only SUMMARY open: don't clobber it, but stay armed so the branch's
        // saved file isn't erased by a later save.
        assert_eq!(restore_disposition(false, true), Keep);
        // A real file is open: the saved view is obsolete either way, including
        // when SUMMARY is layered over that file — dropping keeps persistence
        // tracking what the user actually opened.
        assert_eq!(restore_disposition(true, false), Drop);
        assert_eq!(restore_disposition(true, true), Drop);
    }
}
