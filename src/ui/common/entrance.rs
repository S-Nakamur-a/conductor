//! 起動と索引の演出 — 描き終わったバッファを進捗で伏せる。
//!
//! レイアウトには触らない。各パネルが最終的な矩形へ描き切ったあとのセルを
//! 加工するだけなので、列幅も PTY のサイズも動かない。幅をアニメーションさせると
//! [`crate::terminal::resize`] がフレームごとに PTY を resize してしまう。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::theme::Theme;

/// 四隅を置いてから辺が伸び始めるまでの割合。
pub const HOLD_RATIO: f64 = 0.25;
/// 伸びる先端から何セルぶん明るさを引きずるか。
pub const EDGE_TRAIL: u16 = 10;
/// パネル間のずらし。
pub const PANEL_STAGGER: f64 = 0.08;
/// 枠が閉じたあとの持ち上げの山の高さ。前景色を目立つ側へどれだけ寄せるか。
pub const GLOW_PEAK: f64 = 0.40;

pub const ENTRANCE_MS: u64 = 640;
pub const INDEX_DONE_MS: u64 = 260;
/// パネルごとの開始時刻のゆらぎ幅。順序は変えず、毎回まったく同じ絵にはしない。
pub const JITTER_MS: i64 = 40;

/// 1 枚のパネルが、自分の持ち時間のうち枠を描くのに使う割合。
const FRAME_SHARE: f64 = 0.55;
/// 不定バーの 1 セルを 8 段に割るブロック。
const EIGHTH: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// 両端で傾きが 0 になる smoothstep。[`crate::anim::eased_progress`] と同じ曲線だが、
/// こちらは Duration ではなく正規化済みの進捗を取る。
fn ease(p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

/// 起動演出を 1 フレーム分適用する。
///
/// panels は最終レイアウトの矩形。アニメーション中に再計算しないので四隅は動かない。
/// offsets は [`offsets`] が作る、パネルごとの開始時点。
pub fn apply_entrance(
    buf: &mut Buffer,
    area: Rect,
    panels: &[Rect],
    progress: f64,
    offsets: &[f64],
    theme: &Theme,
) {
    if progress >= 1.0 {
        return;
    }
    let closed = frames_closed_at(offsets);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            match locate(panels, x, y) {
                Cell::Edge(i) => {
                    let p = staggered(progress, i, offsets) / FRAME_SHARE;
                    paint_edge(buf, panels[i], x, y, p.min(1.0), progress, theme);
                }
                // 枠が閉じきるまで中身は伏せる。まだ辺が伸びている横で本文が
                // 読めると、枠が飾りに見える。
                Cell::Inside | Cell::Chrome => {
                    if progress < closed
                        && let Some(cell) = buf.cell_mut((x, y))
                    {
                        cell.reset();
                    }
                }
            }
        }
    }
    apply_glow(buf, area, progress, closed, theme);
}

/// 最後のパネルの枠が閉じきる時点。
///
/// ずらしが大きいほど後ろへ動くので offsets から導く。固定値にすると、
/// ずらしを増やしたときに中身が枠より先に出る。
fn frames_closed_at(offsets: &[f64]) -> f64 {
    let last = offsets.last().copied().unwrap_or(0.0);
    last + FRAME_SHARE * (1.0 - last)
}

/// 枠が閉じたあと、画面全体をひと呼吸だけ持ち上げて「もう使える」を示す。
///
/// 山なりに上がって戻るので終端では何も残らない。前景しか触らないのは、
/// 端末の既定背景が [`Color::Reset`] で [`Theme::lerp`] が効かないため。
fn apply_glow(buf: &mut Buffer, area: Rect, progress: f64, start: f64, theme: &Theme) {
    let g = (progress - start) / (1.0 - start);
    if g <= 0.0 {
        return;
    }
    let k = (g.clamp(0.0, 1.0) * std::f64::consts::PI).sin() * GLOW_PEAK;
    if k <= 0.0 {
        return;
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_fg(raise(theme, cell.fg, k));
            }
        }
    }
}

/// 色を「目立つ側」へ寄せる。ダークテーマでは白へ、ライトテーマでは黒へ。
/// どちらも白へ寄せると、ライトテーマでは文字が地に溶けて読めなくなる。
fn raise(theme: &Theme, color: Color, k: f64) -> Color {
    if theme.light {
        Theme::lerp(color, Color::Rgb(0, 0, 0), k)
    } else {
        Theme::lighten(color, k)
    }
}

