//! 周期の仕事。tick レートとは別で、フレームの速さに数を合わせない。

use std::time::{Duration, Instant};

/// worktree 一覧の取り直し。
pub const WORKTREE_POLL: Duration = Duration::from_secs(3);
/// 終わった子プロセスのセッションの片付け。
pub const PTY_CLEANUP: Duration = Duration::from_secs(10);

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
}
