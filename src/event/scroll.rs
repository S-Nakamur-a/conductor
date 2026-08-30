//! Explorer のファイルツリーと diff リストが共有する
//! スクロール位置の管理。それぞれ選択中のインデックスを自分のスクロール窓の中に
//! 収める。

use crate::app::App;

/// tree_selected が見える位置に収まるよう tree_scroll を調整する。
pub(in crate::event) fn adjust_tree_scroll(app: &mut App) {
    let visible = app.explorer.visible_indices();
    let cur_vis = visible
        .iter()
        .position(|&i| i == app.explorer.tree.tree_selected)
        .unwrap_or(0);

    let page_size = app.explorer.tree_height.max(1);

    if cur_vis < app.explorer.tree.tree_scroll {
        app.explorer.tree.tree_scroll = cur_vis;
    } else if cur_vis >= app.explorer.tree.tree_scroll + page_size {
        app.explorer.tree.tree_scroll = cur_vis.saturating_sub(page_size - 1);
    }
}

/// diff_list_selected が見える位置に収まるよう diff_list_scroll を調整する。
pub(in crate::event) fn adjust_diff_list_scroll(app: &mut App) {
    let selected = app.explorer.diff_list_selected;
    let page_size = app.explorer.diff_list_height.max(1);

    if selected < app.explorer.diff_list_scroll {
        app.explorer.diff_list_scroll = selected;
    } else if selected >= app.explorer.diff_list_scroll + page_size {
        app.explorer.diff_list_scroll = selected.saturating_sub(page_size - 1);
    }
}
