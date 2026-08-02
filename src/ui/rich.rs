//! Rich mode Tier A のエフェクト — 回転するグラデーションボーダー。
//!
//! [crate::term_caps::RichTier] が Tier A 以上のとき、描画済みのフレーム
//! バッファに対して後処理を行う（party.rs と同じパターン）:
//!
//! 1. フォーカス中パネルのボーダーグラデーション — フォーカス中のボーダーの
//!    グリフを、テーマから導出した円錐グラデーション（border_focused を中心
//!    とした色相の揺れ）で再着色し、パネルの周りをゆっくり回転させる。CSS の
//!    conic-gradient のグローのような見た目になる。明度はテーマの色より
//!    *暗くなる* 方向にしか振れないので、ボーダーが白く飛ぶことはない。意図的に
//!    ゆっくりで低彩度にしてある: 大きな主張をせずにフォーカスを示すため。
//!    フォーカスされていないボーダーには手を付けない。
//! 2. Claude 待機中のグロー — 選択中の worktree の Claude セッションが入力を
//!    待っているとき、Claude パネルのボーダーがテーマの waiting 色で明滅する。
//!    フォーカスグラデーションより速く暖色寄りにすることで、周辺視野でも
//!    2つの状態を区別できるようにしてある。フォーカスグラデーションの後に
//!    適用される（つまりそちらより優先される）。
//!
//! どちらのエフェクトも、描画時点でアクティブな [crate::theme::Theme] から
//! すべての色を導出する — テーマごとのグラデーションデータは保持しない。
//!
//! アニメーションのフェーズは ui_tick ではなく壁時計時間（App::rich_epoch）
//! から導出するので、体感速度が再描画レートによって変わることはない。
//! エフェクトが *見た目上進む* のは何かがフレームを再描画したとき
//! （入力、PTY 出力、待機中のパルス）だけ — 完全にアイドルな画面は意図的に
//! グラデーションの途中で止まったままになり、再描画タイマーを強制する
//! のではなくアイドル時の CPU 使用をゼロに保つ。
//!
//! party モードが有効な間はこのパス全体をスキップする: party は
//! border_focused との色の一致でフォーカス中のボーダーを検出しており、
//! このエフェクトがあるとその判定が壊れてしまうため。

use std::f64::consts::TAU;

use ratatui::Frame;
use ratatui::style::{Color, Modifier};

use crate::app::App;

use super::party::{hsl_to_rgb, is_border_glyph};

/// フォーカスグラデーションがパネルを1周する秒数。
/// 人が動きとして知覚できる窓は4〜6秒: これより遅いと動きに見えなくなり、
/// 速いとアンビエントな合図としては気が散ってしまう。
const FOCUS_ROTATE_PERIOD_SECS: f64 = 6.0;
/// 色相の揺れ幅（border_focused の色相を中心に、両側それぞれ何度か）。
const FOCUS_HUE_SWEEP: f64 = 24.0;
/// グラデーションの谷でテーマ色よりどれだけ明度が下がるか（テーマの明度に対する
/// 割合）。山はテーマ色そのものなので、グラデーションは暗くなる一方で白へは
/// 決して明るくならない。
const FOCUS_LIGHTNESS_DIP: f64 = 0.30;
/// ターミナルのセルは幅よりおおよそ2倍縦長なので、y方向の距離をスケールして
/// 回転が縦につぶれずに円形に見えるようにする。
const CELL_ASPECT: f64 = 2.0;
/// 待機中グローの明滅周期（秒）— フォーカスの明滅よりわざと速くしてあり、
/// 「フォーカス中」がアンビエントに見えるのに対して「Claude があなたを
/// 必要としている」が緊急に見えるようにしている。
const WAITING_BREATH_PERIOD_SECS: f64 = 1.6;

/// 描画直後のフレームバッファに rich mode Tier A のエフェクトをすべて適用する。
///
/// render_ui の終わり（party モードのパスの前）で呼ばれる。party モードが
/// 有効なときはそちらが完全に上書きする。
pub fn apply_rich_effects(frame: &mut Frame, app: &App) {
    let t = app.rich.epoch.elapsed().as_secs_f64();
    apply_focus_gradient(frame, app, t);
    apply_waiting_glow(frame, app, t);
}

