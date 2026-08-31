//! 選択とスクロールを持つリストの位置決め。
//!
//! 可視高さをここに持たせていない。持たせると描画が書き戻すことになり、
//! 「いつ正しいか」がフレームの描画順に依存する。高さを決めるのはレイアウト
//! なので、操作のたびに [Viewport] として渡す。

use std::ops::Range;

/// リストに割り当てられた画面。`top` は最初の行の画面 y。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub top: u16,
    pub height: usize,
}

impl Viewport {
    pub fn new(top: u16, height: usize) -> Self {
        Self { top, height }
    }

    /// 枠で囲まれた矩形の内側。skip は先頭でバナーなどに食われる行数。
    pub fn inside(rect: ratatui::layout::Rect, skip: usize) -> Self {
        Self::new(
            rect.y + 1 + skip as u16,
            (rect.height.saturating_sub(2) as usize).saturating_sub(skip),
        )
    }
}

/// リストのどこを選んでいて、窓がどこにあるか。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListCursor {
    selected: usize,
    scroll: usize,
}

impl ListCursor {
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// 選択を delta だけ動かし、窓に入るまで追従させる。
    pub fn step(&mut self, delta: isize, len: usize, view: Viewport) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, len as isize - 1) as usize;
        self.reveal(len, view);
    }

    /// 指定の位置を選ぶ。範囲外は端に丸める。
    pub fn select(&mut self, index: usize, len: usize, view: Viewport) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        self.selected = index.min(len - 1);
        self.reveal(len, view);
    }

    /// 窓には触れず選択だけ置く。窓の高さを知らない場所から選び直すとき用で、
    /// 画面に入れるのは次に高さを知る側 (入力の入口) の仕事になる。
    pub fn place(&mut self, index: usize, len: usize) {
        self.selected = if len == 0 { 0 } else { index.min(len - 1) };
    }

    /// 先頭へ戻す。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 選択は動かさず窓だけ動かす。ホイール用。
    pub fn pan(&mut self, delta: isize, len: usize, view: Viewport) {
        let max = len.saturating_sub(view.height);
        let next = self.scroll as isize + delta;
        self.scroll = next.clamp(0, max as isize) as usize;
    }

    /// 選択が窓の中に入るよう窓を寄せる。
    pub fn reveal(&mut self, len: usize, view: Viewport) {
        let height = view.height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        self.scroll = self.scroll.min(len.saturating_sub(height));
    }

    /// 中身が入れ替わったあと、選択と窓を新しい長さに収める。
    pub fn clamp(&mut self, len: usize, view: Viewport) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        self.selected = self.selected.min(len - 1);
        self.reveal(len, view);
    }

    /// いま描くべき範囲。
    pub fn visible(&self, len: usize, view: Viewport) -> Range<usize> {
        let start = self.scroll.min(len);
        let end = (start + view.height).min(len);
        start..end
    }

    /// 画面 y に載っている要素。窓の外なら None。
    ///
    /// 逆算を呼び出し側に書かせない。書かせると、バナー行などで窓がずれた
    /// ときに補正のためのフィールドが状態側に生える。
    pub fn index_at(&self, y: u16, len: usize, view: Viewport) -> Option<usize> {
        if y < view.top {
            return None;
        }
        let offset = (y - view.top) as usize;
        if offset >= view.height {
            return None;
        }
        let index = self.scroll + offset;
        (index < len).then_some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: Viewport = Viewport { top: 3, height: 5 };

    #[test]
    fn 窓が動くのは選択が窓から出たときだけ() {
        let mut c = ListCursor::default();
        for (delta, selected, scroll) in [(4, 4, 0), (1, 5, 1), (-5, 0, 0)] {
            c.step(delta, 20, VIEW);
            assert_eq!((c.selected(), c.scroll()), (selected, scroll));
        }
    }

    #[test]
    fn stepは両端でクランプする() {
        let mut c = ListCursor::default();
        c.step(-1, 20, VIEW);
        assert_eq!(c.selected(), 0);
        c.step(999, 20, VIEW);
        assert_eq!(c.selected(), 19);
    }

    #[test]
    fn panは選択を動かさない() {
        let mut c = ListCursor::default();
        c.pan(3, 20, VIEW);
        assert_eq!((c.selected(), c.scroll()), (0, 3));
    }

    #[test]
    fn panは最後の項目が下端に着いたところで止まる() {
        let mut c = ListCursor::default();
        c.pan(999, 20, VIEW);
        assert_eq!(c.scroll(), 15);
    }

    #[test]
    fn 窓より短い一覧はスクロールしない() {
        let mut c = ListCursor::default();
        c.pan(999, 3, VIEW);
        c.step(2, 3, VIEW);
        assert_eq!((c.selected(), c.scroll()), (2, 0));
    }

    #[test]
    fn 一覧が縮むとclampが選択を引き戻す() {
        let mut c = ListCursor::default();
        c.select(18, 20, VIEW);
        c.clamp(4, VIEW);
        assert_eq!((c.selected(), c.scroll()), (3, 0));
    }

    #[test]
    fn 空の一覧は原点に戻る() {
        let mut c = ListCursor::default();
        c.select(5, 20, VIEW);
        c.clamp(0, VIEW);
        assert_eq!((c.selected(), c.scroll()), (0, 0));
    }

    #[test]
    fn 可視範囲は窓を一覧の長さに切ったもの() {
        let mut c = ListCursor::default();
        assert_eq!(c.visible(20, VIEW), 0..5);
        c.select(19, 20, VIEW);
        assert_eq!(c.visible(20, VIEW), 15..20);
        assert_eq!(c.visible(2, VIEW), 2..2);
    }

    #[test]
    fn index_atは画面行を戻し範囲外は受け付けない() {
        let mut c = ListCursor::default();
        c.select(19, 20, VIEW);
        assert_eq!(c.index_at(3, 20, VIEW), Some(15));
        assert_eq!(c.index_at(7, 20, VIEW), Some(19));
        for y in [2, 8, 100] {
            assert_eq!(c.index_at(y, 20, VIEW), None, "y={y} は窓の外");
        }
    }

    /// 窓は枠の内側だけを受け付ける。パネルが実際に描く行数 (枠 2 本ぶんを
    /// 引いた高さ) と一致していなければならない。ここがずれると、画面に出て
    /// いない行をクリックで開いてしまう。
    #[test]
    fn index_atは枠付きペインが実際に描く行だけを受け付ける() {
        let (y, h) = (7u16, 12u16);
        let view = Viewport::new(y + 1, h as usize - 2);
        let c = ListCursor::default();
        let hits: Vec<usize> = (y..y + h).filter_map(|r| c.index_at(r, 99, view)).collect();
        assert_eq!(hits, (0..h as usize - 2).collect::<Vec<_>>());
    }

    #[test]
    fn index_atは短い一覧の末尾より後ろを受け付けない() {
        let c = ListCursor::default();
        assert_eq!(c.index_at(4, 2, VIEW), Some(1));
        assert_eq!(c.index_at(5, 2, VIEW), None);
    }
}
