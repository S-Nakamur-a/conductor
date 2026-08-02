//! UI の色を滑らかに動かすための、時間ベースの小さな遷移ヘルパー。
//!
//! Conductor は即時モードで描画し、待機中は再描画を 2fps 程度まで落とす。
//! つまり「遷移」は 2 つの仕組みの合わせ技になる: 経過実時間から求めた
//! イージング済みの値 (このモジュール) と、遷移が続いている間だけフレームを
//! 流し続けるメインループ側の再描画ポンプ (App::has_active_transition /
//! main.rs を参照)。色の補間は Theme::lerp で行い、対象としている
//! 高 FPS・truecolor の端末ではこれが実際に滑らかなグラデーションになる。

use std::time::Duration;

/// フォーカスとパネル枠の遷移にかける時間 (ミリ秒)。
pub const FOCUS_MS: u64 = 180;

/// elapsed 前に始まった duration_ms の遷移について、smoothstep で
/// イージングした [0.0, 1.0] の進捗を返す。両端で傾きが 0 になるので、
/// 線形のランプではなく緩やかな ease-in / ease-out の感触になる。
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
        // 終端を過ぎたぶんは完了側にクランプされる。
        assert_eq!(eased_progress(Duration::from_millis(500), 180), 1.0);
    }

    #[test]
    fn progress_is_monotonic_and_bounded_midway() {
        let a = eased_progress(Duration::from_millis(45), 180);
        let b = eased_progress(Duration::from_millis(90), 180);
        let c = eased_progress(Duration::from_millis(135), 180);
        assert!(a > 0.0 && a < b && b < c && c < 1.0);
        // smoothstep は中点について対称。
        assert!((b - 0.5).abs() < 1e-9);
    }

    #[test]
    fn zero_duration_is_instantly_complete() {
        assert_eq!(eased_progress(Duration::ZERO, 0), 1.0);
    }
}
