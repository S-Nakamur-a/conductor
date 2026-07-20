//! Scroll-position bookkeeping shared by the Explorer's file tree, diff list,
//! and walkthrough step list — each keeps the selected index visible within
//! its own scroll window.

use crate::app::App;

/// Adjust `tree_scroll` so that `tree_selected` stays visible.
pub(in crate::event) fn adjust_tree_scroll(app: &mut App) {
    let visible = app.viewer_state.visible_indices();
    let cur_vis = visible
        .iter()
        .position(|&i| i == app.viewer_state.tree.tree_selected)
        .unwrap_or(0);

    let page_size = app.viewer_state.explorer.explorer_tree_height.max(1);

    if cur_vis < app.viewer_state.tree.tree_scroll {
        app.viewer_state.tree.tree_scroll = cur_vis;
    } else if cur_vis >= app.viewer_state.tree.tree_scroll + page_size {
        app.viewer_state.tree.tree_scroll = cur_vis.saturating_sub(page_size - 1);
    }
}

/// Adjust `diff_list_scroll` so that `diff_list_selected` stays visible.
pub(in crate::event) fn adjust_diff_list_scroll(app: &mut App) {
    let selected = app.viewer_state.explorer.diff_list_selected;
    let page_size = app.viewer_state.explorer.explorer_diff_list_height.max(1);

    if selected < app.viewer_state.explorer.diff_list_scroll {
        app.viewer_state.explorer.diff_list_scroll = selected;
    } else if selected >= app.viewer_state.explorer.diff_list_scroll + page_size {
        app.viewer_state.explorer.diff_list_scroll = selected.saturating_sub(page_size - 1);
    }
}

/// Adjust `walkthrough_scroll` so that `walkthrough_selected` stays visible.
/// Shares `explorer_diff_list_height` with the diff list since both views
/// occupy the same Explorer bottom-pane rect (mutually exclusive, so the
/// height is always current for whichever one is showing).
pub(in crate::event) fn adjust_walkthrough_scroll(app: &mut App) {
    let selected = app.viewer_state.explorer.walkthrough_selected;
    let page_size = app.viewer_state.explorer.explorer_diff_list_height.max(1);

    if selected < app.viewer_state.explorer.walkthrough_scroll {
        app.viewer_state.explorer.walkthrough_scroll = selected;
    } else if selected >= app.viewer_state.explorer.walkthrough_scroll + page_size {
        app.viewer_state.explorer.walkthrough_scroll = selected.saturating_sub(page_size - 1);
    }
}
