//! Explorer panel key handling (file tree, diff list, comment list).

use crossterm::event::KeyEvent;

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};
use crate::review_state::{CommentListRow, ReviewInputMode};
use crate::review_store::{Author, CommentKind};

use super::{adjust_diff_list_scroll, adjust_tree_scroll};

/// Handle keys when the Explorer panel is focused.
pub(super) fn handle_explorer_key(app: &mut App, key: KeyEvent) {
    if app.viewer_state.file_tree.is_empty() {
        app.refresh_viewer();
    }

    // Check for show-diff / show-comments before delegating to sub-panels.
    let action = app.keymap.resolve(&key, KeyContext::Explorer);
    match action {
        Some(Action::ShowDiffList) => {
            app.viewer_state.explorer_show_comments = false;
            app.viewer_state.explorer_focus_on_diff_list = true;
            return;
        }
        Some(Action::ShowCommentList) => {
            app.viewer_state.explorer_show_comments = true;
            app.viewer_state.explorer_focus_on_diff_list = true;
            return;
        }
        _ => {}
    }

    if app.viewer_state.explorer_focus_on_diff_list {
        if app.viewer_state.explorer_show_comments {
            handle_explorer_comment_list_key(app, key);
        } else {
            handle_explorer_diff_list_key(app, key);
        }
        return;
    }

    let visible = app.viewer_state.visible_indices();
    if visible.is_empty() {
        return;
    }

    let cur_vis = visible
        .iter()
        .position(|&i| i == app.viewer_state.tree_selected)
        .unwrap_or(0);

    match action {
        Some(Action::NavigateDown) => {
            if cur_vis + 1 < visible.len() {
                app.viewer_state.tree_selected = visible[cur_vis + 1];
            }
        }
        Some(Action::NavigateUp) => {
            if cur_vis > 0 {
                app.viewer_state.tree_selected = visible[cur_vis - 1];
            }
        }
        Some(Action::Select) => {
            let idx = app.viewer_state.tree_selected;
            if let Some(entry) = app.viewer_state.file_tree.get(idx).cloned() {
                if entry.is_dir {
                    if !entry.is_expanded {
                        if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                            app.viewer_state.ensure_children_loaded(idx, &wt.path);
                        }
                    }
                    app.viewer_state.toggle_dir(idx);
                } else if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let path = wt.path.clone();
                    app.viewer_state.open_file(&path, &entry.path);
                    app.rehighlight_viewer();
                    app.review_state.build_file_comment_cache(&entry.path);
                    app.set_focus(Focus::Viewer);
                }
            }
        }
        Some(Action::ExpandOrRight) => {
            let idx = app.viewer_state.tree_selected;
            if let Some(entry) = app.viewer_state.file_tree.get(idx) {
                if entry.is_dir && !entry.is_expanded {
                    if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                        app.viewer_state.ensure_children_loaded(idx, &wt.path);
                    }
                }
            }
            app.viewer_state.expand_dir(idx);
        }
        Some(Action::CollapseOrLeft) => {
            let idx = app.viewer_state.tree_selected;
            app.viewer_state.collapse_dir(idx);
        }
        Some(Action::GoToTop) => {
            if let Some(&first) = visible.first() {
                app.viewer_state.tree_selected = first;
            }
        }
        Some(Action::GoToBottom) => {
            if let Some(&last) = visible.last() {
                app.viewer_state.tree_selected = last;
            }
        }
        Some(Action::SearchFilename) => {
            app.viewer_state.filename_search_active = true;
            app.viewer_state.filename_search_query.clear();
            app.viewer_state.filename_search_results.clear();
            app.viewer_state.filename_search_selected = 0;
            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                app.viewer_state.populate_filename_search_cache(&wt.path);
            }
            app.viewer_state.execute_filename_search();
        }
        _ => {}
    }

    adjust_tree_scroll(app);
}

// ── Explorer: diff list sub-panel ────────────────────────────────────────

