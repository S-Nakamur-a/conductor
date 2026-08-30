//! ダブルクリックの判定。
//!
//! 分割前は判定用の時刻と添字が `viewer.click` に置かれ、Explorer がそこへ
//! 書いていた。どのパネルの持ち物でもない入力の機構なので、語彙の側に置く。

use std::time::{Duration, Instant};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Default)]
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
    fn the_first_click_is_never_double() {
        assert!(!ClickTracker::default().is_double(3));
    }

    #[test]
    fn the_same_place_twice_in_a_row_is_double() {
        let mut c = ClickTracker::default();
        c.is_double(3);
        assert!(c.is_double(3));
    }

    #[test]
    fn a_different_place_resets_it() {
        let mut c = ClickTracker::default();
        c.is_double(3);
        assert!(!c.is_double(4), "行が違えば速くても 2 回目ではない");
        assert!(c.is_double(4), "そのあと同じ行なら 2 回目");
    }

    #[test]
    fn a_slow_second_click_is_not_double() {
        let mut c = ClickTracker {
            last: Some((3, Instant::now() - DOUBLE_CLICK)),
        };
        assert!(!c.is_double(3));
    }
}
