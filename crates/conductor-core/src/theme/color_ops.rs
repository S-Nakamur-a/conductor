//! どのパレットにも作用する色の演算。RGB でない色 (Reset や ANSI 名) はそのまま返す。

use super::Theme;
use super::hsl::{hsl_to_rgb, rgb_to_hsl};
use ratatui::style::Color;

impl Theme {
    /// factor を掛けて黒へ寄せる (0.0 = 黒、1.0 = 変化なし)。
    pub fn darken(color: Color, factor: f64) -> Color {
        match color {
            Color::Rgb(r, g, b) => Color::Rgb(
                (f64::from(r) * factor) as u8,
                (f64::from(g) * factor) as u8,
                (f64::from(b) * factor) as u8,
            ),
            other => other,
        }
    }

    /// amount だけ白へ寄せる (0.0 = 変化なし、1.0 = 純白)。
    pub fn lighten(color: Color, amount: f64) -> Color {
        match color {
            Color::Rgb(r, g, b) => {
                let a = amount.clamp(0.0, 1.0);
                let mix = |c: u8| (f64::from(c) + (255.0 - f64::from(c)) * a).round() as u8;
                Color::Rgb(mix(r), mix(g), mix(b))
            }
            other => other,
        }
    }

    /// 彩度と明度を保って色相を 180 度回す。RGB の反転では明るさまで反転して濁るため。
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

    /// from から to へ t ([0, 1] にクランプ) で線形補間する。どちらかが RGB でなければ from。
    pub fn lerp(from: Color, to: Color, t: f64) -> Color {
        match (from, to) {
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
                let t = t.clamp(0.0, 1.0);
                let mix =
                    |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
                Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
            }
            _ => from,
        }
    }

    /// これ未満の彩度では rgb_to_hsl の色相が事実上任意 (無彩色なら 0 = 赤) なので、
    /// 彩度を上げると色相を強めるのではなく作り出してしまう。
    const NEUTRAL_SATURATION: f64 = 0.08;

    /// 色相を固定したまま、彩度を最大へ、明度を target_l へ、amount ([0, 1]) だけ寄せる。
    ///
    /// lighten / darken は白や黒に近い色で動く余地が尽きるが、こちらは常に動ける。
    /// ほぼ無彩色の入力は保つべき色相を持たないので hue_fallback の色相を借りる。
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

    /// 2 色の知覚距離の近似 (redmean 重み付け)。0.0 が同一、黒と白でおよそ 765.0。
    /// RGB でない色を含む比較は 0.0。
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

    /// 高コントラスト版を返す。本文と薄いグレー群を背景から遠ざけ、アクセントを少し強める。
    /// 向きは light に従う: ダークなら白へ、ライトなら黒へ。
    pub fn high_contrast(mut self) -> Self {
        let light = self.light;
        let push_from_bg = |c: Color, amount: f64| -> Color {
            if light {
                Theme::darken(c, 1.0 - amount)
            } else {
                Theme::lighten(c, amount)
            }
        };
        // 薄いグレーは背景に最も近く可読性への影響も最大なので最も強く、
        // アクセントは色が飛ばない程度に軽く。
        const TEXT: f64 = 0.40;
        const DIM: f64 = 0.55;
        const ACCENT: f64 = 0.22;

        self.fg = push_from_bg(self.fg, TEXT);
        self.reply_text = push_from_bg(self.reply_text, TEXT);

        self.muted = push_from_bg(self.muted, DIM);
        self.hint = push_from_bg(self.hint, DIM);
        self.dir_fg = push_from_bg(self.dir_fg, DIM);
        self.border_unfocused = push_from_bg(self.border_unfocused, DIM);
        self.border_secondary = push_from_bg(self.border_secondary, DIM);
        self.diff_section_header = push_from_bg(self.diff_section_header, DIM);
        self.gutter_hover_fg = push_from_bg(self.gutter_hover_fg, DIM);

        self.accent = push_from_bg(self.accent, ACCENT);
        self.border_focused = push_from_bg(self.border_focused, ACCENT);
        self.info = push_from_bg(self.info, ACCENT);
        self.success = push_from_bg(self.success, ACCENT);
        self.error = push_from_bg(self.error, ACCENT);
        self.warning = push_from_bg(self.warning, ACCENT);
        self.diff_add = push_from_bg(self.diff_add, ACCENT);
        self.diff_del = push_from_bg(self.diff_del, ACCENT);

        self
    }
}
