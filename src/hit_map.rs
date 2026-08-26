//! 描いた結果できる、画面座標から中身への逆引き。
//!
//! バーやタブ列は自分が何をどこへ描いたかを知っているが、クリックを受け取るのは
//! 次のフレームの入力処理で、そちらは画面座標しか持たない。描画のたびにここへ
//! 記録し、入力側が引く。
//!
//! 同じ形の `Vec<Hit>` と `find` が tab_bar / worktree_bar / menu に 3 つ
//! 並んでいたのをまとめたもの。ここは描画にも入力にも依存しないので、
//! 両方から参照しても向きが循環しない。

/// 画面の列区間から値への逆引き。`x0` は含み、`x1` は含まない。
///
/// 行には関知しない — ポインタが実際にその行にあるかは、引く側が先に確かめる。
pub struct ColumnSpans<T> {
    spans: Vec<(u16, u16, T)>,
}

impl<T> Default for ColumnSpans<T> {
    fn default() -> Self {
        Self { spans: Vec::new() }
    }
}

impl<T> ColumnSpans<T> {
    /// 記録を捨てる。描画は毎フレームここから始める。
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    /// 列区間 `x0..x1` に value を割り当てる。
    pub fn push(&mut self, x0: u16, x1: u16, value: T) {
        self.spans.push((x0, x1, value));
    }

    /// 積んである区間を順に。描いた位置そのものを知りたいときに使う。
    pub fn spans(&self) -> impl Iterator<Item = (u16, u16, &T)> {
        self.spans.iter().map(|(x0, x1, v)| (*x0, *x1, v))
    }

    /// まだ何も描かれていない (このフレームで記録が無い)。
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

impl<T: Copy> ColumnSpans<T> {
    /// col にある値。区間が重なっていれば先に積んだ方が勝つ。
    pub fn at(&self, col: u16) -> Option<T> {
        self.spans
            .iter()
            .find(|(x0, x1, _)| col >= *x0 && col < *x1)
            .map(|(_, _, v)| *v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 区間の左端は含み右端は含まない() {
        let mut s = ColumnSpans::default();
        s.push(3, 7, 'a');
        assert_eq!(s.at(2), None);
        assert_eq!(s.at(3), Some('a'));
        assert_eq!(s.at(6), Some('a'));
        assert_eq!(s.at(7), None);
    }

    #[test]
    fn 重なった区間は先に積んだ方が勝つ() {
        let mut s = ColumnSpans::default();
        s.push(0, 10, 'a');
        s.push(5, 15, 'b');
        assert_eq!(s.at(7), Some('a'));
    }

    #[test]
    fn clear_すると何も引けなくなる() {
        let mut s = ColumnSpans::default();
        s.push(0, 4, 'a');
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.at(1), None);
    }
}
