//! Review mode state — tracks the UI state for the Review panel.
//!
//! Manages the list of comments currently visible, selection, scrolling,
//! and the input mode for adding or editing review comments.

use std::collections::{HashMap, HashSet};

use crate::review_store::{CommentKind, CommentTemplate, ReviewComment, ReviewReply, ReviewStore};
use crate::text_input::TextInput;

/// A single row in the virtual comment list.
///
/// When a comment thread is expanded, reply rows appear after the parent
/// comment row. This enum lets the UI and event handler treat the list
/// as a flat sequence while preserving the parent–reply relationship.
#[derive(Debug, Clone)]
pub enum CommentListRow {
    /// A top-level comment at the given index in `ReviewState::comments`.
    Comment { comment_idx: usize },
    /// A reply belonging to the comment at `comment_idx`.
    Reply {
        comment_idx: usize,
        reply_idx: usize,
    },
}

/// The input mode the review panel is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewInputMode {
    /// Navigating the comment list.
    Normal,
    /// Typing a new comment body (format: "file:line body").
    AddingComment,
    /// Editing the body of an existing comment.
    EditingComment,
    /// Editing the body of an existing reply.
    EditingReply,
    /// Replying to an existing comment.
    ReplyingToComment,
    /// Awaiting y/n confirmation before deleting a comment or reply.
    ConfirmingDelete,
}

/// What a pending (awaiting-confirmation) delete targets.
#[derive(Debug, Clone)]
pub enum PendingDelete {
    /// Delete a whole comment (cascades to its replies).
    Comment { id: String },
    /// Delete a single reply, leaving its parent comment intact.
    Reply { id: String, parent_id: String },
}

/// UI state for the Review mode.
pub struct ReviewState {
    /// Comments for the current worktree, loaded from the database.
    pub comments: Vec<ReviewComment>,
    /// Index of the currently selected comment.
    pub selected: usize,
    /// Current input mode.
    pub input_mode: ReviewInputMode,
    /// Text buffer for the input field (used during adding/editing).
    pub input_buffer: TextInput,
    /// The kind of comment being created (Suggest or Question).
    pub input_kind: CommentKind,
    /// Target of an in-progress **new** comment: `(file_path, line_start,
    /// line_end)`. When set (during `AddingComment`), the compose box renders
    /// inline at that line and the buffer holds only the body — no `file:line`
    /// prefix. `None` falls back to the legacy prefix-in-buffer parse path
    /// (template picker / command palette entry points).
    pub input_anchor: Option<(String, u32, Option<u32>)>,
    /// Optional flash message displayed at the bottom of the panel.
    pub status_message: Option<String>,
    /// Current search/filter query for comments.
    pub search_query: TextInput,
    /// Whether the search input is active.
    pub search_active: bool,
    /// Filtered comment indices (into the `comments` vec).
    pub filtered_indices: Vec<usize>,
    /// Available comment templates loaded from the database.
    pub templates: Vec<CommentTemplate>,
    /// Whether the template picker is visible.
    pub template_picker_active: bool,
    /// Index of the currently selected template in the picker.
    pub template_selected: usize,
    /// Cached comments for the currently viewed file, keyed by 1-indexed line number.
    pub file_comments: HashMap<usize, Vec<ReviewComment>>,
    /// The file path for which `file_comments` was built (for cache invalidation).
    pub file_comments_path: Option<String>,
    /// Cached reply counts per comment ID, loaded alongside comments.
    pub reply_counts: HashMap<String, usize>,
    /// Set of comment IDs whose reply threads are currently expanded.
    pub expanded_comments: HashSet<String>,
    /// Cached replies for expanded comments, keyed by comment ID.
    pub cached_replies: HashMap<String, Vec<ReviewReply>>,
    /// Virtual row list for the comment panel (rebuilt on expansion changes).
    pub comment_list_rows: Vec<CommentListRow>,

    // ── Comment detail overlay ──────────────────────────────────
    /// Whether the comment detail modal is visible.
    pub comment_detail_active: bool,
    /// Scroll offset within the detail modal.
    pub comment_detail_scroll: usize,
    /// Maximum scroll offset (set by render).
    pub comment_detail_max_scroll: usize,
    /// Index of the comment being viewed in the detail modal.
    pub comment_detail_idx: usize,

