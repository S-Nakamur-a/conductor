//! Review comment core / diff navigation for [`App`].
//!
//! Reloading comments from the database, opening a diff file from the diff
//! list (landing on its first comment or change), jumping between changed
//! files, auto-expanding unresolved threads, and adding a new comment.
//! Deletion lives in [`super::review_delete`], editing/status/replies in
//! [`super::review_edit`], template/history helpers in
//! [`super::review_history`], and AI walkthrough generation in
//! [`super::review_walkthrough`].

use super::*;
use crate::review_store::{Author, CommentKind};

impl App {
    /// Reload review comments from the database for the currently selected worktree.
    pub fn refresh_reviews(&mut self) {
        if let Some(store) = &self.review_store {
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // Walkthrough (if any) rides along with the same branch scope.
            self.current_walkthrough = store.get_walkthrough(&wt).ok().flatten();
            // Rebuild per-file cache for the currently viewed file.
            if let Some(file_path) = self.viewer_state.content.current_file.clone() {
                self.review_state.build_file_comment_cache(&file_path);
            }
            // Keep the diff list's SUMMARY pseudo-file in sync with whether this
            // branch has a change summary. Only rebuild when it actually flips,
            // so we don't disturb the display list on every reload.
            let has_summary = self.review_state.change_summary.is_some();
            if self.diff_state.has_summary != has_summary {
                self.diff_state.has_summary = has_summary;
                self.diff_state.rebuild_display_list();
                // If the summary vanished while its pseudo-file was open, leave
                // the now-orphaned summary view so the Viewer and the list agree.
                if !has_summary && self.viewer_state.is_summary() {
                    self.viewer_state.exit_diff_mode();
                }
            }
        }
    }

    /// Open the diff file currently selected in the diff list (the entry at
    /// `diff_list_selected`) into the Viewer. Shared by the file-jump keys; a
    /// no-op if the selected entry isn't a file.
    pub fn open_diff_file_at_selected(&mut self) {
        let idx = self.viewer_state.explorer.diff_list_selected;
        let (file_path, file_diff_clone) = match self.diff_state.resolve_file(idx) {
            Some((f, _)) => (f.path.clone(), f.clone()),
            None => return,
        };
        let Some(wt) = self.worktrees.get(self.selected_worktree) else {
            return;
        };
        let wt_path = wt.path.clone();
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&wt_path, &file_path, tab_width);
        self.viewer_state.reveal_file_in_tree(&file_path, &wt_path);
        self.rehighlight_viewer();
        self.review_state.build_file_comment_cache(&file_path);
        self.expand_threads_for_file(&file_path);
        self.viewer_state.build_unified_diff_view(&file_diff_clone);
        // Land on the first review comment if the file has any (so the reviewer
        // sees it immediately — answers "jump to the file's first comment"),
        // otherwise on the first change.
        let first_comment_line = self
            .review_state
            .comments
            .iter()
            .filter(|c| c.file_path == file_path)
            .map(|c| c.line_start as usize)
            .min();
        let target = first_comment_line
            .and_then(|line| {
                self.viewer_state
                    .diff_view
                    .diff_view_lines
                    .iter()
                    .position(|e| matches!(e, crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(n), .. } if *n == line))
            })
            .or_else(|| {
                self.viewer_state
                    .diff_view
                    .diff_view_lines
                    .iter()
                    .position(|e| {
                        matches!(e, crate::viewer::UnifiedDiffEntry::Line { tag, .. }
                            if *tag != crate::diff_state::DiffLineTag::Equal)
                    })
            });
        if let Some(pos) = target {
            self.viewer_state.diff_view.diff_view_scroll = pos.saturating_sub(3);
        }
    }

    /// Jump to the next (or previous) changed file in the diff list and open it.
    /// Skips non-file rows (section headers, directories, the SUMMARY entry).
    /// The lightweight substitute for GitHub-style cross-file scrolling.
    pub fn jump_to_changed_file(&mut self, forward: bool) {
        use crate::diff_state::DiffListEntry;
        let len = self.diff_state.display_list.len();
        // Clamp the cursor: a stale `diff_list_selected` (e.g. after the list
        // shrank on refresh) must never index past the list in the backward
        // scan below, or `display_list[i]` panics.
        let cur = self.viewer_state.explorer.diff_list_selected.min(len);
        let target = if forward {
            (cur + 1..len)
                .find(|&i| matches!(self.diff_state.display_list[i], DiffListEntry::File { .. }))
        } else {
            (0..cur)
                .rev()
                .find(|&i| matches!(self.diff_state.display_list[i], DiffListEntry::File { .. }))
        };
        if let Some(idx) = target {
            self.viewer_state.explorer.diff_list_selected = idx;
            self.open_diff_file_at_selected();
        }
    }

    /// Default-expand the inline comment threads for a freshly opened file, so
    /// review comments are visible at a glance instead of starting collapsed.
    /// Only the opened file's threads are expanded (not every file's), matching
    /// "the selected file's comments are open by default". The user can still
    /// collapse individual threads afterward.
    pub fn expand_threads_for_file(&mut self, file_path: &str) {
        // Only auto-expand lines with at least one *unresolved* comment.
        // Resolved comments are collapsed by default (their gutter badge still
        // shows, and clicking it opens the thread on demand).
        let lines: Vec<usize> = self
            .review_state
            .comments
            .iter()
            .filter(|c| {
                c.file_path == file_path
                    && c.status != crate::review_store::CommentStatus::Resolved
            })
            .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
            .collect();
        for line in lines {
            self.viewer_state
                .explorer
                .expanded_inline_threads
                .insert(line);
        }
    }

    /// Add a new review comment for the current worktree and refresh the
    /// comment list.
    pub fn add_review_comment(
        &mut self,
        file_path: &str,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: &str,
        author: Author,
    ) {
        let branch = self
            .worktrees
            .get(self.selected_worktree)
            .map(|w| w.branch.clone());

        if let Some(store) = &self.review_store {
            // Invariant: a comment's `worktree` column stores the branch name,
            // `commit_ref` is the symbolic "HEAD", and `branch` is the same
            // branch. The MCP `create_comment` tool (plugins/.../mcp) is a
            // sibling writer that mirrors this exactly — keep the two in sync.
            let wt = self.selected_worktree_branch();
            match store.add_review(
                &wt,
                file_path,
                line_start,
                line_end,
                kind,
                body,
                "HEAD",
                author,
                branch.as_deref(),
            ) {
                Ok(_) => {
                    self.review_state.status_message = Some("Comment added.".to_string());
                    self.record_stat("reviews_created");
                }
                Err(e) => {
                    log::warn!("failed to add review comment: {e}");
                    self.review_state.status_message = Some(format!("Error: {e}"));
                }
            }
            self.review_state.load_comments(store, &wt);
            // Rebuild per-file cache for the commented file.
            self.review_state.build_file_comment_cache(file_path);
            // Keep the just-created thread expanded so the comment is visible
            // immediately instead of collapsing into a gutter badge.
            let line = line_end.unwrap_or(line_start) as usize;
            self.viewer_state
                .explorer
                .expanded_inline_threads
                .insert(line);
        }
    }
}
