//! 名前付き周期タイマーの簡単なレジストリ。

use std::time::{Duration, Instant};

/// 名前付き周期タイマーの集合。
pub struct TimerRegistry {
    timers: Vec<Timer>,
}

struct Timer {
    name: &'static str,
    interval: Duration,
    last_fired: Instant,
}

impl TimerRegistry {
    pub fn new() -> Self {
        Self { timers: Vec::new() }
    }

    /// interval ごとに発火するタイマーを登録する。
    /// 最初の発火は今から interval 後 (fire_immediately を指定した場合を除く)。
    pub fn register(&mut self, name: &'static str, interval: Duration) {
        self.timers.push(Timer {
            name,
            interval,
            last_fired: Instant::now(),
        });
    }

    /// 最初のチェックで即座に発火するタイマーを登録する。
    pub fn register_immediate(&mut self, name: &'static str, interval: Duration) {
        self.timers.push(Timer {
            name,
            interval,
            last_fired: Instant::now() - interval,
        });
    }

    /// 全タイマーを調べ、発火時刻に達したものの名前を返す。
    /// 発火したタイマーは自動的にリセットされる。
    pub fn check_due(&mut self) -> Vec<&'static str> {
        let mut due = Vec::new();
        for timer in &mut self.timers {
            if timer.last_fired.elapsed() >= timer.interval {
                timer.last_fired = Instant::now();
                due.push(timer.name);
            }
        }
        due
    }
}
