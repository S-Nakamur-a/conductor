//! Inline comment thread toggling and reply composition for the viewer panel
//! (used by both the plain-file and unified-diff key handlers).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, StatusLevel};

/// Toggle inline thread expansion for the current cursor line.
pub(super) fn toggle_inline_thread(app: &mut App) {
    let cursor_line = if let Some((start, _)) = app.viewer_state.selected_range() {
        start
    } else {
        app.viewer_state.content.file_scroll + 1
    };

    // Only toggle if the line has comments. The shared helper redirects a
    // mid-range line to its thread's end-line anchor, matching the mouse path
    // (the diff view only renders threads at end lines).
    if !app.review_state.file_comments.contains_key(&cursor_line) {
        return;
    }
    crate::event::mouse::toggle_inline_thread_at(app, cursor_line);
}

/// Start inline reply mode for the current cursor line.
///
/// If the thread is not expanded yet, expands it first and loads replies.
/// Targets the first comment on the line. If already replying to a comment on
/// this line, cycles to the next comment (for multi-comment lines).
pub(super) fn start_inline_reply(app: &mut App) {
    let cursor_line = if let Some((start, _)) = app.viewer_state.selected_range() {
        start
    } else {
        app.viewer_state.content.file_scroll + 1
    };

    let comments = match app.review_state.file_comments.get(&cursor_line) {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    // Auto-expand the thread if not already expanded.
    if !app
        .viewer_state
        .explorer
        .expanded_inline_threads
        .contains(&cursor_line)
    {
        app.viewer_state
            .explorer
            .expanded_inline_threads
            .insert(cursor_line);
        // Load replies if not cached.
        for comment in comments {
            if !app.review_state.cached_replies.contains_key(&comment.id)
                && let Some(store) = app.review_store.as_ref()
                && let Ok(replies) = store.get_replies(&comment.id)
            {
                app.review_state
                    .cached_replies
                    .insert(comment.id.clone(), replies);
            }
        }
    }

    // If already replying on this line, cycle to the next comment.
    let target_id = if app.viewer_state.explorer.inline_reply_line == Some(cursor_line) {
        if let Some(current_id) = &app.viewer_state.explorer.inline_reply_comment_id {
            let current_pos = comments.iter().position(|c| &c.id == current_id);
            match current_pos {
                Some(pos) if pos + 1 < comments.len() => comments[pos + 1].id.clone(),
                _ => comments[0].id.clone(),
            }
        } else {
            comments[0].id.clone()
        }
    } else {
        comments[0].id.clone()
    };

    app.viewer_state.explorer.inline_reply_line = Some(cursor_line);
    app.viewer_state.explorer.inline_reply_comment_id = Some(target_id);
    app.viewer_state.explorer.inline_reply_buffer.clear();
}

/// Handle keys in inline reply input mode.
pub(super) fn handle_inline_reply_input(app: &mut App, key: KeyEvent) {
    // Shift+Enter inserts a newline; plain Enter submits — same convention as
    // the comment compose modal, so the inline reply is a real multi-line form.
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        app.viewer_state.explorer.inline_reply_buffer.insert_char('\n');
        return;
    }
    match key.code {
        KeyCode::Esc => {
            // Cancel reply.
            app.viewer_state.explorer.inline_reply_line = None;
            app.viewer_state.explorer.inline_reply_comment_id = None;
            app.viewer_state.explorer.inline_reply_buffer.clear();
        }
        KeyCode::Enter => {
            // Submit reply.
            if app.viewer_state.explorer.inline_reply_line.is_none() {
                return;
            }
            let body = app
                .viewer_state
                .explorer
                .inline_reply_buffer
                .text()
                .to_string();
            if body.trim().is_empty() {
                app.viewer_state.explorer.inline_reply_line = None;
                app.viewer_state.explorer.inline_reply_comment_id = None;
                app.viewer_state.explorer.inline_reply_buffer.clear();
                return;
            }

            // Use the explicitly tracked comment ID.
            let review_id = app.viewer_state.explorer.inline_reply_comment_id.clone();

            if let Some(review_id) = review_id {
                // Perform DB operations with a scoped borrow of the store.
                let result = if let Some(store) = app.review_store.as_ref() {
                    match store.add_reply(&review_id, &body, crate::review_store::Author::User) {
                        Ok(()) => {
                            let replies = store.get_replies(&review_id).ok();
                            let wt = app.selected_worktree_branch();
                            let counts = store.reply_counts_for_worktree(&wt).ok();
                            Ok((replies, counts))
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    Err(anyhow::anyhow!("No review store"))
                };

                match result {
                    Ok((replies, counts)) => {
                        app.set_status("Reply added.".to_string(), StatusLevel::Success);
                        if let Some(replies) = replies {
                            app.review_state.cached_replies.insert(review_id, replies);
                        }
                        if let Some(counts) = counts {
                            app.review_state.reply_counts = counts;
                        }
                    }
                    Err(e) => {
                        app.set_status(format!("Error: {e}"), StatusLevel::Error);
                    }
                }
            }

            app.viewer_state.explorer.inline_reply_line = None;
            app.viewer_state.explorer.inline_reply_comment_id = None;
            app.viewer_state.explorer.inline_reply_buffer.clear();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.viewer_state
                .explorer
                .inline_reply_buffer
                .delete_to_line_start();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            crate::event::clipboard_paste(
                app,
                |a| &mut a.viewer_state.explorer.inline_reply_buffer,
                true,
            );
        }
        _ => {
            // Full editing (chars, arrows, Home/End, word-move, Backspace/Delete).
            app.viewer_state.explorer.inline_reply_buffer.handle_key(key);
        }
    }
}