/// 索引の実行中を示す不定バー。
///
/// [`crate::semantic_index::Reading`] は状態しか持たず進行度が取れないので、
/// 割合ではなく「流れ続ける光」で表す。埋め尽くすと残り時間として読まれるので、
/// 先端から数セルだけを尾として残す。
pub fn apply_index_bar(buf: &mut Buffer, panel: Rect, phase: f64, theme: &Theme) {
    if panel.width < 4 {
        return;
    }
    let span = panel.width.saturating_sub(2);
    let head = phase.rem_euclid(1.0) * f64::from(span);
    let lit = head.floor() as u16;
    let frac = ((head - head.floor()) * 8.0) as usize;
    for i in 0..span {
        let x = panel.x + 1 + i;
        let Some(cell) = buf.cell_mut((x, panel.y)) else {
            continue;
        };
        let behind = lit.saturating_sub(i);
        if i == lit {
            if frac > 0 {
                cell.set_symbol(EIGHTH[frac]);
                cell.set_fg(theme.fg);
            }
        } else if i < lit && behind <= BAR_TAIL {
            cell.set_symbol("█");
            cell.set_fg(Theme::lerp(
                theme.accent,
                theme.border_unfocused,
                f64::from(behind) / f64::from(BAR_TAIL),
            ));
        }
    }
}

/// 不定バーの尾の長さ。
const BAR_TAIL: u16 = 6;

/// 索引の完了。枠を一度 accent へ沸かせてから通常のボーダー色へ落とす。
pub fn apply_index_done(buf: &mut Buffer, panel: Rect, progress: f64, theme: &Theme) {
    if progress >= 1.0 {
        return;
    }
    let flash = (1.0 - progress / 0.35).max(0.0);
    let settle = ease(((progress - 0.3) / 0.7).max(0.0));
    let peak = Theme::lerp(theme.accent, theme.fg, flash);
    let color = Theme::lerp(peak, theme.border_unfocused, settle);
    for_each_edge(panel, |x, y| {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_fg(color);
        }
    });
}

/// セルがどの構造に属するか。
enum Cell {
    /// panels[i] の枠線上。
    Edge(usize),
    /// どこかのパネルの内側。
    Inside,
    /// どのパネルにも属さない (タイトルバー、メニュー、ストリップ、ステータスバー)。
    Chrome,
}

