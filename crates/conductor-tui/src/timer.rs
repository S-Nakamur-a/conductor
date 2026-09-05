//! 周期の仕事と、静穏を待つ締切。どちらも tick レートとは別で、フレームの速さに数を
//! 合わせない。

use std::time::{Duration, Instant};

/// worktree 一覧の取り直し。
pub const WORKTREE_POLL: Duration = Duration::from_secs(3);
/// 終わった子プロセスのセッションの片付け。
pub const PTY_CLEANUP: Duration = Duration::from_secs(10);
/// ファイルウォッチャーの通知をまとめる猶予。エディタ保存や git checkout は
/// 短時間に何件も飛んでくるので、都度 Explorer を投げ直すと無駄が積み重なる。
pub const FS_DEBOUNCE: Duration = Duration::from_millis(500);
/// 設定ファイルの変更をまとめる猶予。
pub const CONFIG_DEBOUNCE: Duration = Duration::from_millis(300);

pub struct Timer {
    every: Duration,
    last: Instant,
}

impl Timer {
    pub fn new(every: Duration, now: Instant) -> Self {
        Self { every, last: now }
    }

    /// 期限が来ていれば true にして次の期限へ進める。
    pub fn due(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last) < self.every {
            return false;
        }
        self.last = now;
        true
    }
}

/// 触られるたび期限を先送りし、静かになったら一度だけ発火する締切。
pub struct Debounce {
    delay: Duration,
    due: Option<Instant>,
}

impl Debounce {
    pub fn new(delay: Duration) -> Self {
        Self { delay, due: None }
    }

    pub fn touch(&mut self, now: Instant) {
        self.due = Some(now + self.delay);
    }

    /// 期限が来ていれば true にして待ちを解く。
    pub fn fire(&mut self, now: Instant) -> bool {
        if self.due.is_none_or(|due| now < due) {
            return false;
        }
        self.due = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 期限が来るまでは発火しない() {
        let start = Instant::now();
        let mut timer = Timer::new(Duration::from_secs(3), start);
        assert!(!timer.due(start));
        assert!(!timer.due(start + Duration::from_millis(2999)));
        assert!(timer.due(start + Duration::from_secs(3)));
        assert!(!timer.due(start + Duration::from_secs(4)));
        assert!(timer.due(start + Duration::from_secs(6)));
    }

    #[test]
    fn 静穏が続いた一度だけ発火する() {
        let start = Instant::now();
        let mut debounce = Debounce::new(Duration::from_millis(500));
        assert!(!debounce.fire(start), "触られていないのに発火した");

        debounce.touch(start);
        assert!(!debounce.fire(start + Duration::from_millis(499)));
        assert!(debounce.fire(start + Duration::from_millis(500)));
        assert!(
            !debounce.fire(start + Duration::from_secs(10)),
            "二度発火した"
        );
    }

    #[test]
    fn 触り直すと期限が先送りされる() {
        let start = Instant::now();
        let mut debounce = Debounce::new(Duration::from_millis(500));
        debounce.touch(start);
        debounce.touch(start + Duration::from_millis(400));
        assert!(!debounce.fire(start + Duration::from_millis(500)));
        assert!(debounce.fire(start + Duration::from_millis(900)));
    }
}
