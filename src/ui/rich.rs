//! Rich mode Tier A effects — gradient breathing borders.
//!
//! Post-processes the rendered frame buffer (same pattern as `party.rs`)
//! when [`crate::term_caps::RichTier`] is Tier A or higher:
//!
//! 1. **Focused-panel border gradient** — the focused border's glyphs are
//!    recoloured with a theme-derived gradient (a hue sweep around
//!    `border_focused`) that slowly flows along the border and breathes in
//!    brightness. Slow and low-saturation on purpose: it marks focus without
//!    shouting. Unfocused borders are left untouched.
//! 2. **Claude-waiting glow** — when the selected worktree's Claude session
//!    waits for input, the Claude panel's border breathes in the theme's
//!    waiting colours. Faster and warmer than the focus gradient so the two
//!    states stay distinguishable in peripheral vision; it is applied after
//!    (and therefore wins over) the focus gradient.
//!
//! Both effects derive every colour from the active [`crate::theme::Theme`]
//! at render time — no per-theme gradient data is stored.
//!
//! Animation phases derive from wall-clock time (`App::rich_epoch`), not
//! `ui_tick`, so the perceived speed never changes with the redraw rate.
//! The effects only *advance visually* when something redraws the frame
//! (input, PTY output, or the waiting pulse) — a fully idle screen freezes
//! mid-gradient on purpose, keeping idle CPU at zero instead of forcing a
//! redraw timer.
//!
//! The whole pass is skipped while party mode is active: party detects the
//! focused border by colour equality with `border_focused`, which these
//! effects would break.

use std::f64::consts::TAU;

use ratatui::Frame;
use ratatui::style::{Color, Modifier};

use crate::app::App;

use super::party::{hsl_to_rgb, is_border_glyph};

/// Breathing period of the focus gradient, in seconds. Ursula's perception
/// window is 4–6s: slower stops reading as "breathing", faster becomes
/// distracting for an ambient cue.
const FOCUS_BREATH_PERIOD_SECS: f64 = 6.0;
/// Hue sweep amplitude (degrees either side of `border_focused`'s hue).
const FOCUS_HUE_SWEEP: f64 = 24.0;
/// How fast the gradient wave drifts along the border (degrees per second;
/// with the 9°-per-column wave below this is a gentle ~4.4 columns/second).
const FOCUS_FLOW_DEG_PER_SEC: f64 = 40.0;
/// Breathing period of the waiting glow, in seconds — deliberately faster
/// than the focus breath so "Claude needs you" reads as urgent where "this
/// panel has focus" reads as ambient.
const WAITING_BREATH_PERIOD_SECS: f64 = 1.6;

/// Apply all rich-mode Tier A effects to the just-rendered frame buffer.
///
/// Called at the end of `render_ui` (before the party-mode pass, which takes
/// over completely when active).
pub fn apply_rich_effects(frame: &mut Frame, app: &App) {
    let t = app.rich_epoch.elapsed().as_secs_f64();
    apply_focus_gradient(frame, app, t);
    apply_waiting_glow(frame, app, t);
}

/// Recolour every focused-border glyph with the flowing, breathing gradient.
///
/// Like party mode, glyphs are found by colour equality with
/// `border_focused`: only the focused panel paints its border in that colour,
/// so the match automatically scopes the effect (including overlays that
/// deliberately use the focused colour).
fn apply_focus_gradient(frame: &mut Frame, app: &App, t: f64) {
    let focused = app.theme.border_focused;
    let Some((h, s, l)) = rgb_to_hsl(focused) else {
        return;
    };

    let breath = 0.5 + 0.5 * (t * TAU / FOCUS_BREATH_PERIOD_SECS).sin();
    // Swing lightness around the theme's own value (0.82x..1.18x) so the
    // border visibly "breathes" while staying in the theme's neighbourhood.
    let lightness = (l * (0.82 + 0.36 * breath)).min(0.95);

    let area = frame.area();
    let buf = frame.buffer_mut();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y))
                && cell.fg == focused
                && is_border_glyph(cell.symbol())
            {
                // A long sine wave (≈40 columns) drifting slowly along the
                // border, sweeping the hue ±FOCUS_HUE_SWEEP around the theme.
                let flow = (x as f64 * 9.0 + y as f64 * 18.0 - t * FOCUS_FLOW_DEG_PER_SEC)
                    .to_radians()
                    .sin();
                let hue = (h + flow * FOCUS_HUE_SWEEP).rem_euclid(360.0);
                cell.fg = hsl_to_rgb(hue, s, lightness);
            }
        }
    }
}