pub(super) fn handle_explorer_diff_list_key(app: &mut App, key: KeyEvent) {
    let count = app.diff_state.display_list.len();
    let action = app.keymap.resolve(&key, KeyContext::ExplorerDiffList);

    match action {
        Some(Action::ExitSubPanel) => {
            app.viewer_state.explorer_focus_on_diff_list = false;
        }
        Some(Action::NavigateDown) => {
            if count > 0 && app.viewer_state.diff_list_selected + 1 < count {
                app.viewer_state.diff_list_selected += 1;
            }
        }
        Some(Action::NavigateUp) => {
            if app.viewer_state.diff_list_selected > 0 {
                app.viewer_state.diff_list_selected -= 1;
            }
        }
        Some(Action::CollapseOrLeft) => {
            let selected = app.viewer_state.diff_list_selected;
            app.diff_state.collapse_section(selected);
            let new_count = app.diff_state.display_list.len();
            if new_count > 0 && app.viewer_state.diff_list_selected >= new_count {
                app.viewer_state.diff_list_selected = new_count - 1;
            }
        }
        Some(Action::ExpandOrRight) => {
            let selected = app.viewer_state.diff_list_selected;
            app.diff_state.expand_section(selected);
        }
        Some(Action::Select) => {
            let selected = app.viewer_state.diff_list_selected;
            if app.diff_state.toggle_section(selected) {
                let new_count = app.diff_state.display_list.len();
                if new_count > 0 && app.viewer_state.diff_list_selected >= new_count {
                    app.viewer_state.diff_list_selected = new_count - 1;
                }
            } else if let Some((file_diff, _section)) = app.diff_state.resolve_file(selected) {
                let file_path = file_diff.path.clone();
                let file_diff_clone = file_diff.clone();
                if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let wt_path = wt.path.clone();
                    app.viewer_state.open_file(&wt_path, &file_path);
                    app.viewer_state.reveal_file_in_tree(&file_path, &wt_path);
                    app.rehighlight_viewer();
                    app.review_state.build_file_comment_cache(&file_path);

                    app.viewer_state.build_unified_diff_view(&file_diff_clone);

                    if let Some(pos) = app.viewer_state.diff_view_lines.iter().position(|e| {
                        matches!(e, crate::viewer::UnifiedDiffEntry::Line { tag, .. }
                            if *tag != crate::diff_state::DiffLineTag::Equal)
                    }) {
                        app.viewer_state.diff_view_scroll = pos.saturating_sub(3);
                    }

                    app.set_focus(Focus::Viewer);
                }
            }
        }
        Some(Action::GoToTop) => {
            app.viewer_state.diff_list_selected = 0;
        }
        Some(Action::GoToBottom) => {
            if count > 0 {
                app.viewer_state.diff_list_selected = count - 1;
            }
        }
        _ => {}
    }

    adjust_diff_list_scroll(app);
}

// ── Explorer: comment list sub-panel ──────────────────────────────────────

