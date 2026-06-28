//! Mouse event handling — clicks, scrolls, drag interactions.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Focus};
use crate::overlay::ActiveOverlay;
use crate::terminal_link;

use super::explorer::{navigate_to_comment_with_focus, open_viewer_comment};
use super::terminal::{handle_terminal_tab_click, spawn_terminal_session};

/// Maximum gap between two clicks (in milliseconds) to register as a double-click.
const DOUBLE_CLICK_MS: u128 = 400;

/// Record a click at `now` and report whether it forms a double-click with the
/// previous one stored in `last` (i.e. the gap is under [`DOUBLE_CLICK_MS`]).
/// Updates `*last` to `now`.
fn register_double_click(last: &mut std::time::Instant, now: std::time::Instant) -> bool {
    let is_double = now.duration_since(*last).as_millis() < DOUBLE_CLICK_MS;
    *last = now;
    is_double
}

/// Like [`register_double_click`] but also requires the click to land on the
/// same `idx` as the previous one. Updates both `*last` and `*last_idx`.
fn register_double_click_on(
    last: &mut std::time::Instant,
    last_idx: &mut usize,
    idx: usize,
    now: std::time::Instant,
) -> bool {
    let same_idx = *last_idx == idx;
    *last_idx = idx;
    // `register_double_click` always runs first so `*last` is updated regardless.
    register_double_click(last, now) && same_idx
}

/// Open the new-worktree creation dialog. Shared by the worktree bar's `[+]`
/// button and its blank-area double-click so the two entry points can't drift.
fn start_worktree_creation(app: &mut App) {
    use crate::app::{StatusLevel, WorktreeInputMode};
    app.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
    app.worktree_mgr.input_buffer.clear();
    app.set_status(
        "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string(),
        StatusLevel::Info,
    );
}

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

/// Resolve a screen row offset (relative to inner_y) to a 1-indexed file line
/// number, accounting for inline thread rows. Falls back to simple arithmetic
/// when no screen-row mapping is available.
fn resolve_screen_line(app: &App, screen_offset: usize) -> Option<usize> {
    let map = &app.viewer_state.content.screen_row_map;
    if !map.is_empty() {
        match map.get(screen_offset) {
            Some(crate::viewer::ScreenRow::Code(line)) => Some(*line),
            _ => None,
        }
    } else {
        let line_1 = app.viewer_state.content.file_scroll + screen_offset + 1;
        if line_1 <= app.viewer_state.content.file_content.len() {
            Some(line_1)
        } else {
            None
        }
    }
}

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

/// Resolve a screen row to a ThreadActions row, returning the comment_id.
fn resolve_screen_action(app: &App, screen_offset: usize) -> Option<String> {
    let map = &app.viewer_state.content.screen_row_map;
    match map.get(screen_offset) {
        Some(crate::viewer::ScreenRow::ThreadActions { comment_id }) => Some(comment_id.clone()),
        _ => None,
    }
}

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

/// Which of the four main columns a screen column falls into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Worktree,
    Explorer,
    Viewer,
    Terminal,
}

/// Per-frame layout geometry used for mouse hit-testing, snapshotted from the
/// layout cache at the start of [`handle_mouse_event`]. Bundling these values
/// keeps the per-column click handlers from each taking a long argument list.
#[derive(Debug, Clone, Copy)]
struct ClickGeometry {
    main_area: ratatui::layout::Rect,
    left_w: u16,
    explorer_w: u16,
    viewer_w: u16,
    left_end: u16,
    explorer_end: u16,
    viewer_end: u16,
    explorer_mid_y: u16,
    terminal_claude_y: u16,
    terminal_split_y: u16,
}

impl ClickGeometry {
    /// Determine which column the screen column `col` belongs to.
    fn column_at(&self, col: u16) -> Column {
        if col < self.left_end {
            Column::Worktree
        } else if col < self.explorer_end {
            Column::Explorer
        } else if col < self.viewer_end {
            Column::Viewer
        } else {
            Column::Terminal
        }
    }

