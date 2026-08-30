//! ファイルツリーと diff リストのスクロール位置。選択中のインデックスを
//! それぞれのスクロール窓の中に収める。

use super::ExplorerState;

impl ExplorerState {
    /// tree_selected が見える位置に収まるよう tree_scroll を調整する。
    pub(crate) fn adjust_tree_scroll(&mut self) {
        let visible = self.visible_indices();
        let cur_vis = visible
            .iter()
            .position(|&i| i == self.tree.tree_selected)
            .unwrap_or(0);

        let page_size = self.tree_height.max(1);

        if cur_vis < self.tree.tree_scroll {
            self.tree.tree_scroll = cur_vis;
        } else if cur_vis >= self.tree.tree_scroll + page_size {
            self.tree.tree_scroll = cur_vis.saturating_sub(page_size - 1);
        }
    }

    /// diff_list_selected が見える位置に収まるよう diff_list_scroll を調整する。
    pub(crate) fn adjust_diff_list_scroll(&mut self) {
        let selected = self.diff_list_selected;
        let page_size = self.diff_list_height.max(1);

        if selected < self.diff_list_scroll {
            self.diff_list_scroll = selected;
        } else if selected >= self.diff_list_scroll + page_size {
            self.diff_list_scroll = selected.saturating_sub(page_size - 1);
        }
    }
}
