//! 光が枠の上を回り続ける。進捗の出せない仕事が「生きている」ことだけを伝える。

use conductor_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// 光が 1 セル進むのにかける時間。周期ではなく速さで持つので、枠の大きさが違っても
/// 同じ速さで回る。
const MS_PER_CELL: f64 = 18.0;
/// 尾の長さ (セル)。
const TAIL: usize = 12;

/// 枠の上だけを回り、文字は色を寄せるにとどめる。数分続くので本文を 1 文字も隠さない。
pub(super) fn paint(buf: &mut Buffer, panel: Rect, elapsed_ms: f64, theme: &Theme) {
    if panel.width < 3 || panel.height < 3 {
        return;
    }
    let ring = ring(panel);
    let head = (elapsed_ms / MS_PER_CELL) as usize % ring.len();
    for k in 0..=TAIL.min(ring.len() - 1) {
        let (x, y) = ring[(head + ring.len() - k) % ring.len()];
        let Some(cell) = buf.cell_mut((x, y)) else {
            continue;
        };
        let color = if k == 0 {
            theme.fg
        } else {
            Theme::lerp(theme.accent, cell.fg, k as f64 / TAIL as f64)
        };
        cell.set_fg(color);
    }
}

/// 枠のセルを左上から時計回りに。
fn ring(panel: Rect) -> Vec<(u16, u16)> {
    let (x2, y2) = (panel.right() - 1, panel.bottom() - 1);
    let mut cells = Vec::new();
    cells.extend((panel.x..=x2).map(|x| (x, panel.y)));
    cells.extend((panel.y + 1..=y2).map(|y| (x2, y)));
    cells.extend((panel.x..x2).rev().map(|x| (x, y2)));
    cells.extend((panel.y + 1..y2).rev().map(|y| (panel.x, y)));
    cells
}

#[cfg(test)]
mod tests {
    use super::super::tests::{AREA, filled, theme};
    use super::*;

    const PANEL: Rect = Rect::new(17, 3, 24, 11);

    fn changed(buf: &Buffer) -> Vec<(u16, u16)> {
        let before = filled(AREA);
        let mut out = Vec::new();
        for y in AREA.top()..AREA.bottom() {
            for x in AREA.left()..AREA.right() {
                if buf.cell((x, y)) != before.cell((x, y)) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn 光は枠の上だけを通る() {
        for step in 0..40 {
            let mut buf = filled(AREA);
            paint(
                &mut buf,
                PANEL,
                f64::from(step) * MS_PER_CELL * 3.0,
                &theme(),
            );
            for (x, y) in changed(&buf) {
                let on_edge = x == PANEL.x
                    || x == PANEL.right() - 1
                    || y == PANEL.y
                    || y == PANEL.bottom() - 1;
                assert!(on_edge, "内側 ({x},{y}) が動いた");
            }
        }
    }

    /// 枠を埋め尽くすと、点滅する枠にしか見えない。
    #[test]
    fn 点くのは尾の長さまで() {
        let mut buf = filled(AREA);
        paint(&mut buf, PANEL, 1000.0, &theme());
        assert!(changed(&buf).len() <= TAIL + 1);
    }

    #[test]
    fn 光は時間で進む() {
        let mut a = filled(AREA);
        let mut b = filled(AREA);
        paint(&mut a, PANEL, 0.0, &theme());
        paint(&mut b, PANEL, MS_PER_CELL * 5.0, &theme());
        assert_ne!(changed(&a), changed(&b));
    }
}
