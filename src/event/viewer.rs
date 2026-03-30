//! Viewer panel key handling.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, StatusLevel};
use crate::keymap::{Action, KeyContext};

use super::explorer::open_viewer_comment_detail;

/// Handle keys when the Viewer panel is focused.
pub(super) fn handle_viewer_key(app: &mut App, key: KeyEvent) {
    // ── Inline reply input mode ──────────────────────────────────
    if app.viewer_state.explorer.inline_reply_line.is_some() {
        handle_inline_reply_input(app, key);
        return;
    }

    // Clear comment preview on any key input.
    app.viewer_state.explorer.comment_preview_line = None;

    // Unified diff mode has its own navigation.
    if app.viewer_state.diff_view.diff_mode {
        handle_viewer_diff_mode_key(app, key);
        return;
    }

    // ── pending 'g' key — symbol hints are shown, waiting for second key ──
    if app.viewer_state.pending_g_key {
        app.viewer_state.pending_g_key = false;
        match key.code {
            KeyCode::Char('d') => {
                app.symbol_hint_overlay = Default::default();
                handle_go_to_definition(app);
                return;
            }
            KeyCode::Char('i') => {
                app.symbol_hint_overlay = Default::default();
                handle_go_to_implementation(app);
                return;
            }
            KeyCode::Char('r') => {
                app.symbol_hint_overlay = Default::default();
                handle_find_references(app);
                return;
            }
            KeyCode::Char('g') => {
                // gg = go to top
                app.symbol_hint_overlay = Default::default();
                app.viewer_state.content.file_scroll = 0;
                return;
            }
            KeyCode::Esc => {
                app.symbol_hint_overlay = Default::default();
                return;
            }
            KeyCode::Char(c) if c.is_ascii_lowercase() => {
                // First character of a hint label — enter hint input mode.
                app.symbol_hint_overlay.input.push(c);
                return;
            }
            _ => {
                // Unknown second key — dismiss hints.
                app.symbol_hint_overlay = Default::default();
            }
        }
    }

    let total = app.viewer_state.content.file_content.len();
    let action = app.keymap.resolve(&key, KeyContext::Viewer);

    if let Some(Action::ExitToExplorer) = action {
        if app.viewer_state.selection != crate::viewer::LineSelection::None {
            app.viewer_state.clear_selection();
        } else {
            app.set_focus(crate::app::Focus::Explorer);
        }
        return;
    }

    if total == 0 {
        return;
    }

    match action {
        Some(Action::NavigateDown) => {
            if app.viewer_state.content.file_scroll + 1 < total {
                app.viewer_state.content.file_scroll += 1;
            }
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.content.file_scroll = app.viewer_state.content.file_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.content.file_scroll =
                (app.viewer_state.content.file_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.content.file_scroll = app.viewer_state.content.file_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => {
            // 'g' — show symbol hints and wait for second key (gd, gi, gr, gg, or hint label).
            app.viewer_state.pending_g_key = true;
            // Build hints using an estimated viewer height (will be clipped by actual content).
            let hints = app.build_symbol_hints(50);
            app.symbol_hint_overlay.active = !hints.is_empty();
            app.symbol_hint_overlay.hints = hints;
            app.symbol_hint_overlay.input.clear();
        }
        Some(Action::GoToBottom) => {
            app.viewer_state.content.file_scroll = total.saturating_sub(1);
        }
        Some(Action::SearchInFile) => {
            app.viewer_state.search.search_active = true;
            app.viewer_state.search.search_query.clear();
        }
        Some(Action::NextSearchMatch) => {
            app.viewer_state.next_search_match();
        }
        Some(Action::PrevSearchMatch) => {
            app.viewer_state.prev_search_match();
        }
        Some(Action::ScrollLeft) => {
            app.viewer_state.content.h_scroll = app.viewer_state.content.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer_state.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer_state.content.h_scroll = 0;
        }
        Some(Action::ToggleInlineThread) => {
            toggle_inline_thread(app);
        }
        Some(Action::InlineReply) => {
            start_inline_reply(app);
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        Some(Action::JumpBack) => {
            app.jump_back();
        }
        Some(Action::JumpForward) => {
            app.jump_forward();
        }
        _ => {}
    }
}

/// Key handling for the viewer panel in unified diff mode.
pub(super) fn handle_viewer_diff_mode_key(app: &mut App, key: KeyEvent) {
    let total = app.viewer_state.diff_view.diff_view_lines.len();
    let action = app.keymap.resolve(&key, KeyContext::ViewerDiffMode);

    if let Some(Action::ExitToExplorer) = action {
        if app.viewer_state.selection != crate::viewer::LineSelection::None {
            app.viewer_state.clear_selection();
        } else {
            app.viewer_state.exit_diff_mode();
            app.set_focus(crate::app::Focus::Explorer);
        }
        return;
    }

    if total == 0 {
        return;
    }

    match action {
        Some(Action::NavigateDown) => {
            if app.viewer_state.diff_view.diff_view_scroll + 1 < total {
                app.viewer_state.diff_view.diff_view_scroll += 1;
            }
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.diff_view.diff_view_scroll =
                app.viewer_state.diff_view.diff_view_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.diff_view.diff_view_scroll =
                (app.viewer_state.diff_view.diff_view_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.diff_view.diff_view_scroll =
                app.viewer_state.diff_view.diff_view_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => {
            app.viewer_state.diff_view.diff_view_scroll = 0;
        }
        Some(Action::GoToBottom) => {
            app.viewer_state.diff_view.diff_view_scroll = total.saturating_sub(1);
        }
        Some(Action::ScrollLeft) => {
            app.viewer_state.content.h_scroll = app.viewer_state.content.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer_state.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer_state.content.h_scroll = 0;
        }
        Some(Action::ToggleInlineThread) => {
            toggle_inline_thread(app);
        }
        Some(Action::InlineReply) => {
            start_inline_reply(app);
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        Some(Action::ExpandContext) => {
            // Expand 10 lines at the first visible ExpandableContext.
            if let Some(idx) = app.viewer_state.find_visible_expandable(50) {
                app.viewer_state.expand_context_at(idx, false);
            }
        }
        Some(Action::ExpandAllContext) => {
            // Expand all lines at the first visible ExpandableContext.
            if let Some(idx) = app.viewer_state.find_visible_expandable(50) {
                app.viewer_state.expand_context_at(idx, true);
            }
        }
        _ => {}
    }
}

// ── Inline thread helpers ────────────────────────────────────────────

/// Toggle inline thread expansion for the current cursor line.
fn toggle_inline_thread(app: &mut App) {
    let cursor_line = if let Some((start, _)) = app.viewer_state.selected_range() {
        start
    } else {
        app.viewer_state.content.file_scroll + 1
    };

    // Only toggle if the line has comments.
    if !app.review_state.file_comments.contains_key(&cursor_line) {
        return;
    }

    let threads = &mut app.viewer_state.explorer.expanded_inline_threads;
    if threads.contains(&cursor_line) {
        threads.remove(&cursor_line);
        // Also cancel any active reply on this line.
        if app.viewer_state.explorer.inline_reply_line == Some(cursor_line) {
            app.viewer_state.explorer.inline_reply_line = None;
            app.viewer_state.explorer.inline_reply_comment_id = None;
            app.viewer_state.explorer.inline_reply_buffer.clear();
        }
    } else {
        threads.insert(cursor_line);
        // Load replies if not cached.
        if let Some(comments) = app.review_state.file_comments.get(&cursor_line) {
            for comment in comments {
                if !app.review_state.cached_replies.contains_key(&comment.id) {
                    if let Some(store) = app.review_store.as_ref() {
                        if let Ok(replies) = store.get_replies(&comment.id) {
                            app.review_state.cached_replies.insert(comment.id.clone(), replies);
                        }
                    }
                }
            }
        }
    }
}

/// Start inline reply mode for the current cursor line.
///
/// If the thread is not expanded yet, expands it first and loads replies.
/// Targets the first comment on the line. If already replying to a comment on
/// this line, cycles to the next comment (for multi-comment lines).
fn start_inline_reply(app: &mut App) {
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
    if !app.viewer_state.explorer.expanded_inline_threads.contains(&cursor_line) {
        app.viewer_state.explorer.expanded_inline_threads.insert(cursor_line);
        // Load replies if not cached.
        for comment in comments {
            if !app.review_state.cached_replies.contains_key(&comment.id) {
                if let Some(store) = app.review_store.as_ref() {
                    if let Ok(replies) = store.get_replies(&comment.id) {
                        app.review_state.cached_replies.insert(comment.id.clone(), replies);
                    }
                }
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
fn handle_inline_reply_input(app: &mut App, key: KeyEvent) {
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
            let body = app.viewer_state.explorer.inline_reply_buffer.text().to_string();
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
        KeyCode::Backspace => {
            app.viewer_state.explorer.inline_reply_buffer.delete_backward();
        }
        KeyCode::Char(c) => {
            app.viewer_state.explorer.inline_reply_buffer.insert_char(c);
        }
        _ => {}
    }
}

// ── Code navigation handlers ──────────────────────────────────────────

fn handle_go_to_definition(app: &mut App) {
    let symbol = match app.get_symbol_at_cursor() {
        Some(s) => s,
        None => {
            app.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
            return;
        }
    };

    if !app.symbol_index.is_available() {
        app.set_status("Symbol index not ready yet".to_string(), StatusLevel::Warning);
        return;
    }

    // Context-aware: if cursor is at a definition site, show references instead.
    if app.is_cursor_at_definition(&symbol) {
        let root = app.symbol_index.root();
        let refs = app.symbol_index.find_references(&symbol, &root);
        if refs.is_empty() {
            app.set_status(format!("No references found for '{symbol}'"), StatusLevel::Warning);
        } else {
            let count = refs.len();
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = symbol.clone();
            app.references_overlay.results = refs;
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
            app.set_status(
                format!("At definition — showing {count} references for '{symbol}'"),
                StatusLevel::Info,
            );
        }
        return;
    }

    let defs = app.symbol_index.find_definitions(&symbol);
    match defs.len() {
        0 => {
            app.set_status(format!("No definition found for '{symbol}'"), StatusLevel::Warning);
        }
        1 => {
            let def = &defs[0];
            let file = def.file_path.clone();
            let line = def.line;
            app.jump_to_location(&file, line, 0);
            app.set_status(format!("Jumped to definition of '{symbol}'"), StatusLevel::Success);
        }
        n => {
            // Multiple definitions — show in references overlay.
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = format!("{symbol} (definitions)");
            app.references_overlay.results = defs
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
            app.set_status(format!("{n} definitions found for '{symbol}'"), StatusLevel::Info);
        }
    }
}

fn handle_go_to_implementation(app: &mut App) {
    let symbol = match app.get_symbol_at_cursor() {
        Some(s) => s,
        None => {
            app.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
            return;
        }
    };

    if !app.symbol_index.is_available() {
        app.set_status("Symbol index not ready yet".to_string(), StatusLevel::Warning);
        return;
    }

    let impls = app.symbol_index.find_implementations(&symbol);
    match impls.len() {
        0 => {
            app.set_status(format!("No implementations found for '{symbol}'"), StatusLevel::Warning);
        }
        1 => {
            let imp = &impls[0];
            let file = imp.file_path.clone();
            let line = imp.line;
            app.jump_to_location(&file, line, 0);
            app.set_status(format!("Jumped to implementation of '{symbol}'"), StatusLevel::Success);
        }
        n => {
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = format!("{symbol} (implementations)");
            app.references_overlay.results = impls
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
            app.set_status(format!("{n} implementations found for '{symbol}'"), StatusLevel::Info);
        }
    }
}

fn handle_find_references(app: &mut App) {
    let symbol = match app.get_symbol_at_cursor() {
        Some(s) => s,
        None => {
            app.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
            return;
        }
    };

    let root = app.symbol_index.root();
    let refs = app.symbol_index.find_references(&symbol, &root);

    if refs.is_empty() {
        app.set_status(format!("No references found for '{symbol}'"), StatusLevel::Warning);
        return;
    }

    app.references_overlay.active = true;
    app.references_overlay.symbol_name = symbol;
    app.references_overlay.results = refs;
    app.references_overlay.selected = 0;
    app.references_overlay.scroll = 0;
}
