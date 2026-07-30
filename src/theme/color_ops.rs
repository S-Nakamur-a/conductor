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

    /// Saturation below which a color carries no hue worth preserving. Under
    /// this threshold `rgb_to_hsl` reports an essentially arbitrary hue (for a
    /// perfectly achromatic color it reports `0.0`, i.e. red), so saturating
    /// such a color would invent a hue rather than intensify one.
    const NEUTRAL_SATURATION: f64 = 0.08;

    /// Move a color toward the most colorful version of *its own hue*: raise
    /// saturation toward full and slide lightness toward `target_l`, both by
    /// `amount` in `[0, 1]` (`0.0` = unchanged).
    ///
    /// Unlike [`lighten`](Self::lighten) / [`darken`](Self::darken), which
    /// converge on white/black and therefore run out of headroom exactly when
    /// the input is already near one of them, this always has somewhere to go:
    /// a near-white color gains chroma on the way to `target_l`. Hue is held
    /// fixed, so a color that encodes meaning still reads as itself.
    ///
    /// Near-neutral inputs have no hue to preserve, so the hue of
    /// `hue_fallback` is borrowed instead. Non-RGB colors are returned
    /// unchanged.
    pub fn vivify(color: Color, hue_fallback: Color, amount: f64, target_l: f64) -> Color {
        let Color::Rgb(r, g, b) = color else {
            return color;
        };
        let (hue, sat, lum) = rgb_to_hsl(r, g, b);
        let amount = amount.clamp(0.0, 1.0);
        let hue = match hue_fallback {
            Color::Rgb(r, g, b) if sat < Self::NEUTRAL_SATURATION => rgb_to_hsl(r, g, b).0,
            _ => hue,
        };
        let (r, g, b) = hsl_to_rgb(
            hue,
            sat + (1.0 - sat) * amount,
            lum + (target_l - lum) * amount,
        );
        Color::Rgb(r, g, b)
    }

    /// Approximate perceptual distance between two colors, using the "redmean"
    /// weighting. It tracks human perception far better than a plain RGB
    /// euclidean distance — green dominates, and the red/blue weights shift
    /// with the average red level — at a fraction of the cost of a real CIE
    /// ΔE, which would need a full sRGB→Lab conversion.
    ///
    /// The scale runs from `0.0` (identical) to roughly `765.0` (black vs
    /// white). Non-RGB colors have nothing meaningful to compare, so they
    /// report `0.0`.
    pub fn perceptual_distance(a: Color, b: Color) -> f64 {
        let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a, b) else {
            return 0.0;
        };
        let rmean = (f64::from(r1) + f64::from(r2)) / 2.0;
        let dr = f64::from(r1) - f64::from(r2);
        let dg = f64::from(g1) - f64::from(g2);
        let db = f64::from(b1) - f64::from(b2);
        ((2.0 + rmean / 256.0) * dr * dr
            + 4.0 * dg * dg
            + (2.0 + (255.0 - rmean) / 256.0) * db * db)
            .sqrt()
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