fn locate(panels: &[Rect], x: u16, y: u16) -> Cell {
    for (i, pan) in panels.iter().enumerate() {
        if !contains(*pan, x, y) {
            continue;
        }
        return if on_edge(*pan, x, y) {
            Cell::Edge(i)
        } else {
            Cell::Inside
        };
    }
    Cell::Chrome
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

fn on_edge(r: Rect, x: u16, y: u16) -> bool {
    x == r.x || x == r.right().saturating_sub(1) || y == r.y || y == r.bottom().saturating_sub(1)
}

/// パネル i にとっての進捗。offsets のぶん遅れて始まる。
fn staggered(progress: f64, i: usize, offsets: &[f64]) -> f64 {
    let offset = offsets.get(i).copied().unwrap_or(0.0);
    let last = offsets.last().copied().unwrap_or(0.0);
    ((progress - offset) / (1.0 - last).max(0.1)).clamp(0.0, 1.0)
}

/// パネルごとの開始時点。ずらしにゆらぎを乗せたうえで、前のパネルより早く
/// 始まらないよう押し出す。
///
/// ゆらぎ幅がずらしより広いと素朴な加算では順序が入れ替わる。左から順に開く形は
/// 保ったまま、毎回まったく同じ絵にはならないようにするための単調化。
pub fn offsets(jitter: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(jitter.len());
    let mut prev = f64::NEG_INFINITY;
    for (i, j) in jitter.iter().enumerate() {
        let v = (PANEL_STAGGER * i as f64 + j).max(prev);
        out.push(v);
        prev = v;
    }
    out
}

fn for_each_edge(pan: Rect, mut f: impl FnMut(u16, u16)) {
    if pan.width == 0 || pan.height == 0 {
        return;
    }
    for y in pan.y..pan.bottom() {
        for x in pan.x..pan.right() {
            if on_edge(pan, x, y) {
                f(x, y);
            }
        }
    }
}

/// 枠セルの、四隅からの距離と辺の半分の長さ。
///
/// 四隅は与えられた矩形からそのまま読むだけで、進捗によらず動かない。
fn edge_distance(pan: Rect, x: u16, y: u16) -> (f64, f64) {
    let on_top_bottom = y == pan.y || y == pan.bottom().saturating_sub(1);
    if on_top_bottom {
        let d = (x - pan.x).min(pan.right().saturating_sub(1) - x);
        (f64::from(d), f64::from(pan.width) / 2.0 + 1.0)
    } else {
        let d = (y - pan.y).min(pan.bottom().saturating_sub(1) - y);
        (f64::from(d), f64::from(pan.height) / 2.0 + 1.0)
    }
}

fn is_corner(pan: Rect, x: u16, y: u16) -> bool {
    (x == pan.x || x == pan.right().saturating_sub(1))
        && (y == pan.y || y == pan.bottom().saturating_sub(1))
}

fn paint_edge(
    buf: &mut Buffer,
    pan: Rect,
    x: u16,
    y: u16,
    local: f64,
    overall: f64,
    theme: &Theme,
) {
    let Some(cell) = buf.cell_mut((x, y)) else {
        return;
    };
    // 溜めのあいだは四隅だけが先に出る。ここが起動のたびに同じ位置に落ちる。
    if is_corner(pan, x, y) {
        let pop = ease(local / HOLD_RATIO.max(0.06));
        cell.set_fg(Theme::lerp(theme.border_unfocused, theme.fg, pop));
        return;
    }
    let t = ease((local - HOLD_RATIO) / (1.0 - HOLD_RATIO));
    if t <= 0.0 {
        cell.reset();
        return;
    }
    let (dist, span) = edge_distance(pan, x, y);
    let head = t * span;
    if dist > head {
        cell.reset();
        return;
    }
    let back = (head - dist) / f64::from(EDGE_TRAIL.max(1));
    let color = if t >= 1.0 {
        Theme::lerp(
            theme.accent,
            theme.border_unfocused,
            ease((overall - 0.78) / 0.22),
        )
    } else if back < 0.14 {
        theme.fg
    } else {
        Theme::lerp(theme.accent, theme.border_unfocused, (back - 0.14) / 0.86)
    };
    cell.set_fg(color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn theme() -> Theme {
        Theme::from_name("dracula")
    }

    /// 前景色まで入れて埋める。Color::Reset のままだと lighten も lerp も
    /// 素通しになり、色を動かす側のテストが素通りしてしまう。
    fn filled(area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = buf.cell_mut((x, y)).unwrap();
                cell.set_symbol("x");
                cell.set_fg(theme().fg);
            }
        }
        buf
    }

    fn panels() -> Vec<Rect> {
        vec![
            Rect::new(0, 3, 17, 11),
            Rect::new(17, 3, 24, 11),
            Rect::new(41, 3, 16, 8),
        ]
    }

    /// 四隅は進捗によらず同じ位置に居続ける。ここが動くと枠が「組み上がる」ではなく
    /// 「泳ぐ」ように見える。
    #[test]
    fn 四隅は進捗のどの時点でも同じ位置にある() {
        let area = Rect::new(0, 0, 58, 15);
        let corners: Vec<(u16, u16)> = panels()
            .iter()
            .flat_map(|p| {
                [
                    (p.x, p.y),
                    (p.right() - 1, p.y),
                    (p.x, p.bottom() - 1),
                    (p.right() - 1, p.bottom() - 1),
                ]
            })
            .collect();

        for step in 0..=20 {
            let p = f64::from(step) / 20.0;
            let mut buf = filled(area);
            apply_entrance(&mut buf, area, &panels(), p, &[0.0; 4], &theme());
            for &(x, y) in &corners {
                assert_eq!(
                    buf.cell((x, y)).unwrap().symbol(),
                    "x",
                    "四隅 ({x},{y}) が progress={p} で消えた"
                );
            }
        }
    }

    #[test]
    fn 完了時は一切伏せない() {
        let area = Rect::new(0, 0, 58, 15);
        let mut buf = filled(area);
        apply_entrance(&mut buf, area, &panels(), 1.0, &[0.0; 4], &theme());
        for y in 0..15 {
            for x in 0..58 {
                assert_eq!(buf.cell((x, y)).unwrap().symbol(), "x");
            }
        }
    }

    #[test]
    fn 開始時はパネルの内側が伏せられる() {
        let area = Rect::new(0, 0, 58, 15);
        let mut buf = filled(area);
        apply_entrance(&mut buf, area, &panels(), 0.0, &[0.0; 4], &theme());
        assert_eq!(buf.cell((5, 6)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((25, 8)).unwrap().symbol(), " ");
    }

    /// 中身は最後のパネルの枠が閉じるまで出ない。固定値で判定していると、
    /// ずらしを増やしたときに枠より先に本文が読める。
    #[test]
    fn 中身はずらしを増やしても枠の完成を待つ() {
        let area = Rect::new(0, 0, 58, 15);
        let wide = vec![0.0, 0.2, 0.4, 0.6];
        let closed = frames_closed_at(&wide);
        assert!(closed > FRAME_SHARE, "ずらしを増やしても待ち時間が伸びない");

        let mut buf = filled(area);
        apply_entrance(&mut buf, area, &panels(), closed - 0.05, &wide, &theme());
        assert_eq!(
            buf.cell((25, 8)).unwrap().symbol(),
            " ",
            "枠が閉じる前に中身が出ている"
        );
    }

    /// ひと呼吸なので、終わったあとに色が残ってはいけない。残ると起動のたびに
    /// 画面がじわっと明るいままになる。
    #[test]
    fn ひと呼吸の光は終端で残らない() {
        let area = Rect::new(0, 0, 58, 15);
        let th = theme();
        let base = filled(area);

        let mut peak_seen = false;
        for step in 0..=20 {
            let p = f64::from(step) / 20.0;
            let mut buf = filled(area);
            apply_glow(&mut buf, area, p, 0.55, &th);
            let changed = buf.cell((25, 8)).unwrap().fg != base.cell((25, 8)).unwrap().fg;
            if (0.7..0.85).contains(&p) {
                peak_seen |= changed;
            }
            if p >= 1.0 {
                assert!(!changed, "progress={p} で色が残っている");
            }
        }
        assert!(peak_seen, "山が立っていない");
    }

    /// ゆらぎがずらしより広くても左から順のまま。入れ替わると「毎回違う順で開く」
    /// ことになり、固定順を選んだ意図と食い違う。
    #[test]
    fn ゆらぎが広くてもパネルの順序は入れ替わらない() {
        let wide = PANEL_STAGGER * 4.0;
        for case in [
            vec![wide, -wide, wide, -wide],
            vec![-wide, wide, -wide, wide],
            vec![0.0, 0.0, 0.0, 0.0],
        ] {
            let got = offsets(&case);
            for pair in got.windows(2) {
                assert!(
                    pair[0] <= pair[1],
                    "順序が入れ替わった: {got:?} (jitter={case:?})"
                );
            }
        }
    }

    /// 不定バーは進捗ではないので、どの時点でも辺を埋め尽くさない。
    /// 埋まって見えると「あと少し」と読まれてしまう。
    #[test]
    fn 不定バーは辺を埋め尽くさない() {
        let panel = Rect::new(17, 3, 24, 11);
        let area = Rect::new(0, 0, 58, 15);
        for step in 0..20 {
            let phase = f64::from(step) / 20.0;
            let mut buf = filled(area);
            apply_index_bar(&mut buf, panel, phase, &theme());
            let lit = (1..panel.width - 1)
                .filter(|i| buf.cell((panel.x + i, panel.y)).unwrap().symbol() == "█")
                .count();
            assert!(
                lit <= usize::from(BAR_TAIL) + 1,
                "phase={phase} で {lit} セルが点灯した"
            );
        }
    }

    #[test]
    fn 索引完了は枠だけに触る() {
        let panel = Rect::new(17, 3, 24, 11);
        let area = Rect::new(0, 0, 58, 15);
        let mut buf = filled(area);
        let before = buf.cell((25, 8)).unwrap().style();
        apply_index_done(&mut buf, panel, 0.5, &theme());
        assert_eq!(buf.cell((25, 8)).unwrap().style(), before, "内側が変わった");
        assert_ne!(
            buf.cell((17, 3)).unwrap().style(),
            Style::default(),
            "枠が変わっていない"
        );
    }
}
