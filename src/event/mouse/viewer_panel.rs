//! Click handling for the Viewer column: symbol jump, comment threads, the
//! left-margin gutter (comment marker / line numbers / run-test badge), and
//! `ExpandableContext` rows in diff view.

use crossterm::event::{KeyModifiers, MouseEvent};

use crate::app::{App, Focus, StatusLevel};

use super::super::explorer::open_viewer_comment;
use super::{ClickGeometry, resolve_screen_line};

/// Send a comment to the active Claude Code PTY via the address-conductor-comment skill.
fn ask_claude_about_comment(app: &mut App, comment_id: &str) {
    let prompt = format!("/conductor:address-conductor-comment {comment_id}\n");

    // Write to the active Claude Code session.
    if let Some(idx) = app.terminal.active_claude_session {
        if app.terminal.pty_manager.is_waiting_for_input(idx) {
            let _ = app
                .terminal
                .pty_manager
                .write_chunked_to_session(idx, &prompt);
        } else {
            // Queue as deferred prompt.
            app.terminal.deferred_prompts.insert(idx, prompt);
        }
        app.set_focus(Focus::TerminalClaude);
        app.set_status(
            "Sent comment to Claude".to_string(),
            crate::app::StatusLevel::Info,
        );
    } else {
        app.set_status(
            "No active Claude Code session".to_string(),
            crate::app::StatusLevel::Warning,
        );
    }
}

/// Send the clicked run button's test command to the active Shell PTY and focus
/// it. The command is auto-run (terminated with a newline). Language-agnostic —
/// the command (`go test …` or `cargo test …`) is built by the scanner.
fn run_test(app: &mut App, run: &crate::test_run::TestRun) {
    let Some(idx) = app.terminal.active_shell_session else {
        app.set_status(
            "No shell session to run tests".to_string(),
            StatusLevel::Warning,
        );
        return;
    };
    let line = format!("{}\n", run.command);
    if let Err(e) = app
        .terminal
        .pty_manager
        .write_chunked_to_session(idx, &line)
    {
        log::warn!("failed to send test command to shell: {e}");
        app.set_status(
            "Failed to send test command to shell".to_string(),
            StatusLevel::Warning,
        );
        return;
    }
    // Snap the Shell terminal to its live tail so the command is visible.
    app.terminal.scroll_shell = 0;
    app.set_focus(Focus::TerminalShell);
    app.set_status(format!("Running {}", run.label), StatusLevel::Info);
}

/// Resolve a screen row to a ThreadActions row, returning the comment_id.
fn resolve_screen_action(app: &App, screen_offset: usize) -> Option<String> {
    let map = &app.viewer_state.content.screen_row_map;
    match map.get(screen_offset) {
        Some(crate::viewer::ScreenRow::ThreadActions { comment_id }) => Some(comment_id.clone()),
        _ => None,
    }
}