pub(super) fn handle_explorer_comment_list_key(app: &mut App, key: KeyEvent) {
    let row_count = app.review_state.comment_list_rows.len();
    let action = app.keymap.resolve(&key, KeyContext::ExplorerCommentList);

    match action {
        Some(Action::ExitSubPanel) => {
            app.viewer_state.explorer_focus_on_diff_list = false;
        }
        Some(Action::DeleteComment) => {
            if row_count > 0 {
                app.delete_selected_review_comment();
            }
        }
        Some(Action::ToggleResolve) => {
            if row_count > 0 {
                app.toggle_selected_review_status();
            }
        }
        Some(Action::EditComment) => {
            let comment_idx = app
                .review_state
                .selected_comment_idx(app.viewer_state.comment_list_selected);
            if let Some(comment) = comment_idx.and_then(|idx| app.review_state.comments.get(idx)) {
                app.review_state.input_buffer.set_text(&comment.body);
                app.review_state.input_mode = ReviewInputMode::EditingComment;
                app.review_state.selected = comment_idx.unwrap();
                app.review_state.status_message =
                    Some("Edit comment (Enter to save, Esc to cancel)".to_string());
            }
        }
        Some(Action::ReplyToComment) => {
            if row_count > 0 {
                let comment_idx = app
                    .review_state
                    .selected_comment_idx(app.viewer_state.comment_list_selected);
                if let Some(idx) = comment_idx {
                    app.review_state.input_buffer.clear();
                    app.review_state.input_mode = ReviewInputMode::ReplyingToComment;
                    app.review_state.selected = idx;
                    app.review_state.status_message =
                        Some("Reply to comment (Enter to send, Esc to cancel)".to_string());
                }
            }
        }
        Some(Action::NavigateDown) => {
            if row_count > 0 && app.viewer_state.comment_list_selected + 1 < row_count {
                app.viewer_state.comment_list_selected += 1;
            }
        }
        Some(Action::NavigateUp) => {
            if app.viewer_state.comment_list_selected > 0 {
                app.viewer_state.comment_list_selected -= 1;
            }
        }
        Some(Action::GoToTop) => {
            app.viewer_state.comment_list_selected = 0;
        }
        Some(Action::GoToBottom) => {
            if row_count > 0 {
                app.viewer_state.comment_list_selected = row_count - 1;
            }
        }
        Some(Action::CollapseOrLeft) => {
            let visual = app.viewer_state.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual).cloned() {
                Some(CommentListRow::Reply { comment_idx, .. }) => {
                    if let Some(parent_visual) = app
                        .review_state
                        .comment_list_rows
                        .iter()
                        .position(|r| matches!(r, CommentListRow::Comment { comment_idx: ci } if *ci == comment_idx))
                    {
                        app.viewer_state.comment_list_selected = parent_visual;
                    }
                    app.toggle_comment_expansion();
                }
                Some(CommentListRow::Comment { comment_idx }) => {
                    if let Some(comment) = app.review_state.comments.get(comment_idx) {
                        if app.review_state.expanded_comments.contains(&comment.id) {
                            app.toggle_comment_expansion();
                        }
                    }
                }
                None => {}
            }
        }
        Some(Action::Select) | Some(Action::ExpandOrRight) => {
            let visual = app.viewer_state.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual).cloned() {
                Some(CommentListRow::Comment { comment_idx }) => {
                    let has_replies = app
                        .review_state
                        .comments
                        .get(comment_idx)
                        .and_then(|c| app.review_state.reply_counts.get(&c.id))
                        .copied()
                        .unwrap_or(0)
                        > 0;

                    if has_replies {
                        app.toggle_comment_expansion();
                    } else {
                        navigate_to_comment_with_focus(app, comment_idx, false);
                    }
                }
                Some(CommentListRow::Reply { comment_idx, .. }) => {
                    navigate_to_comment_with_focus(app, comment_idx, false);
                }
                None => {}
            }
        }
        Some(Action::ViewCommentDetail) => {
            let visual = app.viewer_state.comment_list_selected;
            if let Some(comment_idx) = app.review_state.selected_comment_idx(visual) {
                app.review_state.comment_detail_idx = comment_idx;
                app.review_state.comment_detail_scroll = 0;
                app.review_state.comment_detail_active = true;
                if let Some(comment) = app.review_state.comments.get(comment_idx) {
                    let cid = comment.id.clone();
                    if !app.review_state.cached_replies.contains_key(&cid) {
                        if let Some(store) = app.review_store.as_ref() {
                            if let Ok(replies) = store.get_replies(&cid) {
                                app.review_state.cached_replies.insert(cid, replies);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    // Adjust scroll for comment list.
    let selected = app.viewer_state.comment_list_selected;
    let page_size = app.viewer_state.explorer_diff_list_height.max(1);
    if selected < app.viewer_state.comment_list_scroll {
        app.viewer_state.comment_list_scroll = selected;
    } else if selected >= app.viewer_state.comment_list_scroll + page_size {
        app.viewer_state.comment_list_scroll = selected.saturating_sub(page_size - 1);
    }
}

/// Navigate to the file and line of the comment at the given index.
/// When `focus_viewer` is true, the focus moves to the Viewer panel;
/// otherwise the current panel focus is preserved (e.g. comment list).
pub(super) fn navigate_to_comment_with_focus(app: &mut App, comment_idx: usize, focus_viewer: bool) {
    if let Some(comment) = app.review_state.comments.get(comment_idx) {
        let file_path = comment.file_path.clone();
        let line = comment.line_start as usize;
        if let Some(wt) = app.worktrees.get(app.selected_worktree) {
            let wt_path = wt.path.clone();
            app.viewer_state.open_file(&wt_path, &file_path);
            app.rehighlight_viewer();
            app.viewer_state.file_scroll = line.saturating_sub(1);
            app.viewer_state.selected_line_start = Some(line);
            app.viewer_state.selected_line_end = None;
            app.review_state.build_file_comment_cache(&file_path);
            if focus_viewer {
                app.set_focus(Focus::Viewer);
            }
        }
    }
}

/// Open the review comment input from the Viewer, pre-filling the location.
pub(super) fn open_viewer_comment(app: &mut App) {
    let file_path = match app.viewer_state.current_file.clone() {
        Some(p) => p,
        None => return,
    };

    let location = if let Some((start, end)) = app.viewer_state.selected_range() {
        if start == end {
            format!("{file_path}:{start} ")
        } else {
            format!("{file_path}:{start}-{end} ")
        }
    } else {
        let line = app.viewer_state.file_scroll + 1;
        format!("{file_path}:{line} ")
    };

    app.viewer_state.clear_selection();
    app.review_state.input_buffer.set_text(&location);
    app.review_state.input_kind = CommentKind::Suggest;
    app.review_state.input_mode = ReviewInputMode::AddingComment;
    app.review_state.status_message =
        Some("Add comment: [s:|q:]file:line body".to_string());
}

/// Open the comment detail modal from the Viewer panel for the current line.
pub(super) fn open_viewer_comment_detail(app: &mut App) {
    // Determine which line the cursor is on (same logic as preview).
    let cursor_line = if let Some((start, _)) = app.viewer_state.selected_range() {
        start
    } else {
        app.viewer_state.file_scroll + 1
    };

    // Find a comment on that line.
    let comments = match app.review_state.file_comments.get(&cursor_line) {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    // Find the index of the first comment in the master comment list.
    let target_id = &comments[0].id;
    let comment_idx = match app.review_state.comments.iter().position(|c| &c.id == target_id) {
        Some(idx) => idx,
        None => return,
    };

    // Load replies if not cached.
    let cid = target_id.clone();
    if !app.review_state.cached_replies.contains_key(&cid) {
        if let Some(store) = app.review_store.as_ref() {
            if let Ok(replies) = store.get_replies(&cid) {
                app.review_state.cached_replies.insert(cid, replies);
            }
        }
    }

    app.review_state.comment_detail_idx = comment_idx;
    app.review_state.comment_detail_scroll = 0;
    app.review_state.comment_detail_active = true;
}

/// Parse the input buffer and add a new review comment.
///
/// Format: `[s:|q:]file_path:line[-end] body_text`
pub(super) fn submit_new_comment(app: &mut App, input: &str) {
    let input = input.trim();
    if input.is_empty() {
        app.review_state.status_message = Some("Empty input, cancelled.".to_string());
        return;
    }

    let (kind, rest) = if let Some(stripped) = input.strip_prefix("s:") {
        (CommentKind::Suggest, stripped)
    } else if let Some(stripped) = input.strip_prefix("q:") {
        (CommentKind::Question, stripped)
    } else {
        (app.review_state.input_kind, input)
    };

    let Some(space_pos) = rest.find(' ') else {
        app.review_state.status_message =
            Some("Format: file:line body  (e.g. src/main.rs:42 fix this)".to_string());
        return;
    };

    let location = &rest[..space_pos];
    let body = rest[space_pos + 1..].trim();

    if body.is_empty() {
        app.review_state.status_message = Some("Comment body is empty.".to_string());
        return;
    }

    let Some(colon_pos) = location.rfind(':') else {
        app.review_state.status_message =
            Some("Format: file:line body  (e.g. src/main.rs:42 fix this)".to_string());
        return;
    };

    let file_path = &location[..colon_pos];
    let line_part = &location[colon_pos + 1..];

    // Parse line range: "42" or "42-50".
    let (line_start, line_end) = if let Some(dash_pos) = line_part.find('-') {
        let start_str = &line_part[..dash_pos];
        let end_str = &line_part[dash_pos + 1..];
        let Ok(start) = start_str.parse::<u32>() else {
            app.review_state.status_message =
                Some(format!("Invalid line number: '{start_str}'"));
            return;
        };
        let Ok(end) = end_str.parse::<u32>() else {
            app.review_state.status_message =
                Some(format!("Invalid line number: '{end_str}'"));
            return;
        };
        (start, Some(end))
    } else {
        let Ok(line) = line_part.parse::<u32>() else {
            app.review_state.status_message =
                Some(format!("Invalid line number: '{line_part}'"));
            return;
        };
        (line, None)
    };

    app.add_review_comment(file_path, line_start, line_end, kind, body, Author::User);
}