    /// Branch-level change summary (the "what & why" of the whole diff), loaded
    /// alongside comments and rendered as a banner above the diff. `None` when
    /// the current branch has no summary written.
    pub change_summary: Option<String>,

    /// Target of an in-progress delete awaiting y/n confirmation (set while
    /// `input_mode == ConfirmingDelete`).
    pub pending_delete: Option<PendingDelete>,
    /// `(reply_id, parent_comment_id)` of a reply being edited (set while
    /// `input_mode == EditingReply`).
    pub editing_reply: Option<(String, String)>,
}

impl ReviewState {
    /// Create a new `ReviewState` with empty defaults.
    pub fn new() -> Self {
        Self {
            comments: Vec::new(),
            selected: 0,
            input_mode: ReviewInputMode::Normal,
            input_buffer: TextInput::new_multiline(),
            input_kind: CommentKind::Suggest,
            input_anchor: None,
            status_message: None,
            search_query: TextInput::new(),
            search_active: false,
            filtered_indices: Vec::new(),
            templates: Vec::new(),
            template_picker_active: false,
            template_selected: 0,
            file_comments: HashMap::new(),
            file_comments_path: None,
            reply_counts: HashMap::new(),
            expanded_comments: HashSet::new(),
            cached_replies: HashMap::new(),
            comment_list_rows: Vec::new(),
            comment_detail_active: false,
            comment_detail_scroll: 0,
            comment_detail_max_scroll: 0,
            comment_detail_idx: 0,
            change_summary: None,
            pending_delete: None,
            editing_reply: None,
        }
    }

    /// Resolve a visual row to `(comment_idx, reply_idx)` when it is a reply row.
    pub fn selected_reply_at(&self, visual_idx: usize) -> Option<(usize, usize)> {
        match self.comment_list_rows.get(visual_idx) {
            Some(CommentListRow::Reply {
                comment_idx,
                reply_idx,
            }) => Some((*comment_idx, *reply_idx)),
            _ => None,
        }
    }

    /// Resolve `(comment_idx, reply_idx)` to `(reply_id, parent_comment_id)`
    /// via the parent comment's cached replies.
    pub fn reply_id_at(&self, comment_idx: usize, reply_idx: usize) -> Option<(String, String)> {
        let comment = self.comments.get(comment_idx)?;
        let replies = self.cached_replies.get(&comment.id)?;
        let reply = replies.get(reply_idx)?;
        Some((reply.id.clone(), comment.id.clone()))
    }

    /// Re-fetch the replies for one comment into the cache (after a reply was
    /// added / edited / deleted), update its reply count, and rebuild the
    /// virtual row list so the thread reflects the change.
    pub fn refresh_replies(&mut self, store: &ReviewStore, comment_id: &str) {
        match store.get_replies(comment_id) {
            Ok(replies) => {
                self.reply_counts
                    .insert(comment_id.to_string(), replies.len());
                if replies.is_empty() {
                    self.cached_replies.remove(comment_id);
                } else {
                    self.cached_replies.insert(comment_id.to_string(), replies);
                }
            }
            Err(e) => log::warn!("failed to refresh replies: {e}"),
        }
        self.rebuild_comment_list_rows();
    }

    /// Reload comments from the database for the given worktree.
    pub fn load_comments(&mut self, store: &ReviewStore, worktree: &str) {
        match store.reviews_for_worktree(worktree) {
            Ok(comments) => {
                self.comments = comments;
                self.filtered_indices = (0..self.comments.len()).collect();
                // Clamp selection to valid range.
                if !self.comments.is_empty() && self.selected >= self.comments.len() {
                    self.selected = self.comments.len() - 1;
                }
            }
            Err(e) => {
                log::warn!("failed to load review comments: {e}");
                self.comments.clear();
                self.filtered_indices.clear();
                self.selected = 0;
            }
        }
        // Load reply counts for all comments in this worktree.
        match store.reply_counts_for_worktree(worktree) {
            Ok(counts) => {
                self.reply_counts = counts;
            }
            Err(e) => {
                log::warn!("failed to load reply counts: {e}");
                self.reply_counts.clear();
            }
        }
        // Load the branch-level change summary for this worktree.
        match store.get_change_summary(worktree) {
            Ok(summary) => self.change_summary = summary,
            Err(e) => {
                log::warn!("failed to load change summary: {e}");
                self.change_summary = None;
            }
        }
        // Clean up expansion state for comments that no longer exist.
        let current_ids: HashSet<String> = self.comments.iter().map(|c| c.id.clone()).collect();
        self.expanded_comments.retain(|id| current_ids.contains(id));
        self.cached_replies.retain(|id, _| current_ids.contains(id));
        self.rebuild_comment_list_rows();
    }