/// Handle a left click in the Viewer column (symbol jump, comment threads, gutter).
pub(super) fn handle_viewer_column_click(
    app: &mut App,
    mouse: MouseEvent,
    col: u16,
    row: u16,
    geom: &ClickGeometry,
) {
    let main_area = geom.main_area;
    let explorer_end = geom.explorer_end;
    let viewer_end = geom.viewer_end;

    app.set_focus(Focus::Viewer);

    // Rendered markdown has no line numbers, so none of what follows (symbol
    // jump, comment threads, the gutter's comment/run-test zones) has a line to
    // resolve against. A click is a plain focus change and nothing more.
    if app.viewer_state.is_showing_rendered_markdown() {
        return;
    }

    let inner_x = explorer_end + 1; // inside left border
    let inner_y = main_area.y + 1; // inside top border
    let marker_w = crate::viewer::COMMENT_MARKER_W;
    let gutter_w = app.viewer_state.gutter_total_width();
    let on_gutter = col >= inner_x && col < inner_x + marker_w + gutter_w;

    // Cmd+Click (macOS) / Ctrl+Click — go-to-definition on the clicked symbol.
    let has_jump_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
        || mouse.modifiers.contains(KeyModifiers::CONTROL);
    if has_jump_modifier && !on_gutter && !app.viewer_state.diff_view.diff_mode && row >= inner_y {
        let badge_w: u16 = 2;
        let content_start_x = inner_x + marker_w + gutter_w + badge_w;
        if col >= content_start_x {
            let screen_offset = (row - inner_y) as usize;
            if let Some(line_1) = resolve_screen_line(app, screen_offset) {
                let content_col =
                    (col - content_start_x) as usize + app.viewer_state.content.h_scroll;
                // `.get`, not an index: `screen_row_map` is only rebuilt on
                // render, so a click processed in the same loop iteration as a
                // file-watcher reload resolves against the *previous* frame's
                // map. If the file shrank (Claude Code rewriting it, a `git
                // checkout`), that line number is now past the end — and
                // indexing would take the whole app down mid-click. The hover
                // path already guards this the same way.
                if let Some(line_text) = app.viewer_state.content.file_content.get(line_1 - 1)
                    && let Some((symbol, _, _)) = crate::app::masked_symbol_at_column(
                        line_text,
                        content_col,
                        line_1,
                        &app.viewer_state.content.code_mask,
                    )
                {
                    handle_symbol_click_jump(app, &symbol, screen_offset);
                }
            }
        }
        return;
    }

    // Handle clicks on thread action rows (reply / resolve / delete / ask).
    // Works in both diff and file-content views (both populate screen_row_map).
    if row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        if let Some(comment_id) = resolve_screen_action(app, screen_offset) {
            use crate::ui::viewer_panel::thread_actions;
            // Determine which action was clicked by column offset, using the
            // same layout constants the renderer draws the row with.
            // Offset equivalence with the renderer: gutter_total_width() is
            // digits+4, and the renderer indents marker(2) + digits + 6
            // (left_pad) + 4 ("  │ ") = marker + gutter_total_width() + 2 + 4.
            let content_x = inner_x + marker_w + gutter_w + 2 + 4;
            let click_col = col.saturating_sub(content_x) as usize;
            if click_col < thread_actions::reply_end() {
                // Reply: start inline reply for this comment.
                // Find which line this comment is on (end line).
                if let Some(comment) = app
                    .review_state
                    .comments
                    .iter()
                    .find(|c| c.id == comment_id)
                {
                    let end_line = comment.line_end.unwrap_or(comment.line_start) as usize;
                    if !app
                        .viewer_state
                        .explorer
                        .expanded_inline_threads
                        .contains(&end_line)
                    {
                        app.viewer_state
                            .explorer
                            .expanded_inline_threads
                            .insert(end_line);
                    }
                    app.viewer_state.explorer.inline_reply_line = Some(end_line);
                    app.viewer_state.explorer.inline_reply_comment_id = Some(comment_id);
                    app.viewer_state.explorer.inline_reply_buffer.clear();
                }
            } else if click_col < thread_actions::resolve_end() {
                // Resolve/unresolve.
                if let Some(store) = app.review_store.as_ref() {
                    let new_status = if let Some(c) = app
                        .review_state
                        .comments
                        .iter()
                        .find(|c| c.id == comment_id)
                    {
                        match c.status {
                            crate::review_store::CommentStatus::Pending => {
                                crate::review_store::CommentStatus::Resolved
                            }
                            crate::review_store::CommentStatus::Resolved => {
                                crate::review_store::CommentStatus::Pending
                            }
                        }
                    } else {
                        return;
                    };
                    let _ = store.update_review_status(&comment_id, new_status);
                    let wt = app.selected_worktree_branch();
                    app.review_state.load_comments(store, &wt);
                    if let Some(file) = app.viewer_state.content.current_file.clone() {
                        app.review_state.build_file_comment_cache(&file);
                    }
                }
            } else {
                // Check if click is on the right-side "ask claude" button.
                // Detect by absolute column: within its width of the right edge.
                let ask_claude_w = thread_actions::ask_claude_width() as u16 + 2;
                if col + ask_claude_w >= viewer_end {
                    // Ask Claude: send the comment to the active Claude PTY.
                    ask_claude_about_comment(app, &comment_id);
                } else {
                    // Delete (with confirmation).
                    app.request_delete_comment_by_id(comment_id);
                }
            }
            return;
        }
    }

    // Click on an ExpandableContext row expands it. Inline threads shift screen
    // rows, so map the row back to its diff entry via the entry map. (These
    // rows carry no line number, so they never collide with the margin
    // dispatch below.)
    if app.viewer_state.diff_view.diff_mode && row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        if let Some(idx) = app
            .viewer_state
            .diff_view
            .screen_entry_map
            .get(screen_offset)
            .copied()
            .flatten()
            && matches!(
                app.viewer_state.diff_view.diff_view_lines.get(idx),
                Some(crate::viewer::UnifiedDiffEntry::ExpandableContext { .. })
            )
        {
            app.viewer_state.expand_context_at(idx, false);
        }
    }

    // Left-margin dispatch. The margin is three zones with distinct jobs:
    //   - comment-marker column (far left, 💬/│) → toggles the existing
    //     inline thread; this is the only place thread focus lives;
    //   - line-number gutter → always starts a NEW comment, even on lines
    //     already covered by a comment range (overlapping/nested ranges);
    //   - 2-cell badge column → ▶ runs the test, otherwise "+" starts a new
    //     comment — identical on every line, commented or not.
    // Clicks on the code content area are treated as plain focus changes.
    let badge_w: u16 = 2;
    let on_marker = col >= inner_x && col < inner_x + marker_w;
    let gutter_start = inner_x + marker_w;
    let on_number_gutter = col >= gutter_start && col < gutter_start + gutter_w;
    let on_badge = col >= gutter_start + gutter_w && col < gutter_start + gutter_w + badge_w;
    if (on_marker || on_number_gutter || on_badge) && row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        // Screen-row mapping handles inline thread rows and both view modes
        // (deletion lines have no new-line number, so they resolve to None).
        if let Some(line_1) = resolve_screen_line(app, screen_offset) {
            // Defensively refresh the per-file comment cache if it's stale (e.g.
            // a comment was created via MCP while a different file was current),
            // so the badge and the dispatch below agree.
            if app.review_state.file_comments_path.as_deref()
                != app.viewer_state.content.current_file.as_deref()
                && let Some(f) = app.viewer_state.content.current_file.clone()
            {
                app.review_state.build_file_comment_cache(&f);
            }
            let zone = if on_marker {
                MarginZone::Marker
            } else if on_badge {
                MarginZone::Badge
            } else {
                MarginZone::NumberGutter
            };
            let has_comment = app.review_state.file_comments.contains_key(&line_1);
            // The ▶ marker is only drawn in file view — don't hit-test it in
            // diff view.
            let has_test_run = !app.viewer_state.diff_view.diff_mode
                && app.viewer_state.content.test_runs.contains_key(&line_1);
            let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
            match classify_margin_click(zone, has_comment, has_test_run, shift) {
                MarginClickAction::ToggleThread => toggle_inline_thread_at(app, line_1),
                MarginClickAction::RunTest => {
                    if let Some(run) = app.viewer_state.content.test_runs.get(&line_1).cloned() {
                        run_test(app, &run);
                    }
                }
                MarginClickAction::StartComment { extend: true } => {
                    // Shift+click extends a range from the previously clicked
                    // line and opens the composer immediately.
                    app.viewer_state.gutter_comment_click(line_1, true);
                    open_viewer_comment(app);
                }
                MarginClickAction::StartComment { extend: false } => {
                    // Plain press: begin a gutter drag. The selection starts as
                    // this single line and grows as the cursor is dragged over
                    // more lines; the composer opens on mouse-up (GitHub-style:
                    // click = one line, drag = a range).
                    app.viewer_state.gutter_comment_click(line_1, false);
                    app.viewer_state.click.gutter_drag_anchor = Some(line_1);
                }
            }
        }
    }
}

