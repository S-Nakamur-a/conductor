//! Explorer のファイルツリー・diff リスト・walkthrough ステップリストが共有する
//! スクロール位置の管理。それぞれ選択中のインデックスを自分のスクロール窓の中に
//! 収める。

use crate::app::App;

/// tree_selected が見える位置に収まるよう tree_scroll を調整する。
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

/// diff_list_selected が見える位置に収まるよう diff_list_scroll を調整する。
pub(in crate::event) fn adjust_diff_list_scroll(app: &mut App) {
    let selected = app.viewer_state.explorer.diff_list_selected;
    let page_size = app.viewer_state.explorer.explorer_diff_list_height.max(1);

    if selected < app.viewer_state.explorer.diff_list_scroll {
        app.viewer_state.explorer.diff_list_scroll = selected;
    } else if selected >= app.viewer_state.explorer.diff_list_scroll + page_size {
        app.viewer_state.explorer.diff_list_scroll = selected.saturating_sub(page_size - 1);
    }
}

/// walkthrough_selected が見える位置に収まるよう walkthrough_scroll を調整する。
/// diff リストとは explorer_diff_list_height を共有している。両ビューは Explorer
/// 下部ペインの同じ矩形を占有し（排他的に表示されるため、どちらが表示中でも
/// 高さは常に最新の値になる）。
pub(in crate::event) fn adjust_walkthrough_scroll(app: &mut App) {
    let selected = app.viewer_state.explorer.walkthrough_selected;
    let page_size = app.viewer_state.explorer.explorer_diff_list_height.max(1);

    if selected < app.viewer_state.explorer.walkthrough_scroll {
        app.viewer_state.explorer.walkthrough_scroll = selected;
    } else if selected >= app.viewer_state.explorer.walkthrough_scroll + page_size {
        app.viewer_state.explorer.walkthrough_scroll = selected.saturating_sub(page_size - 1);
    }
}
