//! 四隅を置き、辺を伸ばして枠を組み上げ、閉じきってから中身を見せる。

use conductor_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::stagger::Stagger;
use super::{ease, on_edge};

pub(super) const DURATION_MS: u64 = 640;
/// パネルごとの開始のずらしと、そこに乗せるゆらぎの幅 (進捗の割合)。
pub(super) const STAGGER_STEP: f64 = 0.08;
pub(super) const JITTER: f64 = 40.0 / DURATION_MS as f64;

/// 四隅を置いてから辺が伸び始めるまでの割合。
const HOLD_RATIO: f64 = 0.25;
/// 伸びる先端から何セルぶん明るさを引きずるか。
const EDGE_TRAIL: u16 = 10;
/// 枠が閉じたあとの持ち上げの山の高さ。
const GLOW_PEAK: f64 = 0.40;
/// 1 枚のパネルが持ち時間のうち枠を描くのに使う割合。
const FRAME_SHARE: f64 = 0.55;

pub(super) fn paint(
    buf: &mut Buffer,
    area: Rect,
    panels: &[Rect],
    progress: f64,
    stagger: &Stagger,
    theme: &Theme,
) {
    if progress >= 1.0 {
        return;
    }
    let closed = frames_closed_at(stagger);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            match locate(panels, x, y) {
                Part::Edge(i) => {
                    let p = stagger.local(progress, i) / FRAME_SHARE;
                    paint_edge(buf, panels[i], x, y, p.min(1.0), progress, theme);
                }
                // 枠が閉じきるまで中身は伏せる。まだ辺が伸びている横で本文が読めると、
                // 枠が飾りに見える。
                Part::Inside | Part::Chrome => {
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

/// 最後のパネルの枠が閉じきる時点。ずらしが大きいほど後ろへ動くのでずらしから導く。
/// 固定値にすると、ずらしを増やしたときに中身が枠より先に出る。
fn frames_closed_at(stagger: &Stagger) -> f64 {
    let last = stagger.last();
    last + FRAME_SHARE * (1.0 - last)
}

/// 枠が閉じたあと、画面全体をひと呼吸だけ持ち上げて「もう使える」を示す。
///
/// 前景しか触らないのは、端末の既定背景が [Color::Reset] で [Theme::lerp] が
/// 効かないため。
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

/// 色を「目立つ側」へ寄せる。どちらも白へ寄せると、ライトテーマでは文字が地に溶ける。
fn raise(theme: &Theme, color: Color, k: f64) -> Color {
    if theme.light {
        Theme::lerp(color, Color::Rgb(0, 0, 0), k)
    } else {
        Theme::lighten(color, k)
    }
}

enum Part {
    /// panels[i] の枠線上。
    Edge(usize),
    Inside,
    /// どのパネルにも属さない (タイトルバー、メニュー、ストリップ、ステータスバー)。
    Chrome,
}

fn locate(panels: &[Rect], x: u16, y: u16) -> Part {
    for (i, pan) in panels.iter().enumerate() {
        if !contains(*pan, x, y) {
            continue;
        }
        return if on_edge(*pan, x, y) {
            Part::Edge(i)
        } else {
            Part::Inside
        };
    }
    Part::Chrome
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// 枠セルの、四隅からの距離と辺の半分の長さ。
fn edge_distance(pan: Rect, x: u16, y: u16) -> (f64, f64) {
    if y == pan.y || y == pan.bottom().saturating_sub(1) {
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
    use super::super::tests::{AREA, filled, theme};
    use super::*;

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
            let mut buf = filled(AREA);
            paint(&mut buf, AREA, &panels(), p, &Stagger::uniform(5), &theme());
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
        let mut buf = filled(AREA);
        paint(
            &mut buf,
            AREA,
            &panels(),
            1.0,
            &Stagger::uniform(5),
            &theme(),
        );
        for y in 0..AREA.height {
            for x in 0..AREA.width {
                assert_eq!(buf.cell((x, y)).unwrap().symbol(), "x");
            }
        }
    }

    #[test]
    fn 開始時はパネルの内側が伏せられる() {
        let mut buf = filled(AREA);
        paint(
            &mut buf,
            AREA,
            &panels(),
            0.0,
            &Stagger::uniform(5),
            &theme(),
        );
        assert_eq!(buf.cell((5, 6)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((25, 8)).unwrap().symbol(), " ");
    }

    /// 中身は最後のパネルの枠が閉じるまで出ない。固定値で判定していると、ずらしを
    /// 増やしたときに枠より先に本文が読める。
    #[test]
    fn 中身はずらしを増やしても枠の完成を待つ() {
        let wide = Stagger::from_jitter(0.2, &[0.0; 4]);
        let closed = frames_closed_at(&wide);
        assert!(closed > FRAME_SHARE, "ずらしを増やしても待ち時間が伸びない");

        let mut buf = filled(AREA);
        paint(&mut buf, AREA, &panels(), closed - 0.05, &wide, &theme());
        assert_eq!(
            buf.cell((25, 8)).unwrap().symbol(),
            " ",
            "枠が閉じる前に中身が出ている"
        );
    }

    /// ひと呼吸なので、終わったあとに色が残ってはいけない。残ると起動のたびに画面が
    /// じわっと明るいままになる。
    #[test]
    fn ひと呼吸の光は終端で残らない() {
        let th = theme();
        let base = filled(AREA);

        let mut peak_seen = false;
        for step in 0..=20 {
            let p = f64::from(step) / 20.0;
            let mut buf = filled(AREA);
            apply_glow(&mut buf, AREA, p, 0.55, &th);
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
}
