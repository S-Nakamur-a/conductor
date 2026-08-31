//! バッジ/ラベルの描画で共有する色計算: HSL 生成、WCAG コントラスト、
//! リポジトリごとのバッジ色の導出。

use ratatui::style::Color;
use std::hash::{Hash, Hasher};

/// HSL（h: 0-360, s: 0-1, l: 0-1）を RGB に変換する。
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

/// WCAG 2.1（SC 1.4.3, D65）に基づく sRGB カラーの相対輝度。
///
/// 各チャンネルは 0-255。それぞれ 0-1 に正規化し、ガンマ展開してリニア光にした後、
/// 標準の輝度係数で合成する。
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

/// 指定した背景に対して WCAG コントラストが最良になるよう、黒か白のテキストを選ぶ。
/// バッジは一目でリポジトリを識別するためのものなので、どちらの候補も 4.5:1 の基準を
/// 満たさない場合でも失敗させず、常により高コントラストな方を選ぶ。named ANSI color
/// ではなく明示的な RGB を使うことで、描画結果を輝度計算と一致させている。
pub(super) fn readable_fg_on(r: u8, g: u8, b: u8) -> Color {
    let bg = relative_luminance(r, g, b);
    // コントラスト比 (L1 + 0.05) / (L2 + 0.05)。黒は L=0、白は L=1。
    let contrast = |fg: f64| (bg.max(fg) + 0.05) / (bg.min(fg) + 0.05);
    if contrast(0.0) >= contrast(1.0) {
        Color::Rgb(0, 0, 0)
    } else {
        Color::Rgb(255, 255, 255)
    }
}

/// リポジトリ名からバッジ背景色、バッジ文字色、ブランチ文字色を生成する。
///
/// 名前のハッシュから色相を決め、そこから3色を作る:
/// - バッジ背景: 控えめ（S=0.6, L=0.45）
/// - バッジ文字: 背景とのコントラストがより良い方の黒か白
///   （色相によって同じ明度でも知覚される輝度が大きく異なるため）
/// - ブランチ文字: 明るめ（S=0.7, L=0.75）
pub(crate) fn name_to_color(name: &str) -> (Color, Color, Color) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish();
    let hue = (hash % 360) as f64;

    let (br, bg, bb) = hsl_to_rgb(hue, 0.6, 0.45);
    let (tr, tg, tb) = hsl_to_rgb(hue, 0.7, 0.75);
    let badge_fg = readable_fg_on(br, bg, bb);
    (Color::Rgb(br, bg, bb), badge_fg, Color::Rgb(tr, tg, tb))
}

/// トークン数を人が読みやすい文字列（例: "1.2K", "14.2M"）に整形する。
pub(crate) fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}
