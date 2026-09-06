//! 四隅から短い腕が伸び縮みする。進捗の出せない仕事が「生きている」ことだけを伝える。

use conductor_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

pub(super) const CYCLE_MS: f64 = 2400.0;
/// 腕の最大の長さ (セル)。
const ARM: u16 = 3;

/// 枠の上だけに描く。数分続くので、本文を 1 文字も隠さない。
pub(super) fn paint(buf: &mut Buffer, panel: Rect, phase: f64, theme: &Theme) {
    if panel.width < 3 || panel.height < 3 {
        return;
    }
    let (x2, y2) = (panel.right() - 1, panel.bottom() - 1);
    let corners = [(panel.x, panel.y), (x2, panel.y), (panel.x, y2), (x2, y2)];
    for (i, &(cx, cy)) in corners.iter().enumerate() {
        let lit = ((phase * std::f64::consts::TAU - i as f64 * 0.5).sin() + 1.0) / 2.0;
        if let Some(cell) = buf.cell_mut((cx, cy)) {
            cell.set_fg(Theme::lerp(cell.fg, theme.fg, lit));
        }
        let arm = (lit * f64::from(ARM)).round() as u16;
        for k in 1..=arm {
            let t = f64::from(k) / f64::from(ARM);
            let hx = if cx == panel.x { cx + k } else { cx - k };
            if hx > panel.x && hx < x2 {
                thicken(buf, (hx, cy), "\u{2500}", "\u{2501}", theme, t);
            }
            let hy = if cy == panel.y { cy + k } else { cy - k };
            if hy > panel.y && hy < y2 {
                thicken(buf, (cx, hy), "\u{2502}", "\u{2503}", theme, t);
            }
        }
    }
}

fn thicken(buf: &mut Buffer, at: (u16, u16), light: &str, heavy: &str, theme: &Theme, t: f64) {
    let Some(cell) = buf.cell_mut(at) else {
        return;
    };
    if cell.symbol() == light {
        cell.set_symbol(heavy);
    }
    cell.set_fg(Theme::lerp(theme.accent, cell.fg, t));
}

#[cfg(test)]
mod tests {
    use super::super::tests::{AREA, filled, theme};
    use super::*;

    const PANEL: Rect = Rect::new(17, 3, 24, 11);

    #[test]
    fn 呼吸は枠だけに触る() {
        let before = filled(AREA);
        for step in 0..20 {
            let mut buf = filled(AREA);
            paint(&mut buf, PANEL, f64::from(step) / 20.0, &theme());
            for y in PANEL.y + 1..PANEL.bottom() - 1 {
                for x in PANEL.x + 1..PANEL.right() - 1 {
                    assert_eq!(
                        buf.cell((x, y)),
                        before.cell((x, y)),
                        "内側 ({x},{y}) が動いた"
                    );
                }
            }
        }
    }

    /// 腕が伸びきると辺の途中まで太線になり、進捗バーと見分けがつかなくなる。
    #[test]
    fn 腕は上限を超えて伸びない() {
        for step in 0..20 {
            let mut buf = filled(AREA);
            for x in PANEL.x + 1..PANEL.right() - 1 {
                buf.cell_mut((x, PANEL.y)).unwrap().set_symbol("\u{2500}");
            }
            paint(&mut buf, PANEL, f64::from(step) / 20.0, &theme());
            let heavy = (PANEL.x + 1..PANEL.right() - 1)
                .filter(|x| buf.cell((*x, PANEL.y)).unwrap().symbol() == "\u{2501}")
                .count();
            assert!(
                heavy <= 2 * usize::from(ARM),
                "step={step}: 太線が {heavy} セル"
            );
        }
    }

    #[test]
    fn 罫線でないセルは記号を変えない() {
        let mut buf = filled(AREA);
        paint(&mut buf, PANEL, 0.25, &theme());
        assert_eq!(buf.cell((PANEL.x + 1, PANEL.y)).unwrap().symbol(), "x");
    }
}
