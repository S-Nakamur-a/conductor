//! Explorer の 2 つのリストのマウスホバー追跡。

use crate::ui::common::list_row::HoverRow;

/// どの行にマウスが乗っているかと、いま離れた行のフェードアウト状態。
///
/// ツリーと Changed files がホバー / 選択の優先規則をそれぞれ実装しないよう、
/// 共通の追跡型 (HoverRow) を 2 つ並べて持つ。
#[derive(Default)]
pub struct ListHover {
    /// Explorer 上半分のファイルツリー (可視リストの添字で追う)。
    pub explorer_tree: HoverRow,
    /// Explorer 下半分の Changed files (差分) リスト。
    pub diff_list: HoverRow,
}

impl ListHover {
    /// どちらかのフェードアニメーションが進行中か。
    pub fn is_animating(&self) -> bool {
        self.explorer_tree.is_animating() || self.diff_list.is_animating()
    }

    /// 両方のホバーを解除する。マウスがパネルから出たときなどに使う。
    pub fn clear(&mut self) {
        self.explorer_tree.set(None);
        self.diff_list.set(None);
    }
}
