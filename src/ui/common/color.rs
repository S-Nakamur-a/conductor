//! Color math shared by the badge/label renderers: HSL generation, WCAG
//! contrast, and per-repository badge color derivation.

use ratatui::style::Color;
use std::hash::{Hash, Hasher};

/// Convert HSL (h: 0-360, s: 0-1, l: 0-1) to RGB.
pub(super) fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
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
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// Relative luminance of an sRGB color per WCAG 2.1 (SC 1.4.3, D65).
///
/// Channels are 0-255. Each is normalized to 0-1, gamma-expanded to linear
/// light, then combined with the standard luminance coefficients.
pub(super) fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    fn linearize(c: u8) -> f64 {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
}

/// Pick black or white text for the best WCAG contrast against the given
/// background. The badge is for identifying the repository at a glance, so we
/// always take the higher-contrast option rather than failing when neither
/// candidate clears the 4.5:1 guideline. Explicit RGB (not named ANSI colors)
/// keeps the rendered result aligned with the luminance computation.
pub(super) fn readable_fg_on(r: u8, g: u8, b: u8) -> Color {
    let bg = relative_luminance(r, g, b);
    // Contrast ratio (L1 + 0.05) / (L2 + 0.05); black has L=0, white has L=1.
    let contrast = |fg: f64| (bg.max(fg) + 0.05) / (bg.min(fg) + 0.05);
    if contrast(0.0) >= contrast(1.0) {
        Color::Rgb(0, 0, 0)
    } else {
        Color::Rgb(255, 255, 255)
    }
}

/// Generate badge background, badge text, and branch text colors from a
/// repository name.
///
/// Uses a hash of the name to pick a hue, then produces three colors:
/// - Badge background: muted (S=0.6, L=0.45)
/// - Badge text: black or white, whichever contrasts better with the
///   background (hues vary widely in perceived luminance at a fixed lightness)
/// - Branch text: brighter (S=0.7, L=0.75)
pub(super) fn name_to_color(name: &str) -> (Color, Color, Color) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish();
    let hue = (hash % 360) as f64;

    let (br, bg, bb) = hsl_to_rgb(hue, 0.6, 0.45);
    let (tr, tg, tb) = hsl_to_rgb(hue, 0.7, 0.75);
    let badge_fg = readable_fg_on(br, bg, bb);
    (Color::Rgb(br, bg, bb), badge_fg, Color::Rgb(tr, tg, tb))
}

/// Format a token count into a human-readable string (e.g. "1.2K", "14.2M").
pub(super) fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}