/// Which zone of the viewer's left margin a click landed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarginZone {
    /// The comment-marker column at the far left (💬 / │), before the numbers.
    Marker,
    /// The line-number gutter.
    NumberGutter,
    /// The 2-cell badge column right of the gutter (▶ / hover "+").
    Badge,
}

/// What a left click in the viewer's left margin does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarginClickAction {
    /// Toggle the inline comment thread injected below the clicked line.
    ToggleThread,
    /// Send the line's test command to the Shell PTY.
    RunTest,
    /// Start a new comment (`extend` = shift-click range extension).
    StartComment { extend: bool },
}

/// Decide what a left click in the viewer's left margin does.
///
/// Thread focus lives ONLY in the marker column (its 💬/│ glyphs mark the
/// thread); the number gutter and the "+" badge column always start a NEW
/// comment — even on lines already covered by an existing comment range — so
/// ranges that overlap or nest inside another comment's range stay creatable,
/// and the "+" affordance behaves identically on every line. The ▶ run-test
/// button keeps its spot in the badge column.
pub(super) fn classify_margin_click(
    zone: MarginZone,
    has_comment: bool,
    has_test_run: bool,
    shift: bool,
) -> MarginClickAction {
    match zone {
        MarginZone::Marker if has_comment => MarginClickAction::ToggleThread,
        MarginZone::Badge if has_test_run => MarginClickAction::RunTest,
        _ => MarginClickAction::StartComment { extend: shift },
    }
}

