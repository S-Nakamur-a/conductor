//! 帯が上から下へ 1 回通り抜ける。「始まった」の合図。

use conductor_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ease, raise};

pub(super) const DURATION_MS: u64 = 320;
/// 先端の後ろに引く残光の行数。
const TRAIL: u16 = 5;
/// 先端で文字色を持ち上げる量。1.0 は実機で眩しすぎた。
const PEAK: f64 = 0.5;

/// 前景しか動かせないので、空白の多いパネルでは先端に線を置かないと何も見えない。
pub(super) fn paint(buf: &mut Buffer, panel: Rect, progress: f64, theme: &Theme) {
    if progress >= 1.0 || panel.width < 3 || panel.height < 3 {
        return;
    }
    let inner = Rect::new(panel.x + 1, panel.y + 1, panel.width - 2, panel.height - 2);
    let head = ease(progress) * f64::from(inner.height + TRAIL);
    for row in 0..inner.height {
        let behind = head - f64::from(row);
        if !(0.0..f64::from(TRAIL)).contains(&behind) {
            continue;
        }
        let k = 1.0 - behind / f64::from(TRAIL);
        for x in inner.x..inner.right() {
            let Some(cell) = buf.cell_mut((x, inner.y + row)) else {
                continue;
            };
            if behind < 1.0 && cell.symbol().trim().is_empty() {
                cell.set_symbol("\u{2500}");
                cell.set_fg(theme.accent);
            } else {
                cell.set_fg(raise(theme, cell.fg, k * PEAK));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{AREA, filled, theme};
    use super::*;

    const PANEL: Rect = Rect::new(17, 3, 24, 11);

    #[test]
    fn 帯は枠に触らず内側だけを通る() {
        for step in 0..20 {
            let mut buf = filled(AREA);
            let before = filled(AREA);
            paint(&mut buf, PANEL, f64::from(step) / 20.0, &theme());
            for x in PANEL.x..PANEL.right() {
                for y in [PANEL.y, PANEL.bottom() - 1] {
                    assert_eq!(
                        buf.cell((x, y)),
                        before.cell((x, y)),
                        "枠 ({x},{y}) が動いた"
                    );
                }
            }
            for y in PANEL.y..PANEL.bottom() {
                for x in [PANEL.x, PANEL.right() - 1] {
                    assert_eq!(
                        buf.cell((x, y)),
                        before.cell((x, y)),
                        "枠 ({x},{y}) が動いた"
                    );
                }
            }
        }
    }

    /// 通り過ぎたあとに色が残ると、帯ではなく「明るくなった」に見える。
    #[test]
    fn 通り過ぎた行と終端は元の色に戻る() {
        let before = filled(AREA);
        let mut seen = false;
        for step in 0..=20 {
            let mut buf = filled(AREA);
            paint(&mut buf, PANEL, f64::from(step) / 20.0, &theme());
            let probe = (25, 5);
            let changed = buf.cell(probe) != before.cell(probe);
            seen |= changed;
            if step == 20 {
                assert!(!changed, "終端で色が残っている");
            }
        }
        assert!(seen, "帯が一度も通らなかった");
    }

    #[test]
    fn 空のセルには先端の線が置かれる() {
        let mut buf = Buffer::empty(AREA);
        // ease(0.5) = 0.5 なので先端は内側の 7 行目あたりに居る。
        paint(&mut buf, PANEL, 0.5, &theme());
        let lined = (PANEL.y + 1..PANEL.bottom() - 1)
            .filter(|y| buf.cell((25, *y)).unwrap().symbol() == "\u{2500}")
            .count();
        assert_eq!(lined, 1, "線は先端の 1 行だけ");
    }
}