    /// Hit-test the `[<=>]` expand button on the top border row, returning the
    /// panel whose button was clicked (if any). The caller must ensure the click
    /// is on the top border row before calling.
    fn expand_button_at(&self, col: u16) -> Option<Focus> {
        if col < self.left_end && self.left_w >= 7 {
            let btn_start = self.main_area.x + self.left_w - 6;
            let btn_end = self.main_area.x + self.left_w - 1;
            (col >= btn_start && col < btn_end).then_some(Focus::Worktree)
        } else if col >= self.left_end && col < self.explorer_end && self.explorer_w >= 7 {
            let btn_start = self.left_end + self.explorer_w - 6;
            let btn_end = self.left_end + self.explorer_w - 1;
            (col >= btn_start && col < btn_end).then_some(Focus::Explorer)
        } else if col >= self.explorer_end && col < self.viewer_end && self.viewer_w >= 7 {
            let btn_start = self.explorer_end + self.viewer_w - 6;
            let btn_end = self.explorer_end + self.viewer_w - 1;
            (col >= btn_start && col < btn_end).then_some(Focus::Viewer)
        } else {
            None
        }
    }
}

/// Handle a left click on the notification bar. Badge clicks jump to the
/// matching worktree. Returns `true` if the click was on the notification bar
/// (and thus consumed), regardless of whether a badge was hit.
fn handle_notification_bar_click(
    app: &mut App,
    col: u16,
    row: u16,
    notif_area: ratatui::layout::Rect,
) -> bool {
    if notif_area.height == 0 || row != notif_area.y {
        return false;
    }
    for (start_col, end_col, branch) in &app.notification_bar_badges {
        if col >= *start_col && col < *end_col {
            if let Some(wt_idx) = app.worktrees.iter().position(|w| w.branch == *branch) {
                app.selected_worktree = wt_idx;
                app.on_worktree_changed();
                app.set_focus(Focus::TerminalClaude);
            }
            return true;
        }
    }
    true
}

/// How many chips a single wheel tick scrolls the worktree strip: a screenful
/// minus one chip of overlap (at least 1). The visible count is read back from
/// the `Select` regions recorded by the last render.
fn wtbar_page_step(app: &App) -> usize {
    use crate::ui::worktree_bar::WtbarAction;
    let visible = app
        .wtbar_hits
        .iter()
        .filter(|h| matches!(h.action, WtbarAction::Select(_)))
        .count();
    visible.saturating_sub(1).max(1)
}

/// Handle a left click on the worktree bar: select (jump to the worktree and
/// its Claude session), delete (with confirmation), or add. Returns `true` if
/// the click landed on the bar row (and was thus consumed).
fn handle_wtbar_click(
    app: &mut App,
    col: u16,
    row: u16,
    wtbar_area: ratatui::layout::Rect,
) -> bool {
    if wtbar_area.height == 0 || row != wtbar_area.y {
        return false;
    }
    use crate::app::{StatusLevel, WorktreeInputMode};
    use crate::ui::worktree_bar::WtbarAction;

    let action = app
        .wtbar_hits
        .iter()
        .find(|h| col >= h.x0 && col < h.x1)
        .map(|h| h.action);

    match action {
        Some(WtbarAction::Select(i)) if i < app.worktrees.len() => {
            app.selected_worktree = i;
            app.on_worktree_changed();
            app.set_focus(Focus::TerminalClaude);
        }
        Some(WtbarAction::ScrollLeft) => {
            app.wtbar_scroll = app.wtbar_scroll.saturating_sub(1);
        }
        Some(WtbarAction::ScrollRight) => {
            app.wtbar_scroll = app.wtbar_scroll.saturating_add(1);
        }
        Some(WtbarAction::Add) => start_worktree_creation(app),
        Some(WtbarAction::Delete(i)) => {
            if let Some(wt) = app.worktrees.get(i) {
                if wt.is_main {
                    app.set_status(
                        "Cannot delete the main worktree.".to_string(),
                        StatusLevel::Error,
                    );
                } else if app.is_worktree_pending_delete(&wt.path) {
                    app.set_status(
                        "Worktree is already being deleted.".to_string(),
                        StatusLevel::Warning,
                    );
                } else {
                    let branch = wt.branch.clone();
                    app.selected_worktree = i;
                    app.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingDelete;
                    app.set_status(
                        format!("Delete worktree '{branch}'? (y/n)"),
                        StatusLevel::Warning,
                    );
                }
            }
        }
        // Blank area of the bar: a double-click acts as the `[+]` button. A
        // single click has nothing to do (the bar holds no focus), so it is
        // just consumed.
        None => {
            if register_double_click(
                &mut app.worktree_mgr.wtbar_blank_last_click,
                std::time::Instant::now(),
            ) {
                start_worktree_creation(app);
            }
        }
        // Stale/out-of-range `Select` hit from a prior render: ignore.
        Some(WtbarAction::Select(_)) => {}
    }
    true
}

