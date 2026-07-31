//! Pure scroll arithmetic for the reflow transcript view.
//!
//! Extracted as a module so these functions can be unit-tested independently
//! of the full `App` state and confirmed against the core invariant:
//! **1 logical line == 1 visual row** (no `Paragraph::wrap`) therefore
//! `max_scroll == total_lines.saturating_sub(inner_height)`.

/// Clamp `scroll` to the valid range `[0, total.saturating_sub(inner)]`.
///
/// `inner` is the height of the panel's visible area in terminal rows.
/// When the total number of lines is less than or equal to `inner` (the whole
/// log fits in the panel), the function returns `0`, keeping the view pinned
/// to the top with no blank rows below the last line.
pub fn clamp_scroll(scroll: usize, total: usize, inner: usize) -> usize {
    scroll.min(total.saturating_sub(inner))
}

/// Return `true` when `scroll` is at or past the logical bottom of the content.
///
/// `inner` is the visible height.  A scroll position at `total - inner` means
/// the last content line is on the last visual row — this is "at bottom".
pub fn at_bottom(scroll: usize, total: usize, inner: usize) -> bool {
    scroll >= total.saturating_sub(inner)
}

/// Scroll offset for the newest line — the position [`at_bottom`] reports as
/// the bottom, and the one a follower is pinned to.
pub fn bottom_scroll(total: usize, inner: usize) -> usize {
    total.saturating_sub(inner)
}

/// Where the viewport should sit once the geometry underneath it has changed.
///
/// Called on every frame with `anchored` set only when the line list was just
/// rebuilt (a width change or the expand toggle). The two branches encode the
/// whole "preserve the reading position" rule:
///
/// * `following` — the reader is riding the newest turn, so re-pin to the
///   bottom. This is what keeps a resize from stranding the newest output
///   below the viewport: a narrower panel wraps the same text into *more*
///   lines, so the old offset (and even a correctly re-anchored one) no
///   longer reaches the end.
/// * otherwise — the reader is parked somewhere in the history, so honour
///   `anchored`: the index of the same logical line (entry / block / offset)
///   after the rebuild. Falling back to `previous` covers the frames where
///   nothing was rebuilt and the offset is still meaningful.
///
/// Never a raw line offset carried across a rebuild: line *indices* mean
/// something different at every width, which is exactly how a reflow used to
/// throw the reader somewhere unrelated.
pub fn scroll_after_reflow(
    following: bool,
    anchored: Option<usize>,
    previous: usize,
    total: usize,
    inner: usize,
) -> usize {
    let target = if following {
        bottom_scroll(total, inner)
    } else {
        anchored.unwrap_or(previous)
    };
    clamp_scroll(target, total, inner)
}

// ── Transition animation ──────────────────────────────────────────────────────

/// Total duration of the entry/exit transition animation in milliseconds.
///
/// 500 ms lets the border hue glide gently from the accent to its complement
/// (and back on exit) as a single smooth gradient — calm rather than the old
/// rapid flicker — while still being short enough that leaving read mode feels
/// responsive.
pub const TRANSITION_DURATION_MS: u64 = 500;

/// Compute how far a transition animation has progressed, clamped to `[0.0, 1.0]`.
///
/// `start` is the `Instant` the animation began.  Returns `0.0` at the moment
/// of creation and `1.0` at or after `duration_ms` milliseconds have elapsed.
/// A `duration_ms` of zero is treated as instantly complete (returns `1.0`) to
/// avoid a division-by-zero.
pub fn sweep_progress(start: &std::time::Instant, duration_ms: u64) -> f64 {
    if duration_ms == 0 {
        return 1.0;
    }
    let elapsed_ms = start.elapsed().as_millis() as f64;
    (elapsed_ms / duration_ms as f64).clamp(0.0, 1.0)
}

