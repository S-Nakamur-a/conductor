//! 起動と索引の演出の進行状態。
//!
//! 実際の描画は [`crate::ui::common::entrance`]。ここが持つのは開始時刻と、
//! 索引がいま作られているかどうかだけ。

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ui::common::entrance as fx;

/// 起動演出の段階。
#[derive(Debug, Default, PartialEq, Eq)]
enum Boot {
    /// 設定で切られたか、済んだか、飛ばされたか。
    #[default]
    Off,
    /// 出す気はあるが、まだ 1 枚も描いていない。
    Pending,
    Running(Instant),
}

#[derive(Debug, Default)]
pub struct EntranceState {
    boot: Boot,
    /// パネルごとの開始時点 (進捗の割合)。
    offsets: Vec<f64>,
    /// 索引を作り始めたのを見た時刻。
    index_since: Option<Instant>,
    /// 索引完了の演出の開始時刻。
    index_done: Option<Instant>,
}

impl EntranceState {
    pub fn new(enabled: bool) -> Self {
        Self {
            boot: if enabled { Boot::Pending } else { Boot::Off },
            offsets: fx::offsets(&jitter()),
            ..Default::default()
        }
    }

    /// 最初のフレームを描く直前に時計を始める。
    ///
    /// App::new の中で始めると、そこから worktree・diff の走査と端末への
    /// 背景色問い合わせが挟まる。実測でフレームが出るのは 2 秒後で、演出は
    /// 画面に何か出るより前に終わっていた。
    pub fn start_if_pending(&mut self) {
        if self.boot == Boot::Pending {
            self.boot = Boot::Running(Instant::now());
        }
    }

    /// 起動演出の進捗。演出が終わっていれば None。
    pub fn boot_progress(&self) -> Option<f64> {
        match self.boot {
            Boot::Off => None,
            // まだ時計が動いていない = これから描く 1 枚目。
            Boot::Pending => Some(0.0),
            Boot::Running(at) => {
                let p = ratio(at.elapsed(), fx::ENTRANCE_MS);
                (p < 1.0).then_some(p)
            }
        }
    }

    pub fn offsets(&self) -> &[f64] {
        &self.offsets
    }

    /// 入力があったら完成状態へ飛ばす。押した人を待たせる理由はない。
    pub fn skip(&mut self) {
        self.boot = Boot::Off;
    }

    /// 索引が作られているかを毎フレーム伝える。開始と完了の縁でだけ状態が動く。
    pub fn note_index_building(&mut self, building: bool) {
        match (self.index_since.is_some(), building) {
            (false, true) => self.index_since = Some(Instant::now()),
            (true, false) => {
                self.index_since = None;
                self.index_done = Some(Instant::now());
            }
            _ => {}
        }
    }

    /// 索引の実行中を示す不定バーの位相。進捗ではないので割合ではなく循環値。
    ///
    /// 開始の合図は別に置いていない。上辺に光が流れ出すこと自体が合図になるうえ、
    /// 索引は起動直後にも走るので、起動演出の真後ろにもう一度演出が挟まる。
    pub fn index_bar_phase(&self) -> Option<f64> {
        let since = self.index_since?;
        Some(since.elapsed().as_millis() as f64 / BAR_CYCLE_MS % 1.0)
    }

    pub fn index_done_progress(&self) -> Option<f64> {
        let at = self.index_done?;
        let p = ratio(at.elapsed(), fx::INDEX_DONE_MS);
        (p < 1.0).then_some(p)
    }

    /// 時間で動いているものがあるか。メインループはこれを見てフレームを流し続ける。
    pub fn is_animating(&self) -> bool {
        self.boot_progress().is_some()
            || self.index_since.is_some()
            || self.index_done_progress().is_some()
    }
}

/// 不定バーが 1 周する時間。
const BAR_CYCLE_MS: f64 = 900.0;

fn ratio(elapsed: Duration, total_ms: u64) -> f64 {
    if total_ms == 0 {
        return 1.0;
    }
    (elapsed.as_millis() as f64 / total_ms as f64).clamp(0.0, 1.0)
}

/// パネルごとの開始のゆらぎ。順序は [`fx::offsets`] が保つので、ここは幅だけ決める。
fn jitter() -> Vec<f64> {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let span = fx::JITTER_MS as f64 / fx::ENTRANCE_MS as f64;
    (0..4)
        .map(|i| {
            // 起動ごとに違えばよいだけなので、質のよい乱数は要らない。
            let bits = seed.rotate_left(i * 8) % 1000;
            (f64::from(bits) / 1000.0 * 2.0 - 1.0) * span
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 設定で切られていれば起動演出は始まらない() {
        let s = EntranceState::new(false);
        assert_eq!(s.boot_progress(), None);
        assert!(!s.is_animating());
    }

    /// 時計は最初のフレームまで動かない。App::new から数えると、worktree の走査と
    /// 端末への問い合わせのあいだに演出が終わってしまう (実測で 2 秒)。
    #[test]
    fn 時計は最初のフレームまで動かない() {
        let mut s = EntranceState::new(true);
        assert_eq!(s.boot_progress(), Some(0.0));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(s.boot_progress(), Some(0.0), "描く前に進んでいる");

        s.start_if_pending();
        std::thread::sleep(Duration::from_millis(30));
        assert!(s.boot_progress().unwrap() > 0.0, "描き始めても進まない");
    }

    #[test]
    fn 時計は一度始めたら打ち直さない() {
        let mut s = EntranceState::new(true);
        s.start_if_pending();
        std::thread::sleep(Duration::from_millis(30));
        let p = s.boot_progress().unwrap();
        s.start_if_pending();
        assert!(s.boot_progress().unwrap() >= p, "2 枚目で先頭へ戻った");
    }

    #[test]
    fn 入力で完成状態へ飛ぶ() {
        let mut s = EntranceState::new(true);
        assert!(s.boot_progress().is_some());
        s.skip();
        assert_eq!(s.boot_progress(), None);
    }

    /// 開始と完了は縁でだけ動く。毎フレーム true を渡しても開始時刻が
    /// 巻き戻ると、バーがいつまでも先頭から出直してしまう。
    #[test]
    fn 索引の開始は縁でだけ動く() {
        let mut s = EntranceState::new(false);
        s.note_index_building(true);
        let first = s.index_since;
        s.note_index_building(true);
        assert_eq!(s.index_since, first);

        s.note_index_building(false);
        assert!(s.index_since.is_none());
        assert!(s.index_done_progress().is_some());
    }

    #[test]
    fn ゆらぎは指定した幅に収まる() {
        let span = fx::JITTER_MS as f64 / fx::ENTRANCE_MS as f64;
        for j in jitter() {
            assert!(j.abs() <= span, "{j} が ±{span} を超えた");
        }
    }
}
