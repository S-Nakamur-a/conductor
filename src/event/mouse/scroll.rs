//! Wheel-scroll handling for every panel (worktree list, explorer, viewer,
//! terminals, embedded editor, reflow transcript view).

use crate::app::{App, Focus};

/// Scroll the panel under the mouse cursor.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_mouse_scroll(
    app: &mut App,
    col: u16,
    row: u16,
    main_area: ratatui::layout::Rect,
    left_end: u16,
    explorer_end: u16,
    viewer_end: u16,
    explorer_mid_y: u16,
    terminal_split_y: u16,
    delta: i32,
) {
    if row < main_area.y || row >= main_area.y + main_area.height {
        return;
    }

    // The editor occupies the merged Explorer+Viewer region. Translate the wheel
    // into arrow keys for the inner program (it runs on the alternate screen);
    // never scroll the hidden Explorer/Viewer state beneath it.
    if app.editor.is_some() && col >= left_end && col < viewer_end {
        if let Some(idx) = app.editor.as_ref().map(|e| e.session_idx) {
            // PTY grid is 1-based; the merged editor region starts at left_end
            // (left border) with content one cell in and one row down.
            let pty_col = col.saturating_sub(left_end).max(1);
            let pty_row = row.saturating_sub(main_area.y).max(1);
            app.terminal.pty_manager.forward_scroll_to_session(
                idx,
                delta.unsigned_abs() as usize,
                delta < 0,
                pty_col,
                pty_row,
            );
        }
        return;
    }

    if col < left_end {
        // Worktree panel scroll.
        let prev_wt = app.selected_worktree;
        if delta > 0 {
            if !app.worktree_list_rows.is_empty() {
                app.worktree_list_selected = (app.worktree_list_selected + 1)
                    .min(app.worktree_list_rows.len().saturating_sub(1));
                app.sync_selected_worktree();
            }
        } else {
            app.worktree_list_selected = app.worktree_list_selected.saturating_sub(1);
            app.sync_selected_worktree();
        }
        if app.selected_worktree != prev_wt {
            app.on_worktree_changed();
        }
    } else if col < explorer_end {
        // Explorer scroll.
        // Determine if scroll is in top half (file tree) or bottom half (diff list).
        if row >= explorer_mid_y {
            // Diff list scroll.
            let file_count = app.diff_state.display_list.len();
            if file_count > 0 {
                if delta > 0 {
                    app.viewer_state.explorer.diff_list_scroll = app
                        .viewer_state
                        .explorer
                        .diff_list_scroll
                        .saturating_add(delta.unsigned_abs() as usize)
                        .min(file_count.saturating_sub(1));
                } else {
                    app.viewer_state.explorer.diff_list_scroll = app
                        .viewer_state
                        .explorer
                        .diff_list_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else {
            // File tree scroll.
            let visible_count = app.viewer_state.visible_indices().len();
            let page = app.viewer_state.explorer.explorer_tree_height.max(1);
            let max_scroll = visible_count.saturating_sub(page);
            if delta > 0 {
                app.viewer_state.tree.tree_scroll = app
                    .viewer_state
                    .tree
                    .tree_scroll
                    .saturating_add(delta.unsigned_abs() as usize)
                    .min(max_scroll);
            } else {
                app.viewer_state.tree.tree_scroll = app
                    .viewer_state
                    .tree
                    .tree_scroll
                    .saturating_sub(delta.unsigned_abs() as usize);
            }
        }
    } else if col < viewer_end {
        // Viewer scroll.
        //
        // The summary pseudo-file is checked first: it renders over the whole
        // panel while `diff_mode` is false and `current_file` still points at
        // whatever was open behind it, so without this the wheel would scroll
        // that hidden file and the summary would sit motionless.
        if app.viewer_state.is_summary() {
            let total = app.viewer_state.summary_total_lines;
            if total > 0 {
                if delta > 0 {
                    app.viewer_state.summary_scroll = (app.viewer_state.summary_scroll
                        + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer_state.summary_scroll = app
                        .viewer_state
                        .summary_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else if app.viewer_state.diff_view.diff_mode {
            // Unified diff view scroll.
            let total = app.viewer_state.diff_view.diff_view_lines.len();
            if total > 0 {
                if delta > 0 {
                    app.viewer_state.diff_view.diff_view_scroll =
                        (app.viewer_state.diff_view.diff_view_scroll
                            + delta.unsigned_abs() as usize)
                            .min(total.saturating_sub(1));
                } else {
                    app.viewer_state.diff_view.diff_view_scroll = app
                        .viewer_state
                        .diff_view
                        .diff_view_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else {
            let total = app.viewer_state.content.file_content.len();
            if total > 0 {
                if delta > 0 {
                    app.viewer_state.content.file_scroll = (app.viewer_state.content.file_scroll
                        + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer_state.content.file_scroll = app
                        .viewer_state
                        .content
                        .file_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        }
    } else {
        // Terminal panels (right column).
        //
        // Focus the panel being scrolled so that wheel events take immediate
        // effect even when the panel does not currently hold keyboard focus.
        // This also satisfies the `focus == TerminalClaude` render guard in
        // terminal_claude.rs so reflow entry and display are consistent.
        // Note: set_focus(TerminalShell) closes reflow if it was active, which
        // is intentional — the user is deliberately scrolling away from Claude.
        if row < terminal_split_y {
            if app.focus != Focus::TerminalClaude {
                app.set_focus(Focus::TerminalClaude);
            }
        } else if app.focus != Focus::TerminalShell {
            app.set_focus(Focus::TerminalShell);
        }

        let abs_delta = delta.unsigned_abs() as usize;
        // ScrollUp (delta < 0) moves toward older content / into history.
        let up = delta < 0;
        let (session_idx, content_y) = if row < terminal_split_y {
            (app.terminal.active_claude_session, main_area.y + 1)
        } else {
            (app.terminal.active_shell_session, terminal_split_y + 1)
        };

        // Full-screen apps that own the screen handle the wheel themselves:
        // apps with mouse reporting on (vim/neovim, `less --mouse`) get an
        // encoded mouse event; alt-screen pagers without mouse reporting get
        // arrow keys. Either way the local scrollback offset is left alone.
        // PTY grid is 1-based; the terminal column starts at `viewer_end` (left
        // border) and the panel content starts at `content_y`.
        let pty_col = col.saturating_sub(viewer_end).max(1);
        let pty_row = row.saturating_sub(content_y).saturating_add(1);
        if let Some(idx) = session_idx
            && app
                .terminal
                .pty_manager
                .forward_scroll_to_session(idx, abs_delta, up, pty_col, pty_row)
        {
            return;
        }

        if row < terminal_split_y {
            if app.reflow.active {
                // While the reflow view is active, route wheel events into its
                // scroll offset.
                //
                // Scroll convention: scroll=0 is the oldest/top content; max is
                // newest/bottom. Wheel-up moves toward older content (subtract).
                //
                // Wheel-down past the logical bottom begins the exit sweep so
                // trackpad inertia carries the user naturally back to the live
                // tail (same experience as scrolling past the end of a document).
                // Wheel-up and wheel-down above the bottom adjust scroll normally.
                if up {
                    app.reflow.scroll = app.reflow.scroll.saturating_sub(abs_delta);
                } else {
                    let inner = app.reflow.last_inner_height as usize;
                    if crate::event::reflow::at_bottom(
                        app.reflow.scroll,
                        app.reflow.total_lines,
                        inner,
                    ) {
                        // Already at the bottom — exit sweep on further down scroll.
                        app.request_close_reflow();
                        return;
                    }
                    app.reflow.scroll = app.reflow.scroll.saturating_add(abs_delta);
                }
                let inner = app.reflow.last_inner_height as usize;
                let max_scroll = app.reflow.total_lines.saturating_sub(inner);
                app.reflow.scroll = app.reflow.scroll.min(max_scroll);
            } else if up {
                // Enter the reflow transcript view on the first upward scroll
                // from the live tail (scroll_claude == 0) instead of the
                // limited vt100 scrollback buffer. Wheel-down never triggers
                // entry; accidental upward inertia still opens the view but
                // the user can Esc back immediately.
                //
                // Skip entry when the worktree is grabbed: the visible PTY
                // runs on the main worktree's session while open_reflow would
                // look up the grabbed (source) worktree's history, producing a
                // mismatch.  Keyboard entry is already blocked by the grabbed-
                // worktree gate in handle_terminal_only_action.
                if app.terminal.scroll_claude == 0 && !app.is_selected_worktree_grabbed() {
                    app.open_reflow();
                } else {
                    app.terminal.scroll_claude =
                        app.terminal.scroll_claude.saturating_add(abs_delta);
                }
            } else {
                app.terminal.scroll_claude = app.terminal.scroll_claude.saturating_sub(abs_delta);
            }
        } else if up {
            app.terminal.scroll_shell = app.terminal.scroll_shell.saturating_add(abs_delta);
        } else {
            app.terminal.scroll_shell = app.terminal.scroll_shell.saturating_sub(abs_delta);
        }
    }
}