/// Smoothstep easing for the entry/exit border-color transition.
///
/// Maps linear `progress` in `[0, 1]` to an eased `[0, 1]` value via the
/// classic `3p² − 2p³` curve, which has zero slope at both ends. Callers use
/// the result to interpolate the border between the accent color and its
/// complement, so the hue change starts and settles gently instead of
/// flickering — `0.0` keeps the start color, `1.0` reaches the target.
pub fn transition_eased(progress: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── clamp_scroll ─────────────────────────────────────────────────────────

    #[test]
    fn clamp_allows_zero() {
        assert_eq!(clamp_scroll(0, 100, 20), 0);
    }

    #[test]
    fn clamp_pins_to_max() {
        // max = 100 - 20 = 80
        assert_eq!(clamp_scroll(200, 100, 20), 80);
    }

    #[test]
    fn clamp_at_exact_max() {
        assert_eq!(clamp_scroll(80, 100, 20), 80);
    }

    #[test]
    fn clamp_within_range_unchanged() {
        assert_eq!(clamp_scroll(40, 100, 20), 40);
    }

    #[test]
    fn clamp_when_log_shorter_than_panel_returns_zero() {
        // total(10) < inner(20): whole log fits → max_scroll = 0
        assert_eq!(clamp_scroll(5, 10, 20), 0);
        assert_eq!(clamp_scroll(0, 10, 20), 0);
    }

    #[test]
    fn clamp_when_total_equals_inner_returns_zero() {
        assert_eq!(clamp_scroll(0, 20, 20), 0);
        assert_eq!(clamp_scroll(1, 20, 20), 0);
    }

    #[test]
    fn clamp_total_zero_returns_zero() {
        assert_eq!(clamp_scroll(0, 0, 20), 0);
    }

    // ── at_bottom ────────────────────────────────────────────────────────────

    #[test]
    fn at_bottom_when_scroll_equals_max() {
        assert!(at_bottom(80, 100, 20));
    }

    #[test]
    fn at_bottom_when_scroll_exceeds_max() {
        assert!(at_bottom(90, 100, 20));
    }

    #[test]
    fn not_at_bottom_when_scroll_below_max() {
        assert!(!at_bottom(79, 100, 20));
    }

    #[test]
    fn at_bottom_when_log_fits_in_panel() {
        // total(10) <= inner(20): max_scroll = 0, any scroll >= 0 is at bottom
        assert!(at_bottom(0, 10, 20));
    }

    // ── scroll_after_reflow ──────────────────────────────────────────────────

    #[test]
    fn follower_repins_to_bottom_when_a_narrower_width_adds_lines() {
        // The regression this whole flag exists for: the reader was riding the
        // newest turn at 100 lines / 20 rows (scroll 80). The panel narrows, the
        // same text now wraps into 140 lines, and the anchor would put the old
        // top line back on top — leaving the newest 40 lines below the viewport.
        let anchored = Some(80); // whatever the anchor resolved to
        assert_eq!(scroll_after_reflow(true, anchored, 80, 140, 20), 120);
    }

    #[test]
    fn follower_repins_to_bottom_when_the_panel_gets_shorter() {
        // Height-only change: no rebuild, so `anchored` is None. Clamping alone
        // would leave scroll at 80 with the last 10 lines cut off below.
        assert_eq!(scroll_after_reflow(true, None, 80, 100, 10), 90);
    }

    #[test]
    fn detached_reader_lands_on_the_anchored_line() {
        // Parked mid-history: the anchor wins over both the old raw offset and
        // the bottom.
        assert_eq!(scroll_after_reflow(false, Some(57), 40, 200, 20), 57);
    }

    #[test]
    fn detached_reader_keeps_its_offset_when_nothing_was_rebuilt() {
        assert_eq!(scroll_after_reflow(false, None, 40, 200, 20), 40);
    }

    #[test]
    fn detached_reader_never_gets_dragged_to_the_bottom() {
        // The property that matters more than any single number: for a reader
        // who is *not* following, no combination of anchor / geometry may
        // resolve to the bottom unless the anchor genuinely points there.
        let total = 300;
        let inner = 25;
        let bottom = bottom_scroll(total, inner);
        for anchored in [Some(0), Some(11), Some(120), None] {
            let got = scroll_after_reflow(false, anchored, 33, total, inner);
            assert!(
                got < bottom,
                "anchored={anchored:?} resolved to {got}, i.e. the live tail"
            );
        }
    }

    #[test]
    fn anchored_index_past_the_end_is_clamped_not_wrapped() {
        // A block that shrank can resolve past the last valid offset; that must
        // clamp to the bottom rather than index out of range downstream.
        assert_eq!(scroll_after_reflow(false, Some(9_999), 40, 200, 20), 180);
    }

    #[test]
    fn short_log_collapses_to_top_for_follower_and_reader_alike() {
        assert_eq!(scroll_after_reflow(true, None, 0, 10, 40), 0);
        assert_eq!(scroll_after_reflow(false, Some(5), 3, 10, 40), 0);
    }

    #[test]
    fn following_result_always_reports_at_bottom() {
        for (total, inner) in [(100usize, 20usize), (10, 40), (0, 5), (41, 7)] {
            let s = scroll_after_reflow(true, None, 0, total, inner);
            assert!(at_bottom(s, total, inner), "total={total} inner={inner}");
        }
    }

    // ── sweep_progress ───────────────────────────────────────────────────────

    #[test]
    fn sweep_progress_zero_duration_returns_complete() {
        let t = std::time::Instant::now();
        assert_eq!(sweep_progress(&t, 0), 1.0);
    }

    #[test]
    fn sweep_progress_fresh_instant_is_near_zero() {
        let t = std::time::Instant::now();
        let p = sweep_progress(&t, TRANSITION_DURATION_MS);
        // A just-started animation must be well under 10 % complete.
        assert!(p < 0.1, "expected near 0.0, got {p}");
    }

    // ── transition_eased ─────────────────────────────────────────────────────

    #[test]
    fn transition_eased_endpoints_are_exact() {
        // Smoothstep pins to 0 at the start and 1 at the end so the border
        // begins at the accent and settles exactly on the complement.
        assert_eq!(transition_eased(0.0), 0.0);
        assert_eq!(transition_eased(1.0), 1.0);
    }

    #[test]
    fn transition_eased_midpoint_is_half() {
        // 3(0.5)² − 2(0.5)³ = 0.5 — symmetric curve passes through its center.
        assert!((transition_eased(0.5) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn transition_eased_is_monotonic_within_unit_range() {
        // A single smooth ramp: never decreasing, always within [0, 1].
        let mut prev = 0.0;
        for i in 0..=100 {
            let p = i as f64 / 100.0;
            let v = transition_eased(p);
            assert!((0.0..=1.0).contains(&v), "transition_eased({p}) = {v} out of range");
            assert!(v >= prev - 1e-12, "transition_eased must be monotonic non-decreasing");
            prev = v;
        }
    }

    #[test]
    fn transition_eased_clamps_out_of_range_input() {
        assert_eq!(transition_eased(-0.5), 0.0);
        assert_eq!(transition_eased(1.5), 1.0);
    }

    // ── Integration: pending_bottom pin ─────────────────────────────────────

    #[test]
    fn pending_bottom_pin_matches_clamp_max() {
        let total = 150usize;
        let inner = 30usize;
        let pinned = total.saturating_sub(inner); // 120
        assert_eq!(clamp_scroll(pinned, total, inner), pinned);
        assert!(at_bottom(pinned, total, inner));
    }
}