/// Where the inline thread for a badge click on `line_1` is anchored.
///
/// Threads are injected below a comment's END line (where its 💬 sits) — the
/// diff renderer draws them nowhere else — so a click on a mid-range │ line
/// redirects to the nearest covering end line instead of dead-toggling a line
/// that never shows a thread. On an end line the minimum is the line itself.
pub(super) fn thread_anchor_line(
    comments: &[crate::review_store::ReviewComment],
    line_1: usize,
) -> usize {
    comments
        .iter()
        .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
        .min()
        .unwrap_or(line_1)
}

/// Toggle the inline comment thread for the comment(s) covering `line_1`,
/// loading replies on first expansion and cancelling an in-progress reply on
/// collapse. Shared by the mouse (marker-column click) and keyboard toggles.
pub(in crate::event) fn toggle_inline_thread_at(app: &mut App, line_1: usize) {
    let line_1 = app
        .review_state
        .file_comments
        .get(&line_1)
        .map_or(line_1, |comments| thread_anchor_line(comments, line_1));
    let threads = &mut app.viewer_state.explorer.expanded_inline_threads;
    if threads.contains(&line_1) {
        threads.remove(&line_1);
        if app.viewer_state.explorer.inline_reply_line == Some(line_1) {
            app.viewer_state.explorer.inline_reply_line = None;
            app.viewer_state.explorer.inline_reply_comment_id = None;
            app.viewer_state.explorer.inline_reply_buffer.clear();
        }
    } else {
        threads.insert(line_1);
        if let Some(comments) = app.review_state.file_comments.get(&line_1) {
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
    }
}

/// Handle Cmd+Click jump-to-definition for a symbol in the viewer.
fn handle_symbol_click_jump(app: &mut App, symbol: &str, source_screen_row: usize) {
    if !app.symbol_index.is_available() {
        app.set_status(
            "Symbol index not ready yet".to_string(),
            StatusLevel::Warning,
        );
        return;
    }

    let defs = app.symbol_index.find_definitions(symbol);

    // Context-aware: if cursor is at the definition site, show references instead.
    if app.is_cursor_at_definition(symbol) {
        // Already at definition — show references.
        let root = app.symbol_index.root();
        let refs = app.symbol_index.find_references(symbol, &root);
        if refs.is_empty() {
            app.set_status(
                format!("No references found for '{symbol}'"),
                StatusLevel::Warning,
            );
        } else {
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = symbol.to_string();
            app.references_overlay.results = refs;
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
        }
        return;
    }

    match defs.len() {
        0 => {
            app.set_status(
                format!("No definition found for '{symbol}'"),
                StatusLevel::Warning,
            );
        }
        1 => {
            let file = defs[0].file_path.clone();
            let line = defs[0].line;
            app.jump_to_location(&file, line, source_screen_row);
            app.set_status(
                format!("Jumped to definition of '{symbol}' (Ctrl+O to go back)"),
                StatusLevel::Success,
            );
        }
        n => {
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
            app.set_status(
                format!("{n} definitions found for '{symbol}'"),
                StatusLevel::Info,
            );
        }
    }
}
