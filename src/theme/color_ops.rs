//! Theme に対する汎用のカラー演算メソッド: darken/lighten/complement/lerp と、
//! それらから導出する高コントラストバリアント。パレットを定義するのではなく、
//! どのテーマのパレットに対しても作用する。

use super::Theme;
use super::hsl::{hsl_to_rgb, rgb_to_hsl};
use ratatui::style::Color;

impl Theme {
    /// RGB カラーを指定した係数だけ暗くする(0.0 = 黒、1.0 = 変化なし)。
    /// RGB でないカラーはそのまま返す。
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

    /// 補色を返す: 彩度と明度を保ったまま HSL 空間で色相を 180° 回転させる。
    /// これにより、生の RGB 反転で得られる濁った結果ではなく、同じ明るさの反対色相
    /// (紫のアクセントなら緑がかった補色、など)が得られる。RGB でないカラーは
    /// そのまま返す。
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

    /// RGB カラーを [0, 1] の amount だけ白へ近づける(0 = 変化なし、1 = 純白)。
    /// 黒へ向けて乗算する [darken] のライトモード版。RGB でないカラーはそのまま返す。
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

    /// このテーマの高コントラストバリアントを返す。汎用的に導出しているため、
    /// 手作りのパレットを用意しなくても組み込み(および任意のカスタムテーマ)すべてが
    /// 「高コントラストモード」を得られる。この変換は、視認性を損ないがちな薄い
    /// 「セカンダリ」グレー(枠線、ヒント、目立たない区切り線、セクションヘッダ)と
    /// 本文テキストを背景から遠ざけ、アクセントを強める。方向はテーマの
    /// [light](Self::light) の極性に従う: ダークテーマは白へ向けて明るく、
    /// ライトテーマは黒へ向けて暗くする。
    pub fn high_contrast(mut self) -> Self {
        // 極性を先に取り出しておくことで、push クロージャは Copy な bool のみを
        // 借用し、self のフィールドは以降で自由に再代入できるようにする。
        let light = self.light;
        // 押し出し量: 薄いグレーは背景に最も近く可読性への影響も最大なので最も強く動かし、
        // 次に本文テキスト、最後にアクセントは色が飛ばない程度に軽く動かす。
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

        // 本文テキストと返信本文。
        self.fg = push(self.fg, TXT);
        self.reply_text = push(self.reply_text, TXT);

        // 薄いグレー群: 枠線、ヒント、目立たない区切り線、セクションヘッダ、パス。
        self.muted = push(self.muted, DIM);
        self.hint = push(self.hint, DIM);
        self.dir_fg = push(self.dir_fg, DIM);
        self.border_unfocused = push(self.border_unfocused, DIM);
        self.border_secondary = push(self.border_secondary, DIM);
        self.diff_section_header = push(self.diff_section_header, DIM);
        self.gutter_hover_fg = push(self.gutter_hover_fg, DIM);

        // アクセント/意味付けされた色: 少しだけ強める。
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

    /// これを下回ると保存すべき色相を持たないとみなす彩度のしきい値。このしきい値未満では
    /// rgb_to_hsl は本質的に任意の色相を返す(完全な無彩色では 0.0、つまり赤を返す)ため、
    /// そうしたカラーを彩度アップすると色相を強めるのではなく作り出してしまう。
    const NEUTRAL_SATURATION: f64 = 0.08;

    /// カラーを、その色自身の色相のまま最も鮮やかな版へ近づける: 彩度を最大へ、明度を
    /// target_l へ、それぞれ [0, 1] の amount だけ寄せる(0.0 = 変化なし)。
    ///
    /// 白/黒へ収束していき入力がすでにどちらかに近いとちょうど余地が尽きる
    /// [lighten](Self::lighten) / [darken](Self::darken) と異なり、この関数には常に
    /// 動ける余地がある: 白に近いカラーでも target_l へ向かう過程で彩度を得る。
    /// 色相は固定するため、意味を担うカラーはそれ自身として読み取れたままになる。
    ///
    /// ほぼ無彩色の入力には保存すべき色相がないため、代わりに hue_fallback の色相を
    /// 借用する。RGB でないカラーはそのまま返す。
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

    /// 2色間の知覚的な距離を「redmean」重み付けで近似する。緑の寄与を強め、
    /// 赤/青の重みを平均赤レベルに応じて変化させることで、単純な RGB のユークリッド距離
    /// より人間の知覚にずっと近い結果になる。しかもフル sRGB→Lab 変換が必要な本物の
    /// CIE ΔE に比べればごくわずかなコストで済む。
    ///
    /// スケールは 0.0(同一)からおよそ 765.0(黒 対 白)までの範囲。RGB でないカラーは
    /// 比較に意味を持たないため 0.0 を返す。
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

    /// 2つの RGB カラーを t([0, 1] にクランプ)で線形補間する
    /// (0.0 = from、1.0 = to)。reflow の枠線をアクセント色とその補色の間で
    /// 滑らかに変化させるのに使う。どちらかが RGB でないカラーなら from をそのまま返す。
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
