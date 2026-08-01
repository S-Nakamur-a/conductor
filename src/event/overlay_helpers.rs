//! Cross-panel overlay entry/exit helpers — opening the filename-search
//! modal and dismissing whatever overlay is currently active.

use crate::app::{App, WorktreeInputMode};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

/// Open the fuzzy filename-search modal and seed it with the current
/// worktree's file list. Triggerable from both the Explorer (file tree) and
/// the Viewer, so files can be switched even while the viewer is maximized.
pub(in crate::event) fn open_filename_search(app: &mut App) {
    app.viewer_state.filename_search.filename_search_active = true;
    app.viewer_state
        .filename_search
        .filename_search_query
        .clear();
    app.viewer_state
        .filename_search
        .filename_search_results
        .clear();
    app.viewer_state.filename_search.filename_search_selected = 0;
    // 検索対象はツリーを構築したのと同じ根。ここで worktree 一覧の有無を条件に
    // すると、git 管理外のディレクトリで候補が常に空になり、検索が黙って死ぬ。
    let root = app.selected_worktree_path();
    app.viewer_state.populate_filename_search_cache(&root);
    app.viewer_state.execute_filename_search();
}

/// Dismiss all active overlays so that focus-switching keys work globally.
pub(super) fn dismiss_overlays(app: &mut App) {
    app.worktree_mgr.skip_reason = None;
    app.review_state.comment_detail_active = false;
    app.review_state.input_mode = ReviewInputMode::Normal;
    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
    app.overlays.active = ActiveOverlay::None;
    app.viewer_state.filename_search.filename_search_active = false;
    app.viewer_state.search.search_active = false;
    app.review_state.search_active = false;
    app.review_state.template_picker_active = false;
    app.code_nav.references.active = false;
    // A deliberate focus switch closes the hover modal stack too.
    app.clear_hover();
}
