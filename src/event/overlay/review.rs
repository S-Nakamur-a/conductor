//! Overlays for the review-comment workflow: the comment detail popup,
//! the comment compose/edit/reply input, the review search filter, and the
//! comment template picker.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::keymap::{Action, KeyContext};
use crate::review_state::ReviewInputMode;
#[allow(unused_imports)]
use crate::review_store::CommentKind;

use crate::event::clipboard_paste;
use crate::event::explorer::submit_new_comment;

use super::overlay_list_nav;

// ── Overlay: comment detail ─────────────────────────────────────────────

pub(in crate::event) fn handle_comment_detail_key(app: &mut App, key: KeyEvent) {
    // Handle scroll navigation via keymap.
    if let Some(action) = app.keymap.resolve(&key, KeyContext::Overlay) {
        match action {
            Action::NavigateDown => {
                if app.review_state.comment_detail_scroll
                    < app.review_state.comment_detail_max_scroll
                {
                    app.review_state.comment_detail_scroll += 1;
                }
                return;
            }
            Action::NavigateUp => {
                if app.review_state.comment_detail_scroll > 0 {
                    app.review_state.comment_detail_scroll -= 1;
                }
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ') => {
            app.review_state.comment_detail_active = false;
        }
        KeyCode::Char('e') => {
            // Edit from the detail view.
            let idx = app.review_state.comment_detail_idx;
            if let Some(comment) = app.review_state.comments.get(idx) {
                app.review_state.input_buffer.set_text(&comment.body);
                app.review_state.input_mode = ReviewInputMode::EditingComment;
                app.review_state.selected = idx;
                app.review_state.comment_detail_active = false;
            }
        }
        KeyCode::Char('R') => {
            // Reply from the detail view.
            let idx = app.review_state.comment_detail_idx;
            app.review_state.input_buffer.clear();
            app.review_state.input_mode = ReviewInputMode::ReplyingToComment;
            app.review_state.selected = idx;
            app.review_state.comment_detail_active = false;
        }
        KeyCode::Delete => {
            // Delete from the detail view (with confirmation).
            let idx = app.review_state.comment_detail_idx;
            app.review_state.comment_detail_active = false;
            if let Some(id) = app.review_state.comments.get(idx).map(|c| c.id.clone()) {
                app.request_delete_comment_by_id(id);
            }
        }
        KeyCode::Char('r') => {
            // Toggle resolve from the detail view.
            let idx = app.review_state.comment_detail_idx;
            app.review_state.selected = idx;
            app.toggle_selected_review_status();
        }
        _ => {}
    }
}

// ── Overlay: review input ───────────────────────────────────────────────

pub(in crate::event) fn handle_review_input_key(app: &mut App, key: KeyEvent) {
    // Delete confirmation is a y/n prompt, not a text field — handle it first.
    if app.review_state.input_mode == ReviewInputMode::ConfirmingDelete {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                app.confirm_pending_delete();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.cancel_pending_delete();
            }
            _ => {}
        }
        return;
    }

    // Shift+Enter inserts a newline (multi-line editing).
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        app.review_state.input_buffer.insert_char('\n');
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.review_state.input_buffer.clear();
            app.review_state.input_anchor = None;
            app.review_state.editing_reply = None;
            app.review_state.input_mode = ReviewInputMode::Normal;
            app.review_state.status_message = None;
        }
        KeyCode::Enter => {
            let buffer = app.review_state.input_buffer.text().to_string();
            match app.review_state.input_mode {
                ReviewInputMode::AddingComment => {
                    // Inline compose: anchor known, buffer is body-only. Falls
                    // back to the legacy `file:line body` parse when no anchor
                    // (template picker / command palette entry points).
                    if let Some((file, start, end)) = app.review_state.input_anchor.take() {
                        let body = buffer.trim();
                        if body.is_empty() {
                            app.review_state.status_message =
                                Some("Comment body is empty.".to_string());
                        } else {
                            let kind = app.review_state.input_kind;
                            app.add_review_comment(
                                &file,
                                start,
                                end,
                                kind,
                                body,
                                crate::review_store::Author::User,
                            );
                        }
                    } else {
                        submit_new_comment(app, &buffer);
                    }
                }
                ReviewInputMode::EditingComment => {
                    if !buffer.is_empty() {
                        app.update_selected_review_body(&buffer);
                    }
                }
                ReviewInputMode::EditingReply => {
                    if !buffer.is_empty() {
                        app.update_selected_reply_body(&buffer);
                    }
                }
                ReviewInputMode::ReplyingToComment => {
                    if !buffer.is_empty() {
                        app.add_reply_to_selected_comment(&buffer);
                    }
                }
                // ConfirmingDelete is intercepted above; Normal never reaches here.
                ReviewInputMode::Normal | ReviewInputMode::ConfirmingDelete => unreachable!(),
            }
            app.review_state.input_buffer.clear();
            app.review_state.editing_reply = None;
            app.review_state.input_mode = ReviewInputMode::Normal;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.review_state.input_buffer.delete_to_line_start();
        }
        KeyCode::Tab if app.review_state.input_mode == ReviewInputMode::AddingComment => {
            app.review_state.input_kind = match app.review_state.input_kind {
                CommentKind::Suggest => CommentKind::Question,
                CommentKind::Question => CommentKind::Suggest,
            };
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.review_state.input_buffer, true);
        }
        _ => {
            app.review_state.input_buffer.handle_key(key);
        }
    }
}

// ── Overlay: review search ──────────────────────────────────────────────

pub(in crate::event) fn handle_review_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.review_state.search_active = false;
            app.review_state.search_query.clear();
            app.review_state.apply_filter();
        }
        KeyCode::Enter => {
            app.review_state.search_active = false;
            app.review_state.apply_filter();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.review_state.search_query.delete_to_line_start();
            app.review_state.apply_filter();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.review_state.search_query, false);
            app.review_state.apply_filter();
        }
        _ => {
            if app.review_state.search_query.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.review_state.apply_filter();
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Overlay: review template picker ─────────────────────────────────────

pub(in crate::event) fn handle_review_template_key(app: &mut App, key: KeyEvent) {
    let count = app.review_state.templates.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.review_state.template_selected,
        count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            if let Some(tmpl) = app
                .review_state
                .templates
                .get(app.review_state.template_selected)
            {
                app.review_state.input_buffer.set_text(&tmpl.body);
                app.review_state.input_kind = tmpl.kind;
                app.review_state.input_mode = ReviewInputMode::AddingComment;
                app.review_state.status_message =
                    Some("Template loaded. Prefix with file:line then Enter.".to_string());
            }
            app.review_state.template_picker_active = false;
        }
        KeyCode::Esc => {
            app.review_state.template_picker_active = false;
        }
        KeyCode::Delete => {
            if let Some(tmpl) = app
                .review_state
                .templates
                .get(app.review_state.template_selected)
            {
                let id = tmpl.id.clone();
                app.delete_review_template(&id);
            }
            let new_count = app.review_state.templates.len();
            if new_count == 0 {
                app.review_state.template_picker_active = false;
            } else if app.review_state.template_selected >= new_count {
                app.review_state.template_selected = new_count - 1;
            }
        }
        _ => {}
    }
}
