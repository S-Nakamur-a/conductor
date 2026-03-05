//! Mouse event handling — clicks, scrolls, drag interactions.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Focus};

use super::explorer::{navigate_to_comment_with_focus, open_viewer_comment};
use super::terminal::{handle_terminal_tab_click, spawn_terminal_session};

/// Process a single mouse event, updating application state as needed.
pub fn handle_mouse_event(
    app: &mut App,
    mouse: MouseEvent,
    frame_area: ratatui::layout::Rect,
) {
    use ratatui::layout::{Constraint, Layout};

    // Compute layout regions — must match render_ui in main.rs.
    let notif_height: u16 = if !app.terminal.cc_waiting_worktrees.is_empty() { 1 } else { 0 };
    let outer = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Length(notif_height), // notification bar
        Constraint::Min(0),
        Constraint::Length(1), // status bar
    ])
    .split(frame_area);
    let notif_area = outer[1];
    let main_area = outer[2];

    let (left_w, explorer_w, viewer_w) = crate::accordion_widths(app.expanded_panel, main_area.width);

    let left_end = main_area.x + left_w;
    let explorer_end = left_end + explorer_w;
    let viewer_end = explorer_end + viewer_w;

    // Compute explorer panel's 50/50 vertical split — must match explorer_panel::render.
    let explorer_v_split = Layout::vertical([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(ratatui::layout::Rect::new(
        left_end,
        main_area.y,
        explorer_w,
        main_area.height,
    ));
    let explorer_mid_y = explorer_v_split[1].y;

    // Compute terminal panel's 80/20 vertical split — must match render_ui in main.rs.
    let right_w = main_area.width.saturating_sub(left_w + explorer_w + viewer_w);
    let terminal_v_split = Layout::vertical([
        Constraint::Percentage(80),
        Constraint::Percentage(20),
    ])
    .split(ratatui::layout::Rect::new(
        viewer_end,
        main_area.y,
        right_w,
        main_area.height,
    ));
    let terminal_split_y = terminal_v_split[1].y;

    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll(app, col, row, main_area, left_end, explorer_end, viewer_end, explorer_mid_y, terminal_split_y, 3);
        }
        MouseEventKind::ScrollUp => {
            handle_mouse_scroll(app, col, row, main_area, left_end, explorer_end, viewer_end, explorer_mid_y, terminal_split_y, -3);
        }
        MouseEventKind::ScrollLeft => {
            // Horizontal scroll — only affects viewer panel.
            if col >= explorer_end && col < viewer_end {
                app.viewer_state.h_scroll = app.viewer_state.h_scroll.saturating_sub(4);
            }
        }
        MouseEventKind::ScrollRight => {
            if col >= explorer_end && col < viewer_end {
                app.viewer_state.scroll_right(4);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // Notification bar click — check for badge clicks.
            if notif_height > 0 && row == notif_area.y {
                for (start_col, end_col, branch) in &app.notification_bar_badges {
                    if col >= *start_col && col < *end_col {
                        if let Some(wt_idx) =
                            app.worktrees.iter().position(|w| w.branch == *branch)
                        {
                            app.selected_worktree = wt_idx;
                            app.on_worktree_changed();
                            app.set_focus(Focus::TerminalClaude);
                        }
                        return;
                    }
                }
                return;
            }

            // Title bar click — check for update badge.
            if row < main_area.y {
                if let Some((start, end)) = app.update_badge_cols {
                    if col >= start && col < end && app.update_info.is_some() {
                        app.start_update_confirm();
                    }
                }
                return;
            }

            // Only handle clicks in the main area.
            if row >= main_area.y && row < main_area.y + main_area.height {
                // Check for [<=>] expand button clicks on the top border row.
                if row == main_area.y {
                    let expand_btn_target = if col < left_end && left_w >= 7 {
                        let btn_start = main_area.x + left_w - 6;
                        let btn_end = main_area.x + left_w - 1;
                        if col >= btn_start && col < btn_end { Some(Focus::Worktree) } else { None }
                    } else if col >= left_end && col < explorer_end && explorer_w >= 7 {
                        let btn_start = left_end + explorer_w - 6;
                        let btn_end = left_end + explorer_w - 1;
                        if col >= btn_start && col < btn_end { Some(Focus::Explorer) } else { None }
                    } else if col >= explorer_end && col < viewer_end && viewer_w >= 7 {
                        let btn_start = explorer_end + viewer_w - 6;
                        let btn_end = explorer_end + viewer_w - 1;
                        if col >= btn_start && col < btn_end { Some(Focus::Viewer) } else { None }
                    } else {
                        None
                    };
                    if let Some(target) = expand_btn_target {
                        if app.expanded_panel == Some(target) {
                            app.expanded_panel = None;
                        } else {
                            app.expanded_panel = Some(target);
                        }
                        return;
                    }
                }

                if col < left_end {
                    // Click selects and switches to the worktree/session.
                    let relative_row = (row - main_area.y) as usize;
                    let item_row = relative_row.saturating_sub(1); // row 0 is border

                    if !app.worktree_list_rows.is_empty() && item_row < app.worktree_list_rows.len() {
                        app.worktree_list_selected = item_row;
                        app.sync_selected_worktree();
                        match app.worktree_list_rows[item_row] {
                            crate::app::WorktreeListRow::Session { pty_idx, .. } => {
                                app.on_worktree_changed();
                                app.terminal.active_claude_session = Some(pty_idx);
                                app.terminal.pty_manager.activate_session(pty_idx);
                                app.set_focus(Focus::TerminalClaude);
                            }
                            crate::app::WorktreeListRow::Worktree(_) => {
                                app.on_worktree_changed();
                                app.set_focus(Focus::Worktree);
                            }
                        }
                    } else {
                        // Clicked on blank space below worktree items.
                        let now = std::time::Instant::now();
                        let elapsed = now.duration_since(app.worktree_mgr.blank_last_click);
                        app.worktree_mgr.blank_last_click = now;

                        if elapsed.as_millis() < 400 {
                            // Double-click → open worktree creation dialog.
                            app.worktree_mgr.input_mode =
                                crate::app::WorktreeInputMode::CreatingWorktree;
                            app.worktree_mgr.input_buffer.clear();
                        } else {
                            // Single click → just focus.
                            app.set_focus(Focus::Worktree);
                        }
                    }
                } else if col < explorer_end {
                    // Explorer column.
                    app.set_focus(Focus::Explorer);

                    // Determine if click is in top half (file tree) or bottom half (diff/comment list).
                    if row >= explorer_mid_y {
                        app.viewer_state.explorer_focus_on_diff_list = true;
                        let inner_y = explorer_mid_y + 1; // inside border
                        if row >= inner_y {
                            let click_offset = (row - inner_y) as usize;

                            if app.viewer_state.explorer_show_comments {
                                // Comment list is displayed — handle comment selection.
                                let idx = app.viewer_state.comment_list_scroll + click_offset;
                                let row_count = app.review_state.comment_list_rows.len();
                                if idx < row_count {
                                    app.viewer_state.comment_list_selected = idx;

                                    // Double-click detection.
                                    let now = std::time::Instant::now();
                                    let elapsed = now.duration_since(app.viewer_state.last_comment_click_time);
                                    let is_double = elapsed.as_millis() < 400
                                        && app.viewer_state.last_comment_click_idx == idx;
                                    app.viewer_state.last_comment_click_time = now;
                                    app.viewer_state.last_comment_click_idx = idx;

                                    // Navigate to the comment's file location.
                                    if let Some(comment_idx) =
                                        app.review_state.selected_comment_idx(idx)
                                    {
                                        // Single click: jump to location, keep focus on comments.
                                        // Double click: jump and focus Viewer.
                                        navigate_to_comment_with_focus(app, comment_idx, is_double);
                                    }
                                }
                            } else {
                                // Diff list is displayed — handle diff selection.
                                let idx = app.viewer_state.diff_list_scroll + click_offset;
                                if idx < app.diff_state.display_list.len() {
                                    app.viewer_state.diff_list_selected = idx;
                                    // Single-click: toggle header or open file in Viewer.
                                    if app.diff_state.toggle_section(idx) {
                                        // Toggled a section header.
                                        let new_count = app.diff_state.display_list.len();
                                        if new_count > 0
                                            && app.viewer_state.diff_list_selected >= new_count
                                        {
                                            app.viewer_state.diff_list_selected = new_count - 1;
                                        }
                                    } else if let Some((file_diff, _section)) =
                                        app.diff_state.resolve_file(idx)
                                    {
                                        let file_path = file_diff.path.clone();
                                        let file_diff_clone = file_diff.clone();
                                        if let Some(wt) =
                                            app.worktrees.get(app.selected_worktree)
                                        {
                                            let wt_path = wt.path.clone();
                                            let tab_width = app.config.viewer.tab_width;
                                            app.viewer_state.open_file(&wt_path, &file_path, tab_width);
                                            app.viewer_state
                                                .reveal_file_in_tree(&file_path, &wt_path);
                                            app.rehighlight_viewer();
                                            app.review_state
                                                .build_file_comment_cache(&file_path);

                                            // Build unified diff view.
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
                            }
                        }
                    } else {
                        app.viewer_state.explorer_focus_on_diff_list = false;
                        // Select the clicked file tree item.
                        let inner_y = main_area.y + 1; // inside border
                        if row >= inner_y {
                            let click_offset = (row - inner_y) as usize;
                            let visible = app.viewer_state.visible_indices();
                            let idx = app.viewer_state.tree_scroll + click_offset;
                            if let Some(&tree_idx) = visible.get(idx) {
                                app.viewer_state.tree_selected = tree_idx;
                                // Single-click opens the file in Viewer (or toggles dir).
                                if let Some(entry) = app.viewer_state.file_tree.get(tree_idx).cloned() {
                                    if entry.is_dir {
                                        // Lazy-load children before expanding.
                                        if !entry.is_expanded {
                                            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                                                app.viewer_state.ensure_children_loaded(tree_idx, &wt.path);
                                            }
                                        }
                                        app.viewer_state.toggle_dir(tree_idx);
                                    } else if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                                        // Double-click detection.
                                        let now = std::time::Instant::now();
                                        let elapsed = now.duration_since(app.viewer_state.last_tree_click_time);
                                        let is_double = elapsed.as_millis() < 400
                                            && app.viewer_state.last_tree_click_idx == tree_idx;
                                        app.viewer_state.last_tree_click_time = now;
                                        app.viewer_state.last_tree_click_idx = tree_idx;

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
                } else if col < viewer_end {
                    // Viewer column.
                    app.set_focus(Focus::Viewer);

                    // Detect clicks on the gutter (line number area) for line selection.
                    let panel_x = explorer_end;
                    let inner_x = panel_x + 1; // inside border
                    let inner_y = main_area.y + 1; // inside border

                    if app.viewer_state.diff_mode {
                        // Diff mode: resolve line number from diff_view_lines.
                        let diff_total = app.viewer_state.diff_view_lines.len();
                        if diff_total > 0 && row >= inner_y {
                            let line_offset = (row - inner_y) as usize;
                            let idx = app.viewer_state.diff_view_scroll + line_offset;
                            if let Some(crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(line_1), tag, .. }) = app.viewer_state.diff_view_lines.get(idx) {
                                if *tag != crate::diff_state::DiffLineTag::Delete {
                                    let line_1 = *line_1;
                                    let has_comment = app.review_state.file_comments.contains_key(&line_1);
                                    if has_comment {
                                        let now = std::time::Instant::now();
                                        let elapsed = now.duration_since(app.viewer_state.last_line_click_time);
                                        let is_double = elapsed.as_millis() < 400
                                            && app.viewer_state.last_line_click_line == line_1;
                                        app.viewer_state.last_line_click_time = now;
                                        app.viewer_state.last_line_click_line = line_1;

                                        if is_double {
                                            app.viewer_state.selected_line_start = Some(line_1);
                                            app.viewer_state.selected_line_end = None;
                                            app.viewer_state.comment_preview_line = None;
                                            open_viewer_comment(app);
                                        } else {
                                            app.viewer_state.clear_selection();
                                            app.viewer_state.comment_preview_line = Some(line_1);
                                        }
                                    } else {
                                        app.viewer_state.comment_preview_line = None;
                                        let is_double = app.viewer_state.click_line_number(line_1);
                                        if is_double {
                                            open_viewer_comment(app);
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                    let total_lines = app.viewer_state.file_content.len();
                    if total_lines > 0 && row >= inner_y {
                        let gutter_width = {
                            let mut count = 0usize;
                            let mut val = total_lines;
                            if val == 0 { 1 } else {
                                while val > 0 { count += 1; val /= 10; }
                                count
                            }
                        };
                        // Gutter: marker(1) + line_num(gutter_width) + " │ "(3) + badge(2)
                        let gutter_end_x = inner_x + (gutter_width as u16) + 6;

                        if col >= inner_x && col < gutter_end_x {
                            let line_offset = (row - inner_y) as usize;
                            let line_1 = app.viewer_state.file_scroll + line_offset + 1;

                            if line_1 <= total_lines {
                                let has_comment = app.review_state.file_comments.contains_key(&line_1);
                                if has_comment {
                                    // Double-click detection for comment lines
                                    let now = std::time::Instant::now();
                                    let elapsed = now.duration_since(app.viewer_state.last_line_click_time);
                                    let is_double = elapsed.as_millis() < 400
                                        && app.viewer_state.last_line_click_line == line_1;
                                    app.viewer_state.last_line_click_time = now;
                                    app.viewer_state.last_line_click_line = line_1;

                                    if is_double {
                                        app.viewer_state.selected_line_start = Some(line_1);
                                        app.viewer_state.selected_line_end = None;
                                        app.viewer_state.comment_preview_line = None;
                                        open_viewer_comment(app);
                                    } else {
                                        // Single click → comment preview only
                                        app.viewer_state.clear_selection();
                                        app.viewer_state.comment_preview_line = Some(line_1);
                                    }
                                } else {
                                    // No comment → existing line selection behavior
                                    app.viewer_state.comment_preview_line = None;
                                    let is_double = app.viewer_state.click_line_number(line_1);
                                    if is_double {
                                        open_viewer_comment(app);
                                    }
                                }
                            }
                        }
                    }
                    } // end non-diff-mode
                } else {
                    // Right column: top 80% = Claude, bottom 20% = Shell.
                    let terminal_x = viewer_end;

                    if row < terminal_split_y {
                        app.set_focus(Focus::TerminalClaude);
                        // Click on tab bar (first row of Claude panel).
                        if row == main_area.y {
                            handle_terminal_tab_click(app, col, terminal_x, true);
                        } else if app.current_worktree_claude_sessions().is_empty()
                        {
                            // Double-click required to spawn a new Claude Code session.
                            let now = std::time::Instant::now();
                            let elapsed =
                                now.duration_since(app.terminal.claude_blank_last_click);
                            app.terminal.claude_blank_last_click = now;
                            if elapsed.as_millis() < 400 {
                                spawn_terminal_session(app);
                            }
                        }
                    } else {
                        app.set_focus(Focus::TerminalShell);
                        // Click on tab bar (first row of Shell panel).
                        if row == terminal_split_y {
                            handle_terminal_tab_click(app, col, terminal_x, false);
                        } else if app.current_worktree_shell_sessions().is_empty()
                        {
                            // Double-click required to spawn a new Shell session.
                            let now = std::time::Instant::now();
                            let elapsed =
                                now.duration_since(app.terminal.shell_blank_last_click);
                            app.terminal.shell_blank_last_click = now;
                            if elapsed.as_millis() < 400 {
                                spawn_terminal_session(app);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Scroll the panel under the mouse cursor.
#[allow(clippy::too_many_arguments)]
fn handle_mouse_scroll(
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

    if col < left_end {
        // Worktree panel scroll.
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
    } else if col < explorer_end {
        // Explorer scroll.
        // Determine if scroll is in top half (file tree) or bottom half (diff list).
        if row >= explorer_mid_y {
            // Diff list scroll.
            let file_count = app.diff_state.display_list.len();
            if file_count > 0 {
                if delta > 0 {
                    app.viewer_state.diff_list_scroll = app
                        .viewer_state
                        .diff_list_scroll
                        .saturating_add(delta.unsigned_abs() as usize)
                        .min(file_count.saturating_sub(1));
                } else {
                    app.viewer_state.diff_list_scroll = app
                        .viewer_state
                        .diff_list_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else {
            // File tree scroll.
            let visible_count = app.viewer_state.visible_indices().len();
            let page = app.viewer_state.explorer_tree_height.max(1);
            let max_scroll = visible_count.saturating_sub(page);
            if delta > 0 {
                app.viewer_state.tree_scroll = app
                    .viewer_state
                    .tree_scroll
                    .saturating_add(delta.unsigned_abs() as usize)
                    .min(max_scroll);
            } else {
                app.viewer_state.tree_scroll = app
                    .viewer_state
                    .tree_scroll
                    .saturating_sub(delta.unsigned_abs() as usize);
            }
        }
    } else if col < viewer_end {
        // Viewer scroll.
        if app.viewer_state.diff_mode {
            // Unified diff view scroll.
            let total = app.viewer_state.diff_view_lines.len();
            if total > 0 {
                if delta > 0 {
                    app.viewer_state.diff_view_scroll = (app.viewer_state.diff_view_scroll
                        + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer_state.diff_view_scroll = app
                        .viewer_state
                        .diff_view_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else {
            let total = app.viewer_state.file_content.len();
            if total > 0 {
                if delta > 0 {
                    app.viewer_state.file_scroll = (app.viewer_state.file_scroll
                        + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer_state.file_scroll = app
                        .viewer_state
                        .file_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        }
    } else {
        // Terminal panels (right column).
        let abs_delta = delta.unsigned_abs() as usize;
        if row < terminal_split_y {
            if delta < 0 {
                // ScrollUp = scroll into history.
                app.terminal.scroll_claude = app.terminal.scroll_claude.saturating_add(abs_delta);
            } else {
                app.terminal.scroll_claude = app.terminal.scroll_claude.saturating_sub(abs_delta);
            }
        } else if delta < 0 {
            app.terminal.scroll_shell = app.terminal.scroll_shell.saturating_add(abs_delta);
        } else {
            app.terminal.scroll_shell = app.terminal.scroll_shell.saturating_sub(abs_delta);
        }
    }
}
