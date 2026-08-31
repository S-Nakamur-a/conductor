//! コメント用の行選択 — ガター上でのクリック / シフトクリックによる範囲選択。

use super::state::{LineSelection, ViewerState};

impl ViewerState {
    /// 現在の行選択をクリアする。
    pub fn clear_selection(&mut self) {
        self.selection = LineSelection::None;
    }

    /// 選択範囲を (start, end) で返す（どちらも1始まり、両端含む、
    /// start <= end になるよう正規化済み）。行が選択されていなければ None。
    pub fn selected_range(&self) -> Option<(usize, usize)> {
        match self.selection {
            LineSelection::None => None,
            LineSelection::Selected { start, end } => Some(if start <= end {
                (start, end)
            } else {
                (end, start)
            }),
        }
    }

    /// 1始まりの行番号が現在の選択範囲に含まれるかを判定する。
    pub fn is_line_selected(&self, line_1indexed: usize) -> bool {
        if let Some((start, end)) = self.selected_range() {
            line_1indexed >= start && line_1indexed <= end
        } else {
            false
        }
    }

    /// ガターの「+」ボタンのクリックを処理する（GitHub 風のコメント操作）。
    ///
    /// 通常のクリックは line_1indexed だけを選択する。シフトクリックは
    /// 直前にクリックした行（アンカー）を起点に範囲を広げる。アンカーは固定され、
    /// 連続するシフトクリックは常に同じ起点から伸びる。呼び出し側はその後
    /// コメント入力を開き、結果の selection を読み取る。
    pub fn gutter_comment_click(&mut self, line_1indexed: usize, extend: bool) {
        let anchor = self.click.last_line_click_line;
        if extend && anchor != 0 {
            let (start, end) = if anchor <= line_1indexed {
                (anchor, line_1indexed)
            } else {
                (line_1indexed, anchor)
            };
            self.selection = LineSelection::Selected { start, end };
        } else {
            self.selection = LineSelection::Selected {
                start: line_1indexed,
                end: line_1indexed,
            };
            self.click.last_line_click_line = line_1indexed;
            self.click.last_line_click_time = std::time::Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ガターのクリックは1行を選ぶ() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(7, false);
        assert_eq!(vs.selected_range(), Some((7, 7)));
    }

    #[test]
    fn shift付きは起点から範囲を伸ばす() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(5, false); // アンカーは5
        vs.gutter_comment_click(9, true); // シフトクリックで9まで拡張
        assert_eq!(vs.selected_range(), Some((5, 9)));
    }

    #[test]
    fn 上向きの範囲も正規化される() {
        let mut vs = ViewerState::default();
        vs.gutter_comment_click(9, false); // アンカーは9
        vs.gutter_comment_click(4, true); // アンカーより上をシフトクリック
        assert_eq!(vs.selected_range(), Some((4, 9)));
    }

    #[test]
    fn 起点が無ければ1行選択に落ちる() {
        let mut vs = ViewerState::default();
        // 事前のクリックがない → アンカーはデフォルトの0なので、単一行選択になる。
        vs.gutter_comment_click(3, true);
        assert_eq!(vs.selected_range(), Some((3, 3)));
    }
}