    /// Rebuild the virtual row list from `comments`, `expanded_comments`,
    /// and `cached_replies`.
    pub fn rebuild_comment_list_rows(&mut self) {
        self.comment_list_rows.clear();
        for (comment_idx, comment) in self.comments.iter().enumerate() {
            self.comment_list_rows
                .push(CommentListRow::Comment { comment_idx });
            if self.expanded_comments.contains(&comment.id)
                && let Some(replies) = self.cached_replies.get(&comment.id)
            {
                for reply_idx in 0..replies.len() {
                    self.comment_list_rows.push(CommentListRow::Reply {
                        comment_idx,
                        reply_idx,
                    });
                }
            }
        }
    }

    /// Resolve a visual row index to the parent comment index.
    pub fn selected_comment_idx(&self, visual_idx: usize) -> Option<usize> {
        match self.comment_list_rows.get(visual_idx) {
            Some(CommentListRow::Comment { comment_idx }) => Some(*comment_idx),
            Some(CommentListRow::Reply { comment_idx, .. }) => Some(*comment_idx),
            None => None,
        }
    }

    /// Return a reference to the currently selected comment, if any.
    pub fn selected_comment(&self) -> Option<&ReviewComment> {
        self.comments.get(self.selected)
    }

    /// Apply the current search query to filter the comment list.
    pub fn apply_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_indices = (0..self.comments.len()).collect();
        } else {
            let query_lower = self.search_query.to_lowercase();
            self.filtered_indices = self
                .comments
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.body.to_lowercase().contains(&query_lower)
                        || c.file_path.to_lowercase().contains(&query_lower)
                })
                .map(|(i, _)| i)
                .collect();
        }
        // Clamp selection.
        if !self.filtered_indices.is_empty() && self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }

    /// Build the per-file comment cache from in-memory comments.
    ///
    /// Filters `self.comments` by `file_path` and maps each line in the
    /// comment's range to a vec of comments covering that line. Resolved
    /// comments are kept here so their badge still appears in the gutter;
    /// the inline thread expansion (see `build_inline_thread_lines`) is what
    /// hides resolved comments.
    pub fn build_file_comment_cache(&mut self, file_path: &str) {
        self.file_comments.clear();
        self.file_comments_path = Some(file_path.to_string());

        for comment in &self.comments {
            if comment.file_path != file_path {
                continue;
            }
            let start = comment.line_start as usize;
            let end = comment.line_end.unwrap_or(comment.line_start) as usize;
            for line in start..=end {
                self.file_comments
                    .entry(line)
                    .or_default()
                    .push(comment.clone());
            }
        }
    }

    /// Load comment templates from the database.
    pub fn load_templates(&mut self, store: &ReviewStore) {
        match store.list_templates() {
            Ok(templates) => {
                self.templates = templates;
                if !self.templates.is_empty() && self.template_selected >= self.templates.len() {
                    self.template_selected = self.templates.len() - 1;
                }
            }
            Err(e) => {
                log::warn!("failed to load comment templates: {e}");
                self.templates.clear();
                self.template_selected = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_store::{Author, CommentKind, CommentStatus, ReviewComment};

    fn comment(id: &str, line: u32, status: CommentStatus) -> ReviewComment {
        ReviewComment {
            id: id.to_string(),
            worktree: "wt".to_string(),
            file_path: "src/main.rs".to_string(),
            line_start: line,
            line_end: None,
            kind: CommentKind::Suggest,
            body: "body".to_string(),
            status,
            commit_ref: "abc".to_string(),
            author: Author::User,
            branch: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn build_file_comment_cache_keeps_resolved_for_badges() {
        let mut state = ReviewState::new();
        state.comments = vec![
            comment("c1", 10, CommentStatus::Pending),
            comment("c2", 20, CommentStatus::Resolved),
        ];

        state.build_file_comment_cache("src/main.rs");

        // Both lines keep a cache entry so the gutter badge still appears;
        // resolved comments are only hidden from the inline thread expansion.
        assert!(state.file_comments.contains_key(&10));
        assert!(state.file_comments.contains_key(&20));
    }
}
