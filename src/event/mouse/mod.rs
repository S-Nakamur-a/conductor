//! Mouse event handling — clicks, scrolls, drag interactions.
//!
//! This module is the entry point (`handle_mouse_event`) plus the shared
//! hit-testing geometry (`ClickGeometry`/`Column`) and double-click helpers
//! used across the per-panel submodules. Each submodule owns one region of
//! the layout: [`bars`] (notification/worktree/title bars), [`worktree_panel`],
//! [`explorer_panel`], [`viewer_panel`], [`terminal_panel`], and [`scroll`]
//! (wheel scrolling for every panel).

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Focus};
use crate::overlay::ActiveOverlay;

use super::explorer::open_viewer_comment;

mod bars;
mod explorer_panel;
mod scroll;
mod terminal_panel;
mod viewer_panel;
mod worktree_panel;

#[cfg(test)]
mod tests;

use bars::{handle_notification_bar_click, handle_title_bar_click, handle_wtbar_click, wtbar_page_step};
use explorer_panel::handle_explorer_column_click;
use scroll::handle_mouse_scroll;
use terminal_panel::handle_terminal_column_click;
use viewer_panel::handle_viewer_column_click;
use worktree_panel::handle_worktree_column_click;

// Re-exported so `event::viewer`'s keyboard toggle can share the exact same
// thread-focus logic as the mouse's marker-column click.
pub(in crate::event) use viewer_panel::toggle_inline_thread_at;

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

/// Returns true if any overlay/modal is active and should consume all mouse events,
/// preventing them from reaching background panels.
/// Route a mouse event against the interactive hover modal stack. Returns
/// `true` when the event was consumed (the caller then returns early).
///
/// - Left click on `N refs` → open the references list (pins the popup).
/// - Left click on a list row → open its code preview.
/// - Left click on the preview → jump to that location, closing the stack.
/// - Left click on empty popup padding → kept (consumed).
/// - Left click anywhere else while pinned → dismiss (swallowed).
/// - Move over any part → keep alive (cancel the transient grace window).
/// - Scroll over the list → move the selection.
fn handle_hover_modal_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    if !app.hover_info_overlay.is_shown() {
        return false;
    }
    let col = mouse.column;
    let row = mouse.row;
    let in_rect = |r: ratatui::layout::Rect| {
        r.width > 0 && r.height > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
    };
    let pinned = app.hover_info_overlay.pinned;

    match mouse.kind {
        MouseEventKind::Moved => {
            if app.hover_point_hit(col, row) {
                app.hover_keep_alive();
                return true;
            }
            // Off the popup: while pinned, still consume so a stray move can't
            // clobber the modal; while transient, let the normal move handler
            // manage the candidate/grace.
            pinned
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // Level 2: click the preview → jump there and close everything.
            if let Some(pr) = app
                .hover_info_overlay
                .refs
                .as_ref()
                .and_then(|r| r.preview.as_ref())
                && in_rect(pr.rect)
            {
                app.hover_jump_to_preview();
                return true;
            }
            // Level 1: click a reference row → open its preview.
            if let Some(refs) = app.hover_info_overlay.refs.as_ref() {
                if let Some((idx, _)) = refs.row_hits.iter().find(|(_, r)| in_rect(*r)).copied() {
                    app.open_hover_preview(idx);
                    return true;
                }
                if in_rect(refs.rect) {
                    return true; // list padding — keep open
                }
            }
            // Base popup: click "N refs" → open the list; click body → keep.
            if in_rect(app.hover_info_overlay.refs_hit) {
                app.open_hover_refs();
                return true;
            }
            if in_rect(app.hover_info_overlay.info_rect) {
                return true;
            }
            // Outside everything: a pinned modal dismisses and swallows the
            // click; a transient popup lets the click through (the top-level
            // non-Moved clear will drop it).
            if pinned {
                app.clear_hover();
                app.dirty.mark_all();
                return true;
            }
            false
        }
        MouseEventKind::ScrollDown => {
            if app
                .hover_info_overlay
                .refs
                .as_ref()
                .is_some_and(|r| in_rect(r.rect))
            {
                app.hover_refs_move(1);
                return true;
            }
            pinned
        }
        MouseEventKind::ScrollUp => {
            if app
                .hover_info_overlay
                .refs
                .as_ref()
                .is_some_and(|r| in_rect(r.rect))
            {
                app.hover_refs_move(-1);
                return true;
            }
            pinned
        }
        _ => pinned,
    }
}

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

