//! Mouse event handling — clicks, scrolls, drag interactions.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Focus};
use crate::overlay::ActiveOverlay;
use crate::terminal_link;

use super::explorer::{navigate_to_comment_with_focus, open_viewer_comment};
use super::terminal::{handle_terminal_tab_click, spawn_terminal_session};

/// Returns true if any overlay/modal is active and should consume all mouse events,
/// preventing them from reaching background panels.
fn has_blocking_overlay(app: &App) -> bool {
    use crate::app::{UpdateState, WorktreeInputMode};
    use crate::review_state::ReviewInputMode;

    app.worktree_mgr.skip_reason.is_some()
        || app.update_state != UpdateState::Idle
        || app.review_state.comment_detail_active
        || app.review_state.input_mode != ReviewInputMode::Normal
        || app.worktree_mgr.input_mode != WorktreeInputMode::Normal
        || app.overlays.active != ActiveOverlay::None
        || app.viewer_state.filename_search.filename_search_active
        || app.review_state.search_active
        || app.review_state.template_picker_active
        || app.references_overlay.active
        || app.symbol_action_overlay.active
}

/// Process a single mouse event, updating application state as needed.
pub fn handle_mouse_event(
    app: &mut App,
    mouse: MouseEvent,
    _frame_area: ratatui::layout::Rect,
) {
    // When any overlay/modal is active, consume all mouse events to prevent
    // them from reaching background panels (scroll, click, etc.).
    if has_blocking_overlay(app) {
        return;
    }

    // Read layout from cache (computed during render).
    let lc = &app.layout_cache;
    let notif_area = lc.notif_area;
    let main_area = lc.main_area;

    let left_w = lc.columns[0].width;
    let explorer_w = lc.columns[1].width;
    let viewer_w = lc.columns[2].width;
    let left_end = lc.columns[0].x + left_w;
    let explorer_end = lc.columns[1].x + explorer_w;
    let viewer_end = lc.columns[2].x + viewer_w;

    let explorer_mid_y = lc.explorer_mid_y;
    let terminal_split_y = lc.terminal_split[1].y;

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
                app.viewer_state.content.h_scroll = app.viewer_state.content.h_scroll.saturating_sub(4);
            }
        }
        MouseEventKind::ScrollRight => {
            if col >= explorer_end && col < viewer_end {
                app.viewer_state.scroll_right(4);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // Notification bar click — check for badge clicks.
            if notif_area.height > 0 && row == notif_area.y {
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
                        // Double-click detection.
                        let now = std::time::Instant::now();
                        let elapsed = now.duration_since(app.worktree_mgr.item_last_click);
                        let is_double = elapsed.as_millis() < 400
                            && app.worktree_mgr.item_last_click_idx == item_row;
                        app.worktree_mgr.item_last_click = now;
                        app.worktree_mgr.item_last_click_idx = item_row;

                        app.set_focus(Focus::Worktree);
                        app.worktree_list_selected = item_row;
                        app.sync_selected_worktree();
                        match app.worktree_list_rows[item_row] {
                            crate::app::WorktreeListRow::Session { pty_idx, .. } => {
                                app.on_worktree_changed();
                                app.terminal.active_claude_session = Some(pty_idx);
                                app.terminal.pty_manager.activate_session(pty_idx);
                                // Single click: keep focus on worktree panel.
                                // Double click: move focus to terminal.
                                if is_double {
                                    app.set_focus(Focus::TerminalClaude);
                                }
                            }
                            crate::app::WorktreeListRow::Worktree(_) => {
                                app.on_worktree_changed();
                                // Focus stays on worktree panel for both single and double click.
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
                        app.viewer_state.explorer.explorer_focus_on_diff_list = true;
                        let inner_y = explorer_mid_y + 1; // inside border
                        if row >= inner_y {
                            let click_offset = (row - inner_y) as usize;

                            if app.viewer_state.explorer.explorer_show_comments {
                                // Comment list is displayed — handle comment selection.
                                let idx = app.viewer_state.explorer.comment_list_scroll + click_offset;
                                let row_count = app.review_state.comment_list_rows.len();
                                if idx < row_count {
                                    app.viewer_state.explorer.comment_list_selected = idx;

                                    // Double-click detection.
                                    let now = std::time::Instant::now();
                                    let elapsed = now.duration_since(app.viewer_state.click.last_comment_click_time);
                                    let is_double = elapsed.as_millis() < 400
                                        && app.viewer_state.click.last_comment_click_idx == idx;
                                    app.viewer_state.click.last_comment_click_time = now;
                                    app.viewer_state.click.last_comment_click_idx = idx;

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
                                let idx = app.viewer_state.explorer.diff_list_scroll + click_offset;
                                if idx < app.diff_state.display_list.len() {
                                    app.viewer_state.explorer.diff_list_selected = idx;
                                    // Single-click: toggle header or open file in Viewer.
                                    if app.diff_state.toggle_section(idx) {
                                        // Toggled a section header.
                                        let new_count = app.diff_state.display_list.len();
                                        if new_count > 0
                                            && app.viewer_state.explorer.diff_list_selected >= new_count
                                        {
                                            app.viewer_state.explorer.diff_list_selected = new_count - 1;
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
                                            if let Some(pos) = app.viewer_state.diff_view.diff_view_lines.iter().position(|e| {
                                                matches!(e, crate::viewer::UnifiedDiffEntry::Line { tag, .. }
                                                    if *tag != crate::diff_state::DiffLineTag::Equal)
                                            }) {
                                                app.viewer_state.diff_view.diff_view_scroll = pos.saturating_sub(3);
                                            }

                                            app.set_focus(Focus::Viewer);
                                        }
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
                                        if !entry.is_expanded {
                                            if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                                                app.viewer_state.ensure_children_loaded(tree_idx, &wt.path);
                                            }
                                        }
                                        app.viewer_state.toggle_dir(tree_idx);
                                    } else if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                                        // Double-click detection.
                                        let now = std::time::Instant::now();
                                        let elapsed = now.duration_since(app.viewer_state.click.last_tree_click_time);
                                        let is_double = elapsed.as_millis() < 400
                                            && app.viewer_state.click.last_tree_click_idx == tree_idx;
                                        app.viewer_state.click.last_tree_click_time = now;
                                        app.viewer_state.click.last_tree_click_idx = tree_idx;

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

                    let inner_x = explorer_end + 1; // inside left border
                    let inner_y = main_area.y + 1; // inside top border
                    let gutter_w = app.viewer_state.gutter_total_width();
                    let on_gutter = col >= inner_x && col < inner_x + gutter_w;

                    // Cmd+Click (macOS) / Ctrl+Click — go-to-definition on the clicked symbol.
                    let has_jump_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
                        || mouse.modifiers.contains(KeyModifiers::CONTROL);
                    if has_jump_modifier && !on_gutter && !app.viewer_state.diff_view.diff_mode && row >= inner_y {
                        let badge_w: u16 = 2;
                        let content_start_x = inner_x + gutter_w + badge_w;
                        if col >= content_start_x {
                            let line_offset = (row - inner_y) as usize;
                            let line_1 = app.viewer_state.content.file_scroll + line_offset + 1;
                            let total_lines = app.viewer_state.content.file_content.len();
                            if line_1 <= total_lines {
                                let content_col = (col - content_start_x) as usize + app.viewer_state.content.h_scroll;
                                let line_text = &app.viewer_state.content.file_content[line_1 - 1];
                                if let Some((symbol, _, _)) = crate::app::extract_symbol_at_column(line_text, content_col) {
                                    handle_symbol_click_jump(app, &symbol, line_offset);
                                }
                            }
                        }
                        return;
                    }

                    // Only trigger comment selection when clicking inside the
                    // line-number gutter (left-most columns).  Clicks on the
                    // code content area are treated as plain focus changes.
                    if on_gutter {
                        // Detect clicks on viewer lines for comment selection.
                        if app.viewer_state.diff_view.diff_mode {
                            // Diff mode: resolve line number from diff_view_lines.
                            let diff_total = app.viewer_state.diff_view.diff_view_lines.len();
                            if diff_total > 0 && row >= inner_y {
                                let line_offset = (row - inner_y) as usize;
                                let idx = app.viewer_state.diff_view.diff_view_scroll + line_offset;
                                if let Some(crate::viewer::UnifiedDiffEntry::Line { new_line_no: Some(line_1), tag, .. }) = app.viewer_state.diff_view.diff_view_lines.get(idx) {
                                    if *tag != crate::diff_state::DiffLineTag::Delete {
                                        let line_1 = *line_1;
                                        let has_comment = app.review_state.file_comments.contains_key(&line_1);
                                        // Show comment preview on single click if the line has a comment.
                                        app.viewer_state.explorer.comment_preview_line = if has_comment { Some(line_1) } else { None };
                                        let should_open = app.viewer_state.click_line_number(line_1);
                                        if should_open {
                                            app.viewer_state.explorer.comment_preview_line = None;
                                            open_viewer_comment(app);
                                        }
                                    }
                                }
                            }
                        } else {
                            let total_lines = app.viewer_state.content.file_content.len();
                            if total_lines > 0 && row >= inner_y {
                                let line_offset = (row - inner_y) as usize;
                                let line_1 = app.viewer_state.content.file_scroll + line_offset + 1;

                                if line_1 <= total_lines {
                                    let has_comment = app.review_state.file_comments.contains_key(&line_1);
                                    // Show comment preview on single click if the line has a comment.
                                    app.viewer_state.explorer.comment_preview_line = if has_comment { Some(line_1) } else { None };
                                    let should_open = app.viewer_state.click_line_number(line_1);
                                    if should_open {
                                        app.viewer_state.explorer.comment_preview_line = None;
                                        open_viewer_comment(app);
                                    }
                                }
                            }
                        } // end non-diff-mode
                    }
                } else {
                    // Right column: top 80% = Claude, bottom 20% = Shell.
                    let terminal_x = viewer_end;

                    // Cmd+Click (macOS) / Ctrl+Click (Linux) — open file from terminal output.
                    let has_open_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
                        || mouse.modifiers.contains(KeyModifiers::CONTROL);

                    if has_open_modifier {
                        let (session_idx, content_y, scroll_offset) = if row < terminal_split_y {
                            (app.terminal.active_claude_session, main_area.y + 1, app.terminal.scroll_claude)
                        } else {
                            (app.terminal.active_shell_session, terminal_split_y + 1, app.terminal.scroll_shell)
                        };
                        if row > content_y {
                            if let Some(idx) = session_idx {
                                if let Some(screen_arc) = app.terminal.pty_manager.get_screen(idx) {
                                    let parser = screen_arc.lock().unwrap_or_else(|e| e.into_inner());
                                    let (_, cols) = parser.screen().size();
                                    let pty_row = row - content_y;
                                    let pty_col = col.saturating_sub(terminal_x) as usize;

                                    // Drop lock and re-acquire with scrollback.
                                    drop(parser);

                                    let text = {
                                        let mut p = screen_arc.lock().unwrap_or_else(|e| e.into_inner());
                                        p.set_scrollback(scroll_offset);
                                        let s = p.screen();
                                        let t = terminal_link::extract_row_text(s, pty_row, cols);
                                        p.set_scrollback(0);
                                        t
                                    };

                                    let wt_path = app.selected_worktree_path();
                                    let links = terminal_link::detect_file_links(&text, &wt_path);
                                    // Prefer the link under the cursor; fall back to first on row.
                                    let link = terminal_link::file_link_at_offset(&links, pty_col)
                                        .or_else(|| links.first());
                                    if let Some(link) = link {
                                        let path = link.path.clone();
                                        let line = link.line;
                                        app.open_file_in_viewer(&path, line);
                                        return;
                                    }
                                }
                            }
                        }
                        // If no link found, fall through to normal click behavior.
                    }

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
        MouseEventKind::Moved => {
            // Track hover line for gutter highlight in the viewer panel.
            let inner_y = main_area.y + 1;
            if col >= explorer_end && col < viewer_end && row >= inner_y && row < main_area.y + main_area.height.saturating_sub(1) {
                let line_offset = (row - inner_y) as usize;
                let inner_x = explorer_end + 1;
                let gutter_w = app.viewer_state.gutter_total_width();
                let on_gutter = col >= inner_x && col < inner_x + gutter_w;

                if app.viewer_state.diff_view.diff_mode {
                    let idx = app.viewer_state.diff_view.diff_view_scroll + line_offset;
                    let resolved = app.viewer_state.diff_view.diff_view_lines.get(idx).and_then(|e| match e {
                        crate::viewer::UnifiedDiffEntry::Line { new_line_no, .. } => *new_line_no,
                        _ => None,
                    });
                    app.viewer_state.click.hover_line = resolved;
                    app.viewer_state.click.hover_gutter_line = if on_gutter { resolved } else { None };
                } else {
                    let line_1 = app.viewer_state.content.file_scroll + line_offset + 1;
                    if line_1 <= app.viewer_state.content.file_content.len() {
                        app.viewer_state.click.hover_line = Some(line_1);
                        app.viewer_state.click.hover_gutter_line = if on_gutter { Some(line_1) } else { None };
                    } else {
                        app.viewer_state.click.hover_line = None;
                        app.viewer_state.click.hover_gutter_line = None;
                    }
                }

                // Cmd/Ctrl+hover: resolve symbol for underline display.
                let has_jump_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
                    || mouse.modifiers.contains(KeyModifiers::CONTROL);
                if has_jump_modifier && !app.viewer_state.diff_view.diff_mode {
                    let gutter_w = app.viewer_state.gutter_total_width();
                    let inner_x = explorer_end + 1;
                    let badge_w: u16 = 2;
                    let content_start_x = inner_x + gutter_w + badge_w;
                    if col >= content_start_x {
                        let line_1 = app.viewer_state.content.file_scroll + line_offset + 1;
                        let total_lines = app.viewer_state.content.file_content.len();
                        if line_1 <= total_lines {
                            let content_col = (col - content_start_x) as usize + app.viewer_state.content.h_scroll;
                            let line_text = &app.viewer_state.content.file_content[line_1 - 1];
                            if let Some((symbol, start, end)) = crate::app::extract_symbol_at_column(line_text, content_col) {
                                if app.can_jump_to_symbol(&symbol) {
                                    app.viewer_state.click.hover_symbol = Some(crate::viewer::HoverSymbol {
                                        text: symbol,
                                        line: line_1,
                                        start_col: start,
                                        end_col: end,
                                    });
                                } else {
                                    app.viewer_state.click.hover_symbol = None;
                                }
                            } else {
                                app.viewer_state.click.hover_symbol = None;
                            }
                        } else {
                            app.viewer_state.click.hover_symbol = None;
                        }
                    } else {
                        app.viewer_state.click.hover_symbol = None;
                    }
                } else {
                    app.viewer_state.click.hover_symbol = None;
                }
            } else {
                app.viewer_state.click.hover_line = None;
                app.viewer_state.click.hover_gutter_line = None;
                app.viewer_state.click.hover_symbol = None;
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
                        .explorer.diff_list_scroll
                        .saturating_add(delta.unsigned_abs() as usize)
                        .min(file_count.saturating_sub(1));
                } else {
                    app.viewer_state.explorer.diff_list_scroll = app
                        .viewer_state
                        .explorer.diff_list_scroll
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
                    .tree.tree_scroll
                    .saturating_add(delta.unsigned_abs() as usize)
                    .min(max_scroll);
            } else {
                app.viewer_state.tree.tree_scroll = app
                    .viewer_state
                    .tree.tree_scroll
                    .saturating_sub(delta.unsigned_abs() as usize);
            }
        }
    } else if col < viewer_end {
        // Viewer scroll.
        if app.viewer_state.diff_view.diff_mode {
            // Unified diff view scroll.
            let total = app.viewer_state.diff_view.diff_view_lines.len();
            if total > 0 {
                if delta > 0 {
                    app.viewer_state.diff_view.diff_view_scroll = (app.viewer_state.diff_view.diff_view_scroll
                        + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer_state.diff_view.diff_view_scroll = app
                        .viewer_state
                        .diff_view.diff_view_scroll
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
                        .content.file_scroll
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

/// Handle Cmd+Click jump-to-definition for a symbol in the viewer.
fn handle_symbol_click_jump(app: &mut App, symbol: &str, source_screen_row: usize) {
    use crate::app::StatusLevel;

    if !app.symbol_index.is_available() {
        app.set_status("Symbol index not ready yet".to_string(), StatusLevel::Warning);
        return;
    }

    let defs = app.symbol_index.find_definitions(symbol);

    // Context-aware: if cursor is at the definition site, show references instead.
    if app.is_cursor_at_definition(symbol) {
        // Already at definition — show references.
        let root = app.symbol_index.root();
        let refs = app.symbol_index.find_references(symbol, &root);
        if refs.is_empty() {
            app.set_status(format!("No references found for '{symbol}'"), StatusLevel::Warning);
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
            app.set_status(format!("No definition found for '{symbol}'"), StatusLevel::Warning);
        }
        1 => {
            let file = defs[0].file_path.clone();
            let line = defs[0].line;
            app.jump_to_location(&file, line, source_screen_row);
            app.set_status(format!("Jumped to definition of '{symbol}' (Ctrl+O to go back)"), StatusLevel::Success);
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
            app.set_status(format!("{n} definitions found for '{symbol}'"), StatusLevel::Info);
        }
    }
}
