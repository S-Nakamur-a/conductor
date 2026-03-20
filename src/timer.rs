//! Simple named-interval timer registry.
//!
//! Replaces scattered `last_X` + `INTERVAL` variables in main.rs with a
//! single data structure that tracks all periodic tasks.

use std::time::{Duration, Instant};

/// A collection of named periodic timers.
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

    /// Register a timer that fires every `interval`.
    /// The first fire happens after `interval` from now (unless `fire_immediately` is set).
    pub fn register(&mut self, name: &'static str, interval: Duration) {
        self.timers.push(Timer {
            name,
            interval,
            last_fired: Instant::now(),
        });
    }

    /// Register a timer that fires immediately on the first check.
    pub fn register_immediate(&mut self, name: &'static str, interval: Duration) {
        self.timers.push(Timer {
            name,
            interval,
            last_fired: Instant::now() - interval,
        });
    }

    /// Check all timers and return names of those that are due.
    /// Resets the fired timers automatically.
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