/// Make the Claude panel's border breathe in the waiting colours while the
/// selected worktree's session waits for input.
///
/// Targets the panel rectangle from the layout cache (not colour matching)
/// so it works whether the panel is focused or not. Skipped while an overlay
/// is open: the glow would otherwise recolour overlay borders crossing the
/// panel area, and the user is already mid-interaction anyway.
fn apply_waiting_glow(frame: &mut Frame, app: &App, t: f64) {
    if app.terminal.cc_waiting_worktrees.is_empty()
        || !app
            .terminal
            .cc_waiting_worktrees
            .contains(&app.selected_worktree_path())
        || app.is_any_overlay_active()
    {
        return;
    }

    let rect = app.layout_cache.terminal_split[0];
    if rect.width < 2 || rect.height < 2 {
        return;
    }

    let breath = 0.5 + 0.5 * (t * TAU / WAITING_BREATH_PERIOD_SECS).sin();
    let color = lerp_rgb(app.theme.waiting_secondary, app.theme.waiting_primary, breath);

    let buf = frame.buffer_mut();
    let (left, right) = (rect.x, rect.x + rect.width - 1);
    let (top, bottom) = (rect.y, rect.y + rect.height - 1);

    let paint = |x: u16, y: u16, buf: &mut ratatui::buffer::Buffer| {
        if let Some(cell) = buf.cell_mut((x, y))
            && is_border_glyph(cell.symbol())
        {
            cell.fg = color;
            cell.modifier.insert(Modifier::BOLD);
        }
    };

    // Perimeter walk. The Claude panel has no top border line (the session
    // tabs row sits there), so the top edge simply finds no border glyphs.
    for x in left..=right {
        paint(x, top, buf);
        paint(x, bottom, buf);
    }
    for y in top..=bottom {
        paint(left, y, buf);
        paint(right, y, buf);
    }
}

/// Convert an RGB [`Color`] to HSL (h: 0-360, s: 0-1, l: 0-1).
/// Returns `None` for non-RGB colours (indexed/named), which rich effects
/// leave untouched.
fn rgb_to_hsl(color: Color) -> Option<(f64, f64, f64)> {
    let Color::Rgb(r, g, b) = color else {
        return None;
    };
    let r = r as f64 / 255.0;
    let g = g as f64 / 255.0;
    let b = b as f64 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if max == min {
        return Some((0.0, 0.0, l)); // achromatic
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    Some((h * 60.0, s, l))
}

/// Linear interpolation between two RGB colours (`t`: 0 = `a`, 1 = `b`).
/// Falls back to `b` when either colour is not RGB.
fn lerp_rgb(a: Color, b: Color, t: f64) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return b;
    };
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Maximum per-channel error tolerated in an RGB → HSL → RGB round trip.
    const ROUND_TRIP_TOLERANCE: i32 = 2;

    #[test]
    fn rgb_hsl_round_trips_theme_colors() {
        // Every built-in theme's border/waiting colors must survive the
        // round trip, or the gradient would visibly shift the theme.
        for name in [
            "catppuccin-mocha",
            "dracula",
            "nord",
            "solarized-dark",
            "tokyo-night",
            "gruvbox",
            "rose-pine",
            "kanagawa",
        ] {
            let theme = crate::theme::Theme::from_name(name);
            for color in [
                theme.border_focused,
                theme.waiting_primary,
                theme.waiting_secondary,
            ] {
                let (h, s, l) = rgb_to_hsl(color).expect("theme colors are RGB");
                let back = hsl_to_rgb(h.rem_euclid(360.0), s, l);
                let (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) = (color, back) else {
                    panic!("expected RGB");
                };
                for (a, b) in [(r0, r1), (g0, g1), (b0, b1)] {
                    assert!(
                        (a as i32 - b as i32).abs() <= ROUND_TRIP_TOLERANCE,
                        "{name}: {color:?} round-tripped to {back:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn rgb_to_hsl_rejects_non_rgb() {
        assert!(rgb_to_hsl(Color::Indexed(3)).is_none());
        assert!(rgb_to_hsl(Color::Red).is_none());
    }

    #[test]
    fn rgb_to_hsl_achromatic() {
        let (h, s, l) = rgb_to_hsl(Color::Rgb(128, 128, 128)).unwrap();
        assert_eq!(h, 0.0);
        assert_eq!(s, 0.0);
        assert!((l - 0.502).abs() < 0.01);
    }

    #[test]
    fn lerp_rgb_endpoints_and_midpoint() {
        let a = Color::Rgb(0, 100, 200);
        let b = Color::Rgb(200, 0, 100);
        assert_eq!(lerp_rgb(a, b, 0.0), a);
        assert_eq!(lerp_rgb(a, b, 1.0), b);
        assert_eq!(lerp_rgb(a, b, 0.5), Color::Rgb(100, 50, 150));
        // Out-of-range t is clamped.
        assert_eq!(lerp_rgb(a, b, -1.0), a);
        assert_eq!(lerp_rgb(a, b, 2.0), b);
    }

    #[test]
    fn lerp_rgb_falls_back_on_non_rgb() {
        assert_eq!(
            lerp_rgb(Color::Red, Color::Rgb(1, 2, 3), 0.5),
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn focus_gradient_stays_near_theme_hue() {
        // The gradient must never wander far from the theme's hue: sample a
        // full breath+flow cycle and check the hue distance.
        let theme = crate::theme::Theme::from_name("catppuccin-mocha");
        let (h0, _, _) = rgb_to_hsl(theme.border_focused).unwrap();
        for step in 0..200 {
            let t = step as f64 * 0.1; // 20 seconds in 100ms steps
            let flow = (10.0 * 9.0 + 5.0 * 18.0 - t * FOCUS_FLOW_DEG_PER_SEC)
                .to_radians()
                .sin();
            let hue = (h0 + flow * FOCUS_HUE_SWEEP).rem_euclid(360.0);
            let dist = (hue - h0).abs().min(360.0 - (hue - h0).abs());
            assert!(
                dist <= FOCUS_HUE_SWEEP + 0.001,
                "hue drifted {dist}° at t={t}"
            );
        }
    }
}
