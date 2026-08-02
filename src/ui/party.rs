//! パーティモード — 隠しフラッシュなイースターエッグオーバーレイ。
//!
//! [crate::app::App::party_mode] が有効なとき、このモジュールは描画済みのフレーム
//! バッファを後処理して、ui_tick でアニメーションする3つの効果を加える:
//!
//! 1. 虹色フォーカスボーダー — テーマのフォーカスボーダー色で描かれたすべてのボーダー
//!    文字を流れる虹色に塗り替え、現在フォーカスされているパネルが光って揺らめくようにする。
//! 2. 虹色タイトルバー — 画面上部のタイトルバーの文字がきらめく。
//! 3. 紙吹雪 — メインコンテンツ領域に星がゆっくり降ってくる。
//!
//! シンタックストークンの虹色効果（Viewer 向け）は viewer_panel.rs にあり、
//! ここの [rainbow] を再利用している。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::app::App;

/// HSL（h: 0-360, s: 0-1, l: 0-1）を RGB の [Color] に変換する。
///
/// ローカルにコピーしたもの（common.rs 側の同等品は private）。rich.rs の
/// リッチモード効果と共有している。h は 0-360 に正規化済みのものを渡すこと。
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

/// 指定した phase（色相の度数として解釈する）に対応する鮮やかな虹色。
///
/// phase が360の倍数だけ異なっていても同じ色になるので、呼び出し側は位置の項と
/// 時間の項を自由に組み合わせて流れるようなグラデーションを作れる。
pub fn rainbow(phase: f64) -> Color {
    hsl_to_rgb(phase.rem_euclid(360.0), 1.0, 0.6)
}

/// s が罫線素片（U+2500..=U+257F）、つまりパネルのボーダー文字で始まるかどうか。
/// テキスト内容に触れずボーダーだけを対象にするために使う。rich.rs の
/// リッチモード効果と共有している。
pub(crate) fn is_border_glyph(s: &str) -> bool {
    matches!(s.chars().next(), Some(c) if ('\u{2500}'..='\u{257F}').contains(&c))
}

/// 紙吹雪の配置用の小さな決定的ハッシュ — rand への依存はなく、フレームをまたいで
/// 安定する（時間の項は呼び出し側が加えるだけ）。
fn pseudo_random(seed: u64) -> u64 {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    x ^= x >> 16;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    x
}

/// 1セル分のきらめき文字（それぞれ端末上でちょうど1カラム幅）。
const SPARKLES: &[&str] = &["✦", "✧", "·", "*", "+", "✩"];

/// 描画直後のフレームバッファに、パーティモードの効果をすべて適用する。
///
/// app.party_mode が有効なとき render_ui の最後で呼ばれるため、現在画面上にある
/// もの（フォーカスボーダー色を使っているオーバーレイの枠も含む）を塗り替える。
pub fn apply_party_effects(frame: &mut Frame, app: &App) {
    let tick = app.ui_tick as f64;
    let focused = app.theme.border_focused;
    let area = frame.area();
    let buf = frame.buffer_mut();

    // 効果1: 虹色フォーカスボーダー。
    // フォーカス中のパネルだけが border_focused でボーダーを描くので、その色に
    // マッチさせるだけで自動的に虹色効果がアクティブパネルに限定される。
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

    // 効果2: 虹色タイトルバー。
    let title = app.layout.cache.title_area;
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

    // 効果3: 漂う紙吹雪。
    draw_confetti(buf, app.layout.cache.main_area, tick);
}

/// area 全体にきらめきを散らし、tick が進むにつれて下方向に漂わせる。
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
        // 各きらめきはわずかに異なる速度で落ち、縦方向にラップする。
        let speed = 2 + (rx % 3); // 1行あたりのtick数に幅を持たせる除数
        let drift = (tick as u64 / speed).wrapping_add(rx >> 8);
        let y = area.y + (drift % h) as u16;
        let glyph = SPARKLES[(rx as usize >> 4) % SPARKLES.len()];

        if let Some(cell) = buf.cell_mut((x, y)) {
            // グラフィックスプロトコルのセルには触れない: リッチモードのピクセル画像
            // プレビューは、その領域を Unicode のプレースホルダ文字（plane-16 の
            // private use）でマークしている。上書きすると次の全画面再描画まで
            // 画像に穴が開いてしまう。
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
        // 罫線素片（細線 + 太線）はボーダーである。
        assert!(is_border_glyph("│"));
        assert!(is_border_glyph("─"));
        assert!(is_border_glyph("┏"));
        assert!(is_border_glyph("┃"));
        // 普通のテキストや空白はボーダーではない。
        assert!(!is_border_glyph("a"));
        assert!(!is_border_glyph(" "));
        assert!(!is_border_glyph(""));
        assert!(!is_border_glyph("✦"));
    }
}
