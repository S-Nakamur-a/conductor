//! Explorer panel — file tree browser in the middle column.
//!
//! Displays the file tree of the currently selected worktree in the top half,
//! and a list of changed (diff) files in the bottom half. Enter on a file
//! opens it in the Viewer panel.
//!
//! Split by rendering responsibility: [`file_tree`] draws the top-half file
//! tree, [`diff_list`] the bottom-half changed-files list (and its comment
//! badge), [`comment_list`] the toggled review-comment list (both as the
//! bottom pane and as the full-screen `C` overlay), and [`search_box`] the
//! in-panel filename search input.

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

mod comment_list;
mod diff_list;
mod file_tree;
mod search_box;

pub use comment_list::render_comment_list_overlay;

/// Render the explorer (file tree) panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Explorer;

    // Split into top (file tree) and bottom (diff list), using the configured,
    // runtime-resizable ratio (Ctrl+Alt+↑/↓). Must match `LayoutCache`'s
    // `explorer_mid_y` so mouse routing lines up with what's drawn.
    let tree_pct = app.config.layout.explorer_split_pct;
    let changed_pct = 100u16.saturating_sub(tree_pct);
    let chunks = Layout::vertical([
        Constraint::Percentage(tree_pct),
        Constraint::Percentage(changed_pct),
    ])
    .split(area);

    // Record actual panel heights for scroll calculations in event handling.
    let tree_inner_height = chunks[0].height.saturating_sub(2) as usize;
    // The diff list gives its top row to the error banner, which is not a
    // `display_list` entry. Both the scroll page size and the mouse handler's
    // row→index conversion have to know that, so publish the row count from
    // here — the one place that also knows which view is on screen.
    let shows_error_banner = app.viewer_state.explorer.explorer_bottom_view
        == crate::viewer::ExplorerBottomView::DiffList
        && app.diff_state.error.is_some();
    let banner_rows = diff_list::diff_list_banner_rows(shows_error_banner);
    let diff_inner_height = (chunks[1].height.saturating_sub(2) as usize).saturating_sub(banner_rows);
    app.viewer_state.explorer.explorer_tree_height = tree_inner_height.max(1);
    app.viewer_state.explorer.explorer_diff_list_height = diff_inner_height.max(1);
    app.viewer_state.explorer.explorer_diff_banner_rows = banner_rows;

    file_tree::render_file_tree(frame, chunks[0], app, focused);
    match app.viewer_state.explorer.explorer_bottom_view {
        crate::viewer::ExplorerBottomView::Comments => {
            comment_list::render_comment_list(frame, chunks[1], app, focused);
        }
        crate::viewer::ExplorerBottomView::Walkthrough => {
            super::walkthrough_pane::render(frame, chunks[1], app, focused);
        }
        crate::viewer::ExplorerBottomView::DiffList => {
            diff_list::render_diff_list(frame, chunks[1], app, focused);
        }
    }

    // Show search input overlay (skip cursor positioning when a global overlay covers us).
    let overlay_active = app.is_any_overlay_active();
    if app.viewer_state.search.search_active {
        search_box::render_search_box(
            frame,
            area,
            &app.viewer_state.search.search_query,
            &app.theme,
            overlay_active,
        );
    }

    // The fuzzy filename-search modal is rendered at the top level
    // (see `layout::render_ui`) so it stays visible even when this panel is
    // collapsed to zero width (e.g. while the viewer is maximized).
}
