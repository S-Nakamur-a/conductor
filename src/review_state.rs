//! Review mode state — tracks the UI state for the Review panel.
//!
//! Manages the list of comments currently visible, selection, scrolling,
//! and the input mode for adding or editing review comments.

use std::collections::{HashMap, HashSet};

use crate::review_store::{CommentKind, CommentTemplate, ReviewComment, ReviewReply, ReviewStore};

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
    Reply { comment_idx: usize, reply_idx: usize },
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
    /// Replying to an existing comment.
    ReplyingToComment,
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
    pub input_buffer: String,
    /// The kind of comment being created (Suggest or Question).
    pub input_kind: CommentKind,
    /// Optional flash message displayed at the bottom of the panel.
    pub status_message: Option<String>,
    /// Current search/filter query for comments.
    pub search_query: String,
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
    /// Index of the comment being viewed in the detail modal.
    pub comment_detail_idx: usize,
}

impl ReviewState {
    /// Create a new `ReviewState` with empty defaults.
    pub fn new() -> Self {
        Self {
            comments: Vec::new(),
            selected: 0,
            input_mode: ReviewInputMode::Normal,
            input_buffer: String::new(),
            input_kind: CommentKind::Suggest,
            status_message: None,
            search_query: String::new(),
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
            comment_detail_idx: 0,
        }
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
            self.comment_list_rows.push(CommentListRow::Comment { comment_idx });
            if self.expanded_comments.contains(&comment.id) {
                if let Some(replies) = self.cached_replies.get(&comment.id) {
                    for reply_idx in 0..replies.len() {
                        self.comment_list_rows.push(CommentListRow::Reply {
                            comment_idx,
                            reply_idx,
                        });
                    }
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
    /// comment's range to a vec of comments covering that line.
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
