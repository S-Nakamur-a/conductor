//! ダブルクリックの判定。

use std::time::{Duration, Instant};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Default)]
pub struct ClickTracker {
    last: Option<(usize, Instant)>,
}

impl ClickTracker {
    /// この位置へのクリックを記録し、2 回目かどうかを返す。
    ///
    /// 同じ位置であることも条件にしている。時間だけで判定すると、素早く別の行を
    /// 続けて押したときに 2 回目と見なされる。
    pub fn is_double(&mut self, index: usize) -> bool {
        let double = self
            .last
            .is_some_and(|(i, at)| i == index && at.elapsed() < DOUBLE_CLICK);
        self.last = Some((index, Instant::now()));
        double
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 最初のクリックはダブルにならない() {
        assert!(!ClickTracker::default().is_double(3));
    }

    #[test]
    fn 同じ場所を続けて2回押すとダブルになる() {
        let mut c = ClickTracker::default();
        c.is_double(3);
        assert!(c.is_double(3));
    }

    #[test]
    fn 別の場所を押すと数え直しになる() {
        let mut c = ClickTracker::default();
        c.is_double(3);
        assert!(!c.is_double(4), "行が違えば速くても 2 回目ではない");
        assert!(c.is_double(4), "そのあと同じ行なら 2 回目");
    }

    #[test]
    fn 遅い2回目はダブルにならない() {
        let mut c = ClickTracker {
            last: Some((3, Instant::now() - DOUBLE_CLICK)),
        };
        assert!(!c.is_double(3));
    }
}
