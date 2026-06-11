//! Party mode — a hidden, flashy easter-egg overlay.
//!
//! When [`crate::app::App::party_mode`] is on, this module post-processes the
//! rendered frame buffer to add three effects, all animated by `ui_tick`:
//!
//! 1. **Rainbow focused border** — every border glyph drawn in the theme's
//!    focused-border colour is recoloured with a flowing rainbow, so the panel
//!    that currently has focus glows and swirls.
//! 2. **Rainbow title bar** — the top title bar's text shimmers.
//! 3. **Confetti** — sparkles drift down across the main content area.
//!
//! The syntax-token rainbow (effect for the Viewer) lives in `viewer_panel.rs`
//! and reuses [`rainbow`] from here.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::app::App;

/// Convert HSL (h: 0-360, s: 0-1, l: 0-1) to an RGB [`Color`].
///
/// Local copy (the equivalent in `common.rs` is private); shared with the
/// rich-mode effects in `rich.rs`. Pass `h` already normalized to 0-360.
pub(crate) fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h2 = h / 60.0;
    let x = c * (1.0 - (h2 % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h2 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::Rgb(
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// A vivid rainbow colour for the given phase (interpreted as degrees of hue).
///
/// Phases that differ by a multiple of 360 produce the same colour, so callers
/// can freely mix position and time terms to get a flowing gradient.
pub fn rainbow(phase: f64) -> Color {
    hsl_to_rgb(phase.rem_euclid(360.0), 1.0, 0.6)
}

/// Whether `s` begins with a box-drawing glyph (U+2500..=U+257F) — i.e. a panel
/// border character. Used to target borders without touching text content.
/// Shared with the rich-mode effects in `rich.rs`.
pub(crate) fn is_border_glyph(s: &str) -> bool {
    matches!(s.chars().next(), Some(c) if ('\u{2500}'..='\u{257F}').contains(&c))
}

/// Small deterministic hash for confetti placement — no `rand` dependency, and
/// stable across frames (the only time term is added by the caller).
fn pseudo_random(seed: u64) -> u64 {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    x ^= x >> 16;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    x
}

/// Single-cell sparkle glyphs (each exactly one terminal column wide).
const SPARKLES: &[&str] = &["✦", "✧", "·", "*", "+", "✩"];

/// Apply all party-mode effects to the just-rendered frame buffer.
///
/// Called at the very end of `render_ui` when `app.party_mode` is set, so it
/// recolours whatever is currently on screen (including any open overlay whose
/// border uses the focused-border colour).
pub fn apply_party_effects(frame: &mut Frame, app: &App) {
    let tick = app.ui_tick as f64;
    let focused = app.theme.border_focused;
    let area = frame.area();
    let buf = frame.buffer_mut();

    // ── Effect 1: rainbow focused border ──────────────────────────────
    // Only the focused panel paints its border in `border_focused`, so matching
    // that colour automatically scopes the rainbow to the active panel.
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y))
                && cell.fg == focused
                && is_border_glyph(cell.symbol())
            {
                cell.fg = rainbow(x as f64 * 6.0 + y as f64 * 12.0 - tick * 6.0);
                cell.modifier.insert(Modifier::BOLD);
            }
        }
    }

    // ── Effect 2: rainbow title bar ───────────────────────────────────
    let title = app.layout_cache.title_area;
    if title.height >= 1 {
        let y = title.y;
        for x in title.x..title.x.saturating_add(title.width) {
            if let Some(cell) = buf.cell_mut((x, y))
                && cell.symbol() != " "
                && !cell.symbol().is_empty()
            {
                cell.fg = rainbow(x as f64 * 8.0 - tick * 5.0);
                cell.modifier.insert(Modifier::BOLD);
            }
        }
    }

    // ── Effect 3: drifting confetti ───────────────────────────────────
    draw_confetti(buf, app.layout_cache.main_area, tick);
}

/// Scatter sparkles across `area`, drifting downward as `tick` advances.
fn draw_confetti(buf: &mut ratatui::buffer::Buffer, area: Rect, tick: f64) {
    if area.width < 4 || area.height < 3 {
        return;
    }
    let count = (area.width / 12).clamp(4, 24) as u64;
    let h = area.height as u64;
    let w = area.width as u64;

    for i in 0..count {
        let rx = pseudo_random(i.wrapping_mul(2654435761));
        let x = area.x + (rx % w) as u16;
        // Each sparkle falls at a slightly different speed and wraps vertically.
        let speed = 2 + (rx % 3); // ticks-per-row divisor variety
        let drift = (tick as u64 / speed).wrapping_add(rx >> 8);
        let y = area.y + (drift % h) as u16;
        let glyph = SPARKLES[(rx as usize >> 4) % SPARKLES.len()];

        if let Some(cell) = buf.cell_mut((x, y)) {
            // Leave graphics-protocol cells alone: rich mode's pixel image
            // preview marks its area with Unicode placeholder characters
            // (plane-16 private use); overwriting one would punch a hole in
            // the image until the next full repaint.
            if cell
                .symbol()
                .chars()
                .next()
                .is_some_and(|c| c >= '\u{100000}')
            {
                continue;
            }
            cell.set_symbol(glyph);
            cell.fg = rainbow(tick * 9.0 + i as f64 * 47.0);
            cell.modifier.insert(Modifier::BOLD);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rainbow_returns_rgb() {
        for phase in [0.0, 90.0, 180.0, 359.0, 720.0, -30.0] {
            assert!(matches!(rainbow(phase), Color::Rgb(_, _, _)));
        }
    }

    #[test]
    fn rainbow_is_periodic_in_360() {
        assert_eq!(rainbow(10.0), rainbow(370.0));
    }

    #[test]
    fn border_glyph_detection() {
        // Box-drawing characters (plain + thick) are borders.
        assert!(is_border_glyph("│"));
        assert!(is_border_glyph("─"));
        assert!(is_border_glyph("┏"));
        assert!(is_border_glyph("┃"));
        // Ordinary text and blanks are not.
        assert!(!is_border_glyph("a"));
        assert!(!is_border_glyph(" "));
        assert!(!is_border_glyph(""));
        assert!(!is_border_glyph("✦"));
    }
}
