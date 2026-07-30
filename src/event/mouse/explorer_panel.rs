//! Click handling for the Explorer column (file tree / diff list / comment list).

use crate::app::{App, Focus};

use super::super::explorer::navigate_to_comment_with_focus;
use super::{register_double_click_on, ClickGeometry};

/// Send all pending comments to Claude via /conductor:address-conductor-comment (no ID = bulk mode).
fn ask_claude_all_comments(app: &mut App) {
    let prompt = "/conductor:address-conductor-comment\n".to_string();
    if let Some(idx) = app.terminal.active_claude_session {
        if app.terminal.pty_manager.is_waiting_for_input(idx) {
            let _ = app
                .terminal
                .pty_manager
                .write_chunked_to_session(idx, &prompt);
        } else {
            app.terminal.deferred_prompts.insert(idx, prompt);
        }
        app.set_focus(Focus::TerminalClaude);
        app.set_status(
            "Sent all comments to Claude".to_string(),
            crate::app::StatusLevel::Info,
        );
    } else {
        app.set_status(
            "No active Claude Code session".to_string(),
            crate::app::StatusLevel::Warning,
        );
    }
}

/// Handle a left click in the Explorer column (file tree / diff list / comment list).
pub(super) fn handle_explorer_column_click(app: &mut App, col: u16, row: u16, geom: &ClickGeometry) {
    let main_area = geom.main_area;
    let explorer_mid_y = geom.explorer_mid_y;
    let explorer_end = geom.explorer_end;

    app.set_focus(Focus::Explorer);

    // Determine if click is in top half (file tree) or bottom half (diff/comment list).
    if row >= explorer_mid_y {
        app.viewer_state.explorer.explorer_focus_on_diff_list = true;

        // Check for click on bottom border "✨ Ask Claude All" button.
        let bottom_border_y = main_area.y + main_area.height.saturating_sub(1);
        if row == bottom_border_y
            && app.viewer_state.explorer.explorer_bottom_view
                == crate::viewer::ExplorerBottomView::Comments
        {
            // " ✨ Ask Claude All " is right-aligned, ~19 chars from right edge.
            let ask_label_w = 19_u16;
            let ask_start_col = explorer_end.saturating_sub(ask_label_w + 1);
            if col >= ask_start_col && col < explorer_end {
                ask_claude_all_comments(app);
                return;
            }
        }

        let inner_y = explorer_mid_y + 1; // inside border
        if row >= inner_y {
            let click_offset = (row - inner_y) as usize;

            if app.viewer_state.explorer.explorer_bottom_view
                == crate::viewer::ExplorerBottomView::Comments
            {
                // Comment list is displayed — handle comment selection.
                let idx = app.viewer_state.explorer.comment_list_scroll + click_offset;
                let row_count = app.review_state.comment_list_rows.len();
                if idx < row_count {
                    app.viewer_state.explorer.comment_list_selected = idx;

                    // Double-click detection.
                    let is_double = register_double_click_on(
                        &mut app.viewer_state.click.last_comment_click_time,
                        &mut app.viewer_state.click.last_comment_click_idx,
                        idx,
                        std::time::Instant::now(),
                    );

                    // Navigate to the comment's file location.
                    if let Some(comment_idx) = app.review_state.selected_comment_idx(idx) {
                        // Single click: jump to location, keep focus on comments.
                        // Double click: jump and focus Viewer.
                        navigate_to_comment_with_focus(app, comment_idx, is_double);
                    }
                }
            } else if app.viewer_state.explorer.explorer_bottom_view
                == crate::viewer::ExplorerBottomView::DiffList
            {
                // Diff list is displayed — handle diff selection.
                // The error banner occupies the top row(s) without being in
                // `display_list`, so every entry sits that much lower on screen
                // than its index suggests. Without this the click lands one file
                // off, and clicking the message itself opens whatever happens to
                // be scrolled to the top.
                let Some(click_offset) =
                    click_offset.checked_sub(app.viewer_state.explorer.explorer_diff_banner_rows)
                else {
                    return;
                };
                let idx = app.viewer_state.explorer.diff_list_scroll + click_offset;
                if idx < app.diff_state.display_list.len() {
                    app.viewer_state.explorer.diff_list_selected = idx;
                    // Single-click: SUMMARY pseudo-file opens the change summary.
                    if matches!(
                        app.diff_state.display_list.get(idx),
                        Some(crate::diff_state::DiffListEntry::Summary {})
                    ) {
                        app.viewer_state.enter_summary_view();
                        app.set_focus(Focus::Viewer);
                    }
                    // Single-click: toggle header or open file in Viewer.
                    else if app.diff_state.toggle_section(idx) {
                        // Toggled a section header.
                        let new_count = app.diff_state.display_list.len();
                        if new_count > 0
                            && app.viewer_state.explorer.diff_list_selected >= new_count
                        {
                            app.viewer_state.explorer.diff_list_selected = new_count - 1;
                        }
                    } else if app.diff_state.resolve_file(idx).is_some() {
                        // `diff_list_selected` already points at this row; the
                        // shared opener lands on the first comment if any.
                        app.open_diff_file_at_selected();
                        app.set_focus(Focus::Viewer);
                    }
                }
            }
        }
    } else {
        app.viewer_state.explorer.explorer_focus_on_diff_list = false;
        // Select the clicked file tree item.
        let inner_y = main_area.y + 1; // inside border
        if row >= inner_y {
            let click_offset = (row - inner_y) as usize;
            let visible = app.viewer_state.visible_indices();
            let idx = app.viewer_state.tree.tree_scroll + click_offset;
            if let Some(&tree_idx) = visible.get(idx) {
                app.viewer_state.tree.tree_selected = tree_idx;
                // Single-click opens the file in Viewer (or toggles dir).
                if let Some(entry) = app.viewer_state.tree.file_tree.get(tree_idx).cloned() {
                    if entry.is_dir {
                        // Lazy-load children before expanding.
                        if !entry.is_expanded
                            && let Some(wt) = app.worktrees.get(app.selected_worktree)
                        {
                            app.viewer_state.ensure_children_loaded(tree_idx, &wt.path);
                        }
                        app.viewer_state.toggle_dir(tree_idx);
                    } else if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                        // Double-click detection.
                        let is_double = register_double_click_on(
                            &mut app.viewer_state.click.last_tree_click_time,
                            &mut app.viewer_state.click.last_tree_click_idx,
                            tree_idx,
                            std::time::Instant::now(),
                        );

                        let wt_path = wt.path.clone();
                        let tab_width = app.config.viewer.tab_width;
                        app.viewer_state.open_file(&wt_path, &entry.path, tab_width);
                        app.rehighlight_viewer();
                        app.review_state.build_file_comment_cache(&entry.path);
                        // Single click: keep focus on Explorer.
                        // Double click: move focus to Viewer.
                        if is_double {
                            app.set_focus(Focus::Viewer);
                        }
                    }
                }
            }
        }
    }
}
