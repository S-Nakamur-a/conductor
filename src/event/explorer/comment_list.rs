//! Explorer comment-list sub-panel: navigating and acting on review
//! comments/replies, plus jumping the Viewer to a selected comment's
//! location.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;
use crate::review_state::CommentListRow;

pub(in crate::event) fn handle_explorer_comment_list_key(app: &mut App, key: KeyEvent) {
    let row_count = app.review_state.comment_list_rows.len();
    let action = app.keymap.resolve(&key, KeyContext::ExplorerCommentList);

    // When this backs the full-screen comment-list modal, Esc closes it and
    // selecting a comment jumps to it and then closes the modal.
    let in_modal = app.overlays.active == ActiveOverlay::CommentList;
    if in_modal && key.code == KeyCode::Esc {
        app.overlays.active = ActiveOverlay::None;
        return;
    }
    // Close the modal only when Select actually jumps to a location — a Select
    // on a comment that has replies just expands its thread in place, so we
    // must keep the modal open in that case.
    let close_after = in_modal
        && matches!(action, Some(Action::Select))
        && {
            let visual = app.viewer_state.explorer.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual) {
                Some(CommentListRow::Comment { comment_idx }) => {
                    let has_replies = app
                        .review_state
                        .comments
                        .get(*comment_idx)
                        .and_then(|c| app.review_state.reply_counts.get(&c.id))
                        .copied()
                        .unwrap_or(0)
                        > 0;
                    !has_replies
                }
                Some(CommentListRow::Reply { .. }) => true,
                None => false,
            }
        };

    match action {
        Some(Action::ExitSubPanel) => {
            app.viewer_state.explorer.explorer_focus_on_diff_list = false;
        }
        Some(Action::DeleteComment) if row_count > 0 => {
            app.request_delete_selected_review_item();
        }
        Some(Action::ToggleResolve) if row_count > 0 => {
            app.toggle_selected_review_status();
        }
        Some(Action::EditComment) => {
            app.start_edit_selected_review_item();
        }
        Some(Action::ReplyToComment) if row_count > 0 => {
            let comment_idx = app
                .review_state
                .selected_comment_idx(app.viewer_state.explorer.comment_list_selected);
            if let Some(idx) = comment_idx {
                app.review_state.input_buffer.clear();
                app.review_state.input_mode =
                    crate::review_state::ReviewInputMode::ReplyingToComment;
                app.review_state.selected = idx;
                app.review_state.status_message =
                    Some("Reply to comment (Enter to send, Esc to cancel)".to_string());
            }
        }
        Some(Action::NavigateDown)
            if row_count > 0 && app.viewer_state.explorer.comment_list_selected + 1 < row_count =>
        {
            app.viewer_state.explorer.comment_list_selected += 1;
        }
        Some(Action::NavigateUp) if app.viewer_state.explorer.comment_list_selected > 0 => {
            app.viewer_state.explorer.comment_list_selected -= 1;
        }
        Some(Action::GoToTop) => {
            app.viewer_state.explorer.comment_list_selected = 0;
        }
        Some(Action::GoToBottom) if row_count > 0 => {
            app.viewer_state.explorer.comment_list_selected = row_count - 1;
        }
        Some(Action::CollapseOrLeft) => {
            let visual = app.viewer_state.explorer.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual).cloned() {
                Some(CommentListRow::Reply { comment_idx, .. }) => {
                    if let Some(parent_visual) = app
                        .review_state
                        .comment_list_rows
                        .iter()
                        .position(|r| matches!(r, CommentListRow::Comment { comment_idx: ci } if *ci == comment_idx))
                    {
                        app.viewer_state.explorer.comment_list_selected = parent_visual;
                    }
                    app.toggle_comment_expansion();
                }
                Some(CommentListRow::Comment { comment_idx }) => {
                    if let Some(comment) = app.review_state.comments.get(comment_idx)
                        && app.review_state.expanded_comments.contains(&comment.id)
                    {
                        app.toggle_comment_expansion();
                    }
                }
                None => {}
            }
        }
        Some(Action::Select) | Some(Action::ExpandOrRight) => {
            let visual = app.viewer_state.explorer.comment_list_selected;
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
            let visual = app.viewer_state.explorer.comment_list_selected;
            if let Some(comment_idx) = app.review_state.selected_comment_idx(visual) {
                app.review_state.comment_detail_idx = comment_idx;
                app.review_state.comment_detail_scroll = 0;
                app.review_state.comment_detail_active = true;
                if let Some(comment) = app.review_state.comments.get(comment_idx) {
                    let cid = comment.id.clone();
                    if !app.review_state.cached_replies.contains_key(&cid)
                        && let Some(store) = app.review_store.as_ref()
                        && let Ok(replies) = store.get_replies(&cid)
                    {
                        app.review_state.cached_replies.insert(cid, replies);
                    }
                }
            }
        }
        _ => {}
    }

    if close_after {
        app.overlays.active = ActiveOverlay::None;
    }

    // Adjust scroll for comment list.
    let selected = app.viewer_state.explorer.comment_list_selected;
    let page_size = app.viewer_state.explorer.explorer_diff_list_height.max(1);
    if selected < app.viewer_state.explorer.comment_list_scroll {
        app.viewer_state.explorer.comment_list_scroll = selected;
    } else if selected >= app.viewer_state.explorer.comment_list_scroll + page_size {
        app.viewer_state.explorer.comment_list_scroll = selected.saturating_sub(page_size - 1);
    }
}

/// Navigate to the file and line of the comment at the given index.
/// When `focus_viewer` is true, the focus moves to the Viewer panel;
/// otherwise the current panel focus is preserved (e.g. comment list).
pub(in crate::event) fn navigate_to_comment_with_focus(
    app: &mut App,
    comment_idx: usize,
    focus_viewer: bool,
) {
    if let Some(comment) = app.review_state.comments.get(comment_idx) {
        let file_path = comment.file_path.clone();
        let line = comment.line_start as usize;
        if let Some(wt) = app.worktrees.get(app.selected_worktree) {
            let wt_path = wt.path.clone();
            let tab_width = app.config.viewer.tab_width;
            app.viewer_state.open_file(&wt_path, &file_path, tab_width);
            app.rehighlight_viewer();
            app.viewer_state.content.file_scroll = line.saturating_sub(1);
            app.viewer_state.selection = crate::viewer::LineSelection::Selected {
                start: line,
                end: line,
            };
            app.review_state.build_file_comment_cache(&file_path);
            if focus_viewer {
                app.set_focus(Focus::Viewer);
            }
        }
    }
}