/// Whether `divider` can currently be grabbed for a mouse resize. Never while a
/// panel is maximized (the columns collapse to the edges, so the boundaries are
/// meaningless), and not the Explorer-side boundaries while the editor has
/// merged the Explorer+Viewer columns into a single PTY.
fn divider_draggable(app: &App, divider: crate::app::Divider) -> bool {
    use crate::app::Divider;
    if app.expanded_panel.is_some() {
        return false;
    }
    if app.editor.is_some() && matches!(divider, Divider::ExplorerViewer | Divider::ExplorerSplit) {
        return false;
    }
    true
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

    /// Hit-test a draggable panel divider at screen cell (`col`, `row`).
    ///
    /// Adjacent columns each render their own border, so a vertical boundary is
    /// a two-cell-thick line (the left panel's right border at `edge - 1` and
    /// the right panel's left border at `edge`); both cells count as a grab
    /// zone, and likewise for horizontal boundaries. Vertical dividers (column
    /// boundaries) win over horizontal ones (interior column splits) where they
    /// meet at a corner. Editor-merge and maximize gating is left to the caller.
    fn divider_at(&self, col: u16, row: u16) -> Option<crate::app::Divider> {
        use crate::app::Divider;

        let top = self.main_area.y;
        let bottom = self.main_area.y.saturating_add(self.main_area.height);
        let right = self.main_area.x.saturating_add(self.main_area.width);
        let on_boundary = |v: u16, edge: u16| edge > 0 && (v == edge - 1 || v == edge);

        // Vertical dividers: the full-height column boundaries.
        if row >= top && row < bottom {
            if on_boundary(col, self.explorer_end) {
                return Some(Divider::ExplorerViewer);
            }
            if on_boundary(col, self.viewer_end) {
                return Some(Divider::ViewerTerminal);
            }
        }
        // Horizontal dividers: splits interior to a single column.
        if col >= self.left_end
            && col < self.explorer_end
            && on_boundary(row, self.explorer_mid_y)
        {
            return Some(Divider::ExplorerSplit);
        }
        if col >= self.viewer_end && col < right && on_boundary(row, self.terminal_split_y) {
            return Some(Divider::TerminalSplit);
        }
        None
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

/// Process a single mouse event, updating application state as needed.
pub fn handle_mouse_event(app: &mut App, mouse: MouseEvent, _frame_area: ratatui::layout::Rect) {
    // Interactive hover modal stack (popup → refs list → preview) gets first
    // crack at the mouse: clicks on its parts drill in, moving over it keeps it
    // alive, and a pinned modal swallows clicks outside it (to dismiss).
    if handle_hover_modal_mouse(app, mouse) {
        return;
    }

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

    // Any mouse action other than a plain move (scroll, click, drag) invalidates
    // the auto-hover popup: it was tied to a now-stale line. Moved manages its
    // own candidate below.
    if !matches!(mouse.kind, MouseEventKind::Moved) && app.clear_hover() {
        app.dirty.mark_all();
    }

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

            // Grab a panel divider to begin a mouse resize. Checked before the
            // editor-refocus and column routing below so a boundary always wins
            // over the panel that sits on it (the [<=>] expand buttons live a few
            // cells inward, so they don't overlap the grab zone).
            if let Some(divider) = geom.divider_at(col, row)
                && divider_draggable(app, divider)
            {
                app.divider_drag = Some(divider);
                app.divider_hover = Some(divider);
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
        MouseEventKind::Drag(MouseButton::Left) => {
            // A divider drag takes priority: move the grabbed boundary to track
            // the cursor. The clamped mutators reject out-of-bounds targets, so
            // dragging past a panel's minimum simply pins the divider there.
            if let Some(divider) = app.divider_drag {
                app.drag_divider_to(divider, col, row);
                return;
            }
            // Extend an in-progress gutter range selection to the dragged line.
            if let Some(anchor) = app.viewer_state.click.gutter_drag_anchor {
                let inner_y = main_area.y + 1;
                if row >= inner_y && col >= explorer_end && col < viewer_end {
                    let screen_offset = (row - inner_y) as usize;
                    if let Some(line) = resolve_screen_line(app, screen_offset) {
                        let (start, end) = if anchor <= line {
                            (anchor, line)
                        } else {
                            (line, anchor)
                        };
                        app.viewer_state.selection =
                            crate::viewer::LineSelection::Selected { start, end };
                    }
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // Finish a divider drag: persist the final ratios once (the drag
            // itself intentionally skips the per-event config write).
            if app.divider_drag.take().is_some() {
                app.divider_hover = geom.divider_at(col, row);
                app.persist_layout();
                return;
            }
            // Finish a gutter drag: commit the (single-line or range) selection
            // by opening the comment composer for it.
            let was_dragging = app.viewer_state.click.gutter_drag_anchor.take().is_some();
            if was_dragging {
                open_viewer_comment(app);
            }
        }
        MouseEventKind::Moved => {
            // Light up the divider under the cursor as a resize affordance — the
            // terminal stand-in for a col-/row-resize mouse cursor (a plain hover
            // never mutates a ratio; only a drag does).
            app.divider_hover = geom
                .divider_at(col, row)
                .filter(|&d| divider_draggable(app, d));

            // Track hover line for gutter highlight in the viewer panel.
            let inner_y = main_area.y + 1;
            if col >= explorer_end && col < viewer_end && row >= inner_y && row < main_area.y + main_area.height.saturating_sub(1) {
                let line_offset = (row - inner_y) as usize;
                let inner_x = explorer_end + 1;
                let gutter_w = app.viewer_state.gutter_total_width();
                // Include the comment-marker column (left) and the 2-cell "+"
                // badge column (right) so the "+" button stays lit (and
                // clickable) while the cursor is over the whole left margin.
                let badge_w: u16 = 2;
                let marker_w = crate::viewer::COMMENT_MARKER_W;
                let on_gutter =
                    col >= inner_x && col < inner_x + marker_w + gutter_w + badge_w;

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
                    let content_start_x =
                        inner_x + crate::viewer::COMMENT_MARKER_W + gutter_w + badge_w;
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

                // Auto-hover candidate: the symbol under the resting mouse, with
                // no modifier required. Works in both plain-file and diff mode:
                // `gutter_total_width()` already accounts for the diff `+/-`
                // prefix, so the content-start column is the same formula, and
                // `resolve_screen_line` yields the new-side file line (None on
                // deletion rows, which we skip). Feeds the idle debounce in
                // `tick_hover`; blank space / non-identifiers clear it.
                let auto_cand = {
                    let gutter_w = app.viewer_state.gutter_total_width();
                    let inner_x = explorer_end + 1;
                    let badge_w: u16 = 2;
                    let content_start_x =
                        inner_x + crate::viewer::COMMENT_MARKER_W + gutter_w + badge_w;
                    if col >= content_start_x {
                        resolve_screen_line(app, line_offset).and_then(|line_1| {
                            let content_col =
                                (col - content_start_x) as usize + app.viewer_state.content.h_scroll;
                            app.viewer_state
                                .content
                                .file_content
                                .get(line_1 - 1)
                                .and_then(|text| {
                                    crate::app::extract_symbol_at_column(text, content_col).map(
                                        |(symbol, start, _)| {
                                            // Anchor the popup at the symbol's
                                            // start column and this screen row.
                                            let anchor_col = content_start_x
                                                + (start.saturating_sub(
                                                    app.viewer_state.content.h_scroll,
                                                )) as u16;
                                            (symbol, line_1, row, anchor_col)
                                        },
                                    )
                                })
                        })
                    } else {
                        None
                    }
                };
                app.set_mouse_hover_candidate(auto_cand);
            } else {
                app.viewer_state.click.hover_line = None;
                app.viewer_state.click.hover_gutter_line = None;
                app.viewer_state.click.hover_symbol = None;
                app.set_mouse_hover_candidate(None);
            }
        }
        _ => {}
    }
}
