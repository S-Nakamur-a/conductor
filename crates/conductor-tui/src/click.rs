//! ダブルクリックの判定。

use std::time::{Duration, Instant};

pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug)]
pub struct ClickTracker<K = usize> {
    last: Option<(K, Instant)>,
}

impl<K> Default for ClickTracker<K> {
    fn default() -> Self {
        Self { last: None }
    }
}

impl<K: PartialEq> ClickTracker<K> {
    /// この位置へのクリックを記録し、2 回目かどうかを返す。
    ///
    /// 同じ位置であることも条件にしている。時間だけで判定すると、素早く別の行を
    /// 続けて押したときに 2 回目と見なされる。
    pub fn is_double(&mut self, key: K) -> bool {
        let double = self
            .last
            .as_ref()
            .is_some_and(|(i, at)| *i == key && at.elapsed() < DOUBLE_CLICK);
        self.arm(key);
        double
    }

    pub fn arm(&mut self, key: K) {
        self.last = Some((key, Instant::now()));
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