/// Handle a left click on the title bar (above the main area). Clicking the
/// update badge starts the update flow. Returns `true` if the click was on the
/// title bar (and thus consumed).
fn handle_title_bar_click(
    app: &mut App,
    col: u16,
    row: u16,
    main_area: ratatui::layout::Rect,
) -> bool {
    if row >= main_area.y {
        return false;
    }
    if let Some((start, end)) = app.update_badge_cols
        && col >= start
        && col < end
        && app.update_info.is_some()
    {
        app.start_update_confirm();
    }
    true
}

/// Process a single mouse event, updating application state as needed.
pub fn handle_mouse_event(app: &mut App, mouse: MouseEvent, _frame_area: ratatui::layout::Rect) {
    // When any overlay/modal is active, consume all mouse events to prevent
    // them from reaching background panels (scroll, click, etc.).
    if has_blocking_overlay(app) {
        return;
    }

    // Read layout from cache (computed during render).
    let lc = &app.layout_cache;
    let notif_area = lc.notif_area;
    let wtbar_area = lc.wtbar_area;
    let main_area = lc.main_area;

    let left_w = lc.columns[0].width;
    let explorer_w = lc.columns[1].width;
    let viewer_w = lc.columns[2].width;
    let left_end = lc.columns[0].x + left_w;
    let explorer_end = lc.columns[1].x + explorer_w;
    let viewer_end = lc.columns[2].x + viewer_w;

    let explorer_mid_y = lc.explorer_mid_y;
    let terminal_claude_y = lc.terminal_split[0].y;
    let terminal_split_y = lc.terminal_split[1].y;

    let col = mouse.column;
    let row = mouse.row;

    let geom = ClickGeometry {
        main_area,
        left_w,
        explorer_w,
        viewer_w,
        left_end,
        explorer_end,
        viewer_end,
        explorer_mid_y,
        terminal_claude_y,
        terminal_split_y,
    };

    match mouse.kind {
        MouseEventKind::ScrollDown if wtbar_area.height > 0 && row == wtbar_area.y => {
            // Wheel over the worktree strip pages it sideways by ~a screenful
            // (one chip of overlap) so trackpad bursts and wheel detents both
            // move a useful amount without skipping chips.
            app.wtbar_scroll = app.wtbar_scroll.saturating_add(wtbar_page_step(app));
        }
        MouseEventKind::ScrollUp if wtbar_area.height > 0 && row == wtbar_area.y => {
            app.wtbar_scroll = app.wtbar_scroll.saturating_sub(wtbar_page_step(app));
        }
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll(app, col, row, main_area, left_end, explorer_end, viewer_end, explorer_mid_y, terminal_split_y, 3);
        }
        MouseEventKind::ScrollUp => {
            handle_mouse_scroll(app, col, row, main_area, left_end, explorer_end, viewer_end, explorer_mid_y, terminal_split_y, -3);
        }
        MouseEventKind::ScrollLeft
            // Horizontal scroll — only affects viewer panel.
            if col >= explorer_end && col < viewer_end => {
                app.viewer_state.content.h_scroll = app.viewer_state.content.h_scroll.saturating_sub(4);
            }
        MouseEventKind::ScrollRight
            if col >= explorer_end && col < viewer_end => {
                app.viewer_state.scroll_right(4);
            }
        MouseEventKind::Down(MouseButton::Left) => {
            // Notification / worktree / title bar clicks are consumed first.
            // The worktree bar must be checked before the title bar: the latter
            // treats every row above `main_area` as "title" and would otherwise
            // swallow the worktree strip's row.
            if handle_notification_bar_click(app, col, row, notif_area) {
                return;
            }
            if handle_wtbar_click(app, col, row, wtbar_area) {
                return;
            }
            if handle_title_bar_click(app, col, row, main_area) {
                return;
            }

            // Only handle clicks in the main area.
            if row < main_area.y || row >= main_area.y + main_area.height {
                return;
            }

            // The embedded editor occupies the merged Explorer+Viewer region; a
            // click anywhere in it just (re)focuses the editor — the Explorer and
            // Viewer panels behind it are hidden, so their click handlers must
            // not run. Clicks on the terminal column still fall through.
            if app.editor.is_some() && col >= left_end && col < viewer_end {
                app.set_focus(Focus::Editor);
                return;
            }

            // Check for [<=>] expand button clicks on the top border row.
            if row == main_area.y
                && let Some(target) = geom.expand_button_at(col) {
                    app.expanded_panel = if app.expanded_panel == Some(target) {
                        None
                    } else {
                        Some(target)
                    };
                    return;
                }

            match geom.column_at(col) {
                Column::Worktree => handle_worktree_column_click(app, row, &geom),
                Column::Explorer => handle_explorer_column_click(app, col, row, &geom),
                Column::Viewer => handle_viewer_column_click(app, mouse, col, row, &geom),
                Column::Terminal => handle_terminal_column_click(app, mouse, col, row, &geom),
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

                // Both diff and file-content views now populate `screen_row_map`
                // (the diff view injects inline comment threads), so a single
                // screen-row lookup resolves the hovered line in both modes.
                let resolved = resolve_screen_line(app, line_offset);
                app.viewer_state.click.hover_line = resolved;
                app.viewer_state.click.hover_gutter_line = if on_gutter { resolved } else { None };

                // Cmd/Ctrl+hover: resolve symbol for underline display.
                let has_jump_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
                    || mouse.modifiers.contains(KeyModifiers::CONTROL);
                if has_jump_modifier && !app.viewer_state.diff_view.diff_mode {
                    let gutter_w = app.viewer_state.gutter_total_width();
                    let inner_x = explorer_end + 1;
                    let badge_w: u16 = 2;
                    let content_start_x = inner_x + gutter_w + badge_w;
                    if col >= content_start_x {
                        if let Some(line_1) = resolve_screen_line(app, line_offset) {
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

/// Handle a left click in the Worktree column (worktree list / inline sessions).
fn handle_worktree_column_click(app: &mut App, row: u16, geom: &ClickGeometry) {
    let main_area = geom.main_area;
    // Click selects and switches to the worktree/session.
    let relative_row = (row - main_area.y) as usize;
    let item_row = relative_row.saturating_sub(1); // row 0 is border

    if !app.worktree_list_rows.is_empty() && item_row < app.worktree_list_rows.len() {
        // Double-click detection.
        let is_double = register_double_click_on(
            &mut app.worktree_mgr.item_last_click,
            &mut app.worktree_mgr.item_last_click_idx,
            item_row,
            std::time::Instant::now(),
        );

        app.set_focus(Focus::Worktree);
        app.worktree_list_selected = item_row;
        app.sync_selected_worktree();
        match app.worktree_list_rows[item_row] {
            crate::app::WorktreeListRow::Session { pty_idx, .. } => {
                app.on_worktree_changed();
                app.switch_claude_session(pty_idx);
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
        let is_double = register_double_click(
            &mut app.worktree_mgr.blank_last_click,
            std::time::Instant::now(),
        );

        if is_double {
            // Double-click → open worktree creation dialog.
            app.worktree_mgr.input_mode = crate::app::WorktreeInputMode::CreatingWorktree;
            app.worktree_mgr.input_buffer.clear();
        } else {
            // Single click → just focus.
            app.set_focus(Focus::Worktree);
        }
    }
}

/// Handle a left click in the Explorer column (file tree / diff list / comment list).
fn handle_explorer_column_click(app: &mut App, col: u16, row: u16, geom: &ClickGeometry) {
    let main_area = geom.main_area;
    let explorer_mid_y = geom.explorer_mid_y;
    let explorer_end = geom.explorer_end;

    app.set_focus(Focus::Explorer);

    // Determine if click is in top half (file tree) or bottom half (diff/comment list).
    if row >= explorer_mid_y {
        app.viewer_state.explorer.explorer_focus_on_diff_list = true;

        // Check for click on bottom border "✨ Ask Claude All" button.
        let bottom_border_y = main_area.y + main_area.height.saturating_sub(1);
        if row == bottom_border_y && app.viewer_state.explorer.explorer_show_comments {
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

            if app.viewer_state.explorer.explorer_show_comments {
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
            } else {
                // Diff list is displayed — handle diff selection.
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

/// Handle a left click in the Viewer column (symbol jump, comment threads, gutter).
fn handle_viewer_column_click(
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
            let screen_offset = (row - inner_y) as usize;
            if let Some(line_1) = resolve_screen_line(app, screen_offset) {
                let content_col =
                    (col - content_start_x) as usize + app.viewer_state.content.h_scroll;
                let line_text = &app.viewer_state.content.file_content[line_1 - 1];
                if let Some((symbol, _, _)) =
                    crate::app::extract_symbol_at_column(line_text, content_col)
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
            // digits+4, and the renderer indents digits + 6 (left_pad) + 4
            // ("  │ ") = digits + 10 = gutter_total_width() + 2 + 4.
            let content_x = inner_x + gutter_w + 2 + 4;
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
                    // Delete.
                    if let Some(store) = app.review_store.as_ref() {
                        let _ = store.delete_review(&comment_id);
                        let wt = app.selected_worktree_branch();
                        app.review_state.load_comments(store, &wt);
                        if let Some(file) = app.viewer_state.content.current_file.clone() {
                            app.review_state.build_file_comment_cache(&file);
                        }
                    }
                }
            }
            return;
        }
    }

    // Click anywhere in the left margin (line-number gutter + comment badge) of
    // a *commented* line toggles its inline thread. A generous hit target so it
    // works regardless of landing exactly on the 2-cell 💬 glyph, whose width
    // and column can drift with the terminal/font. Non-commented lines fall
    // through to the gutter selection path below.
    let badge_w: u16 = 2;
    let on_margin = col >= inner_x && col < inner_x + gutter_w + badge_w;
    if on_margin && row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        if let Some(line_1) = resolve_screen_line(app, screen_offset) {
            // Defensively refresh the per-file comment cache if it's stale (e.g.
            // a comment was created via MCP while a different file was current),
            // so the badge and the toggle gate agree.
            if app.review_state.file_comments_path.as_deref()
                != app.viewer_state.content.current_file.as_deref()
                && let Some(f) = app.viewer_state.content.current_file.clone()
            {
                app.review_state.build_file_comment_cache(&f);
            }
            if app.review_state.file_comments.contains_key(&line_1) {
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
                return;
            }
        }
    }

    // Click on an ExpandableContext row expands it. Inline threads shift screen
    // rows, so map the row back to its diff entry via the entry map.
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

    // Only trigger comment selection when clicking inside the
    // line-number gutter (left-most columns).  Clicks on the
    // code content area are treated as plain focus changes.
    if on_gutter && row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        // Screen-row mapping handles inline thread rows and both view modes
        // (deletion lines have no new-line number, so they resolve to None).
        if let Some(line_1) = resolve_screen_line(app, screen_offset) {
            let has_comment = app.review_state.file_comments.contains_key(&line_1);
            app.viewer_state.explorer.comment_preview_line =
                if has_comment { Some(line_1) } else { None };
            let should_open = app.viewer_state.click_line_number(line_1);
            if should_open {
                app.viewer_state.explorer.comment_preview_line = None;
                open_viewer_comment(app);
            }
        }
    }
}

/// Handle a left click in the right column (Claude terminal / Shell).
fn handle_terminal_column_click(
    app: &mut App,
    mouse: MouseEvent,
    col: u16,
    row: u16,
    geom: &ClickGeometry,
) {
    let main_area = geom.main_area;
    let viewer_end = geom.viewer_end;
    let terminal_claude_y = geom.terminal_claude_y;
    let terminal_split_y = geom.terminal_split_y;

    // Right column: top 80% = Claude, bottom 20% = Shell.
    let terminal_x = viewer_end;

    // Cmd+Click (macOS) / Ctrl+Click (Linux) — open file from terminal output.
    let has_open_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
        || mouse.modifiers.contains(KeyModifiers::CONTROL);

    if has_open_modifier {
        let (session_idx, content_y, scroll_offset) = if row < terminal_split_y {
            (
                app.terminal.active_claude_session,
                main_area.y + 1,
                app.terminal.scroll_claude,
            )
        } else {
            (
                app.terminal.active_shell_session,
                terminal_split_y + 1,
                app.terminal.scroll_shell,
            )
        };
        if row > content_y
            && let Some(idx) = session_idx
            && let Some(screen_arc) = app.terminal.pty_manager.get_screen(idx)
        {
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
            let link =
                terminal_link::file_link_at_offset(&links, pty_col).or_else(|| links.first());
            if let Some(link) = link {
                let path = link.path.clone();
                let line = link.line;
                app.open_file_in_viewer(&path, line);
                return;
            }
        }
        // If no link found, fall through to normal click behavior.
    }

    if row < terminal_split_y {
        app.set_focus(Focus::TerminalClaude);
        // Click on tab bar (first row of Claude panel).
        if row == terminal_claude_y {
            handle_terminal_tab_click(app, col, terminal_x, true);
        } else if app.current_worktree_claude_sessions().is_empty() {
            // Double-click required to spawn a new Claude Code session.
            if register_double_click(
                &mut app.terminal.claude_blank_last_click,
                std::time::Instant::now(),
            ) {
                spawn_terminal_session(app);
            }
        }
    } else {
        app.set_focus(Focus::TerminalShell);
        // Click on tab bar (first row of Shell panel).
        if row == terminal_split_y {
            handle_terminal_tab_click(app, col, terminal_x, false);
        } else if app.current_worktree_shell_sessions().is_empty() {
            // Double-click required to spawn a new Shell session.
            if register_double_click(
                &mut app.terminal.shell_blank_last_click,
                std::time::Instant::now(),
            ) {
                spawn_terminal_session(app);
            }
        }
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
        if app.viewer_state.diff_view.diff_mode {
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

/// Handle Cmd+Click jump-to-definition for a symbol in the viewer.
fn handle_symbol_click_jump(app: &mut App, symbol: &str, source_screen_row: usize) {
    use crate::app::StatusLevel;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Build a `ClickGeometry` with the given column boundaries. Widths/heights
    /// are set so that the `[<=>]` expand button (last 5 cols before each column
    /// border, requiring width >= 7) is testable.
    fn geom(left_end: u16, explorer_end: u16, viewer_end: u16) -> ClickGeometry {
        ClickGeometry {
            main_area: ratatui::layout::Rect::new(0, 1, viewer_end + 20, 40),
            left_w: left_end,
            explorer_w: explorer_end - left_end,
            viewer_w: viewer_end - explorer_end,
            left_end,
            explorer_end,
            viewer_end,
            explorer_mid_y: 20,
            terminal_claude_y: 1,
            terminal_split_y: 33,
        }
    }

    #[test]
    fn column_at_maps_columns_by_boundary() {
        let g = geom(20, 50, 90);
        assert_eq!(g.column_at(0), Column::Worktree);
        assert_eq!(g.column_at(19), Column::Worktree);
        assert_eq!(g.column_at(20), Column::Explorer);
        assert_eq!(g.column_at(49), Column::Explorer);
        assert_eq!(g.column_at(50), Column::Viewer);
        assert_eq!(g.column_at(89), Column::Viewer);
        assert_eq!(g.column_at(90), Column::Terminal);
        assert_eq!(g.column_at(200), Column::Terminal);
    }

    #[test]
    fn expand_button_hits_last_cols_of_each_column() {
        // main_area.x == 0, so the worktree button spans [left_w-6, left_w-1).
        let g = geom(20, 50, 90);
        // Worktree button: cols 14..19.
        assert_eq!(g.expand_button_at(14), Some(Focus::Worktree));
        assert_eq!(g.expand_button_at(18), Some(Focus::Worktree));
        assert_eq!(g.expand_button_at(19), None); // btn_end is exclusive
        assert_eq!(g.expand_button_at(13), None);
        // Explorer button: [left_end + explorer_w - 6, ...) = [44, 49).
        assert_eq!(g.expand_button_at(44), Some(Focus::Explorer));
        assert_eq!(g.expand_button_at(48), Some(Focus::Explorer));
        // Viewer button: [explorer_end + viewer_w - 6, ...) = [84, 89).
        assert_eq!(g.expand_button_at(84), Some(Focus::Viewer));
        assert_eq!(g.expand_button_at(88), Some(Focus::Viewer));
    }

    #[test]
    fn expand_button_absent_for_narrow_columns() {
        // A column narrower than 7 has no expand button.
        let g = geom(5, 50, 90);
        assert_eq!(g.expand_button_at(0), None);
        assert_eq!(g.expand_button_at(4), None);
    }

    #[test]
    fn double_click_within_threshold() {
        let t0 = Instant::now();
        let mut last = t0;
        // A click 100ms after the previous one is a double-click.
        let is_double = register_double_click(&mut last, t0 + Duration::from_millis(100));
        assert!(is_double);
        assert_eq!(last, t0 + Duration::from_millis(100));
    }

    #[test]
    fn single_click_beyond_threshold() {
        let t0 = Instant::now();
        let mut last = t0;
        // A click 400ms later is *not* a double-click (boundary is exclusive).
        assert!(!register_double_click(
            &mut last,
            t0 + Duration::from_millis(400)
        ));
        // And one well beyond the threshold is not either.
        let t1 = t0 + Duration::from_millis(400);
        assert!(!register_double_click(
            &mut last,
            t1 + Duration::from_millis(500)
        ));
    }

    #[test]
    fn indexed_double_click_requires_same_idx() {
        let t0 = Instant::now();
        let mut last = t0;
        let mut last_idx = 0usize;
        // First click on idx 5: even within the time window, the stored idx (0)
        // differs, so it is not a double-click.
        let first =
            register_double_click_on(&mut last, &mut last_idx, 5, t0 + Duration::from_millis(50));
        assert!(!first);
        assert_eq!(last_idx, 5);
        // Second click on the same idx within the window: double-click.
        let second =
            register_double_click_on(&mut last, &mut last_idx, 5, t0 + Duration::from_millis(100));
        assert!(second);
    }

    #[test]
    fn indexed_double_click_resets_on_different_idx() {
        let t0 = Instant::now();
        let mut last = t0;
        let mut last_idx = 3usize;
        // Quick click but on a different row → not a double-click, and the
        // stored index/time update so the next click compares against this one.
        let hit =
            register_double_click_on(&mut last, &mut last_idx, 7, t0 + Duration::from_millis(10));
        assert!(!hit);
        assert_eq!(last_idx, 7);
        assert_eq!(last, t0 + Duration::from_millis(10));
    }
}
