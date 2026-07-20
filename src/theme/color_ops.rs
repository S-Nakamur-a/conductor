//! Generic color-math methods on `Theme`: darken/lighten/complement/lerp and
//! the derived high-contrast variant. These operate on any theme's palette
//! rather than defining one.

use super::Theme;
use super::hsl::{hsl_to_rgb, rgb_to_hsl};
use ratatui::style::Color;

impl Theme {
    /// Darken an RGB color by the given factor (0.0 = black, 1.0 = unchanged).
    /// Non-RGB colors are returned unchanged.
    pub fn darken(color: Color, factor: f64) -> Color {
        match color {
            Color::Rgb(r, g, b) => Color::Rgb(
                (r as f64 * factor) as u8,
                (g as f64 * factor) as u8,
                (b as f64 * factor) as u8,
            ),
            other => other,
        }
    }

    /// Return the complementary color: the hue rotated 180° in HSL space while
    /// preserving saturation and lightness. This yields an equally-bright
    /// opposite hue (a green-ish complement for a purple accent, etc.) rather
    /// than the muddy result of a raw RGB inversion. Non-RGB colors are
    /// returned unchanged.
    pub fn complement(color: Color) -> Color {
        match color {
            Color::Rgb(r, g, b) => {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let (r, g, b) = hsl_to_rgb((h + 0.5) % 1.0, s, l);
                Color::Rgb(r, g, b)
            }
            other => other,
        }
    }

    /// Move an RGB color toward white by `amount` in `[0, 1]` (0 = unchanged,
    /// 1 = pure white). The light-mode counterpart to [`darken`], which multiplies
    /// toward black. Non-RGB colors are returned unchanged.
    pub fn lighten(color: Color, amount: f64) -> Color {
        match color {
            Color::Rgb(r, g, b) => {
                let a = amount.clamp(0.0, 1.0);
                let mix = |c: u8| (c as f64 + (255.0 - c as f64) * a).round() as u8;
                Color::Rgb(mix(r), mix(g), mix(b))
            }
            other => other,
        }
    }

    /// Return a higher-contrast variant of this theme, derived generically so
    /// every built-in (and any custom theme) gains a "high contrast mode" without
    /// a hand-authored palette. The transform pushes the dim "secondary" greys
    /// (borders, hints, muted separators, section headers — the usual legibility
    /// offenders) and the body text away from the background, and intensifies
    /// accents. Direction follows the theme's [`light`](Self::light) polarity:
    /// dark themes brighten toward white, light themes deepen toward black.
    pub fn high_contrast(mut self) -> Self {
        // Capture polarity up front so the push closure borrows only a Copy bool,
        // leaving `self`'s fields free to be reassigned below.
        let light = self.light;
        // Push amounts: dim greys move the most (they start closest to the
        // background and hurt readability the most), then body text, then a
        // gentle nudge for accents so they pop without washing out.
        let push = |c: Color, amount: f64| -> Color {
            if light {
                Theme::darken(c, 1.0 - amount)
            } else {
                Theme::lighten(c, amount)
            }
        };
        const TXT: f64 = 0.40;
        const DIM: f64 = 0.55;
        const ACC: f64 = 0.22;

        // Body text + reply bodies.
        self.fg = push(self.fg, TXT);
        self.reply_text = push(self.reply_text, TXT);

        // Dim greys: borders, hints, muted separators, section headers, paths.
        self.muted = push(self.muted, DIM);
        self.hint = push(self.hint, DIM);
        self.dir_fg = push(self.dir_fg, DIM);
        self.border_unfocused = push(self.border_unfocused, DIM);
        self.border_secondary = push(self.border_secondary, DIM);
        self.diff_section_header = push(self.diff_section_header, DIM);
        self.gutter_hover_fg = push(self.gutter_hover_fg, DIM);

        // Accents / semantic colours: intensify a touch.
        self.accent = push(self.accent, ACC);
        self.border_focused = push(self.border_focused, ACC);
        self.info = push(self.info, ACC);
        self.success = push(self.success, ACC);
        self.error = push(self.error, ACC);
        self.warning = push(self.warning, ACC);
        self.diff_add = push(self.diff_add, ACC);
        self.diff_del = push(self.diff_del, ACC);

        self
    }

    /// Linearly interpolate between two RGB colors by `t`, clamped to `[0, 1]`
    /// (`0.0` = `from`, `1.0` = `to`). Used to glide the reflow border between
    /// the accent and its complement. If either color is non-RGB, `from` is
    /// returned unchanged.
    pub fn lerp(from: Color, to: Color, t: f64) -> Color {
        match (from, to) {
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
                let t = t.clamp(0.0, 1.0);
                let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
                Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
            }
            _ => from,
        }
    }
}