/// フォーカス中のボーダーのグリフをすべて、回転する円錐グラデーションで再着色する。
///
/// party モードと同様、グリフは border_focused との色の一致で見つける:
/// フォーカス中のパネルだけがその色でボーダーを塗るので、この一致だけで
/// エフェクトの適用範囲が自動的に絞られる（意図的にフォーカス色を使っている
/// オーバーレイも含む）。
///
/// グラデーションの中心は一致したグリフのバウンディングボックス（つまり
/// フォーカス中パネルの矩形）なので、明るい山は画面を斜めに横切るのではなく、
/// パネルの周りを目に見えて周回する。
fn apply_focus_gradient(frame: &mut Frame, app: &App, t: f64) {
    let focused = app.theme.border_focused;
    let Some((h, s, l)) = rgb_to_hsl(focused) else {
        return;
    };

    let area = frame.area();
    let buf = frame.buffer_mut();

    // パス1: フォーカス中ボーダーのグリフのバウンディングボックス → グラデーションの中心。
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u16::MAX, u16::MAX, 0u16, 0u16);
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell((x, y))
                && cell.fg == focused
                && is_border_glyph(cell.symbol())
            {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x > max_x {
        return; // 画面上にフォーカス中のボーダーがない
    }
    let cx = (min_x as f64 + max_x as f64) / 2.0;
    let cy = (min_y as f64 + max_y as f64) / 2.0;

    // パス2: 中心を軸に、時間とともに回転する円錐グラデーション。
    let rotation = t * TAU / FOCUS_ROTATE_PERIOD_SECS;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if let Some(cell) = buf.cell_mut((x, y))
                && cell.fg == focused
                && is_border_glyph(cell.symbol())
            {
                let angle = ((y as f64 - cy) * CELL_ASPECT).atan2(x as f64 - cx);
                let (hue, lightness) = conic_gradient_hsl(h, l, angle - rotation);
                cell.fg = hsl_to_rgb(hue, s, lightness);
            }
        }
    }
}

/// パネルの周りの位相 phase ラジアンにおける、フォーカスグラデーションの
/// 色相と明度。山（sin(phase) = 1）はテーマ色そのものであり、谷は
/// FOCUS_LIGHTNESS_DIP だけ暗くなる。そのためグラデーションはテーマ色より
/// 明るくなることはなく、白く飛ぶこともない。
fn conic_gradient_hsl(h: f64, l: f64, phase: f64) -> (f64, f64) {
    let wave = phase.sin();
    let hue = (h + wave * FOCUS_HUE_SWEEP).rem_euclid(360.0);
    let lightness = l * (1.0 - FOCUS_LIGHTNESS_DIP * (0.5 - 0.5 * wave));
    (hue, lightness)
}

/// 選択中の worktree のセッションが入力を待っている間、Claude パネルの
/// ボーダーを waiting 色で明滅させる。
///
/// パネルが焦点を持つかどうかにかかわらず動くよう、色の一致ではなく layout
/// キャッシュのパネル矩形を対象にする。オーバーレイが開いている間はスキップ
/// する: そうしないとグローがパネル領域を横切るオーバーレイのボーダーまで
/// 再着色してしまうし、そもそもユーザは既にオーバーレイを操作中であるため。
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

    let rect = app.layout.cache.terminal_split[0];
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

    // 外周を走査する。Claude パネルには上端のボーダー行がない（そこには
    // セッションタブの行がある）ので、上端はボーダーのグリフを見つけないだけ。
    for x in left..=right {
        paint(x, top, buf);
        paint(x, bottom, buf);
    }
    for y in top..=bottom {
        paint(left, y, buf);
        paint(right, y, buf);
    }
}

/// RGB の [Color] を HSL（h: 0-360, s: 0-1, l: 0-1）に変換する。
/// RGB でない色（indexed/named）には None を返し、rich エフェクトは
/// それらに手を付けない。
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
        return Some((0.0, 0.0, l)); // 無彩色
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

/// 2つの RGB 色の間の線形補間（t: 0 = a, 1 = b）。
/// どちらかが RGB でない場合は b にフォールバックする。
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

    /// RGB → HSL → RGB のラウンドトリップで許容する、チャンネルごとの最大誤差。
    const ROUND_TRIP_TOLERANCE: i32 = 2;

    #[test]
    fn rgb_hsl_round_trips_theme_colors() {
        // すべての組み込みテーマの border/waiting 色がラウンドトリップに耐えな
        // ければならない。さもないとグラデーションがテーマの色を目に見えて
        // ずらしてしまう。
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
        // 範囲外の t はクランプされる。
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
        // グラデーションはテーマの色相から大きく離れてはならない: 1周分
        // サンプリングして色相の距離を検証する。
        let theme = crate::theme::Theme::from_name("catppuccin-mocha");
        let (h0, _, _) = rgb_to_hsl(theme.border_focused).unwrap();
        for step in 0..360 {
            let phase = (step as f64).to_radians();
            let (hue, _) = conic_gradient_hsl(h0, 0.8, phase);
            let dist = (hue - h0).abs().min(360.0 - (hue - h0).abs());
            assert!(
                dist <= FOCUS_HUE_SWEEP + 0.001,
                "hue drifted {dist}° at phase={phase}"
            );
        }
    }

    #[test]
    fn focus_gradient_never_brightens_past_theme() {
        // 以前の明滅エフェクトは明度をテーマ色より上に押し上げてボーダーを
        // 白く飛ばしていた。回転グラデーションは常に暗くなる方向にしか
        // 振れてはならない。
        for step in 0..360 {
            let phase = (step as f64).to_radians();
            let (_, lightness) = conic_gradient_hsl(260.0, 0.8, phase);
            assert!(
                lightness <= 0.8 + 1e-9,
                "lightness {lightness} exceeded theme at phase={phase}"
            );
            assert!(
                lightness >= 0.8 * (1.0 - FOCUS_LIGHTNESS_DIP) - 1e-9,
                "lightness {lightness} dipped past the trough at phase={phase}"
            );
        }
    }
}
