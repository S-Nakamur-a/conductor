//! Small time-based transition helpers for smooth UI color animation.
//!
//! Conductor renders in immediate mode and, when idle, throttles its redraw to
//! ~2fps. A "transition" is therefore two things working together: an eased
//! value derived from elapsed wall-clock time (here), and a redraw pump in the
//! main loop that keeps frames flowing while the transition is in flight (see
//! `App::has_active_transition` / `main.rs`). Colors are interpolated with
//! `Theme::lerp`, which the high-FPS, truecolor terminals this targets render as
//! a genuinely smooth gradient.

use std::time::Duration;

/// Duration of focus / panel-border transitions, in milliseconds.
pub const FOCUS_MS: u64 = 180;

/// Smoothstep-eased progress in `[0.0, 1.0]` for a transition of `duration_ms`
/// that began `elapsed` ago. Zero slope at both ends gives a gentle ease-in /
/// ease-out feel rather than a linear ramp.
pub fn eased_progress(elapsed: Duration, duration_ms: u64) -> f64 {
    if duration_ms == 0 {
        return 1.0;
    }
    let p = (elapsed.as_millis() as f64 / duration_ms as f64).clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_zero_at_start_and_one_at_end() {
        assert_eq!(eased_progress(Duration::ZERO, 180), 0.0);
        assert_eq!(eased_progress(Duration::from_millis(180), 180), 1.0);
        // Past the end clamps to fully complete.
        assert_eq!(eased_progress(Duration::from_millis(500), 180), 1.0);
    }

    #[test]
    fn progress_is_monotonic_and_bounded_midway() {
        let a = eased_progress(Duration::from_millis(45), 180);
        let b = eased_progress(Duration::from_millis(90), 180);
        let c = eased_progress(Duration::from_millis(135), 180);
        assert!(a > 0.0 && a < b && b < c && c < 1.0);
        // Smoothstep is symmetric about the midpoint.
        assert!((b - 0.5).abs() < 1e-9);
    }

    #[test]
    fn zero_duration_is_instantly_complete() {
        assert_eq!(eased_progress(Duration::ZERO, 0), 1.0);
    }
}
