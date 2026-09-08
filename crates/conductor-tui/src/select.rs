//! 区画の中で閉じるマウス選択と、離したときのクリップボードへの書き出し。
//!
//! 端末自身の選択は画面全体の矩形なので、隣の区画の文字が混ざる。マウスを捕まえて
//! いる間、端末は素のドラッグを選択にしない (Alacritty は Shift で端末側に戻る) ので、
//! ここで区画の枠の内側に切り詰めた選択を持ち、OSC 52 で端末のクリップボードへ渡す。

use std::ops::RangeInclusive;

use base64::Engine;
use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::layout::{Layout, Region};

/// 画面座標で持つ。区画の中身が動いても選択は動かない — 端末の選択と同じ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub region: Region,
    anchor: (u16, u16),
    head: (u16, u16),
}

impl Selection {
    pub fn begin(region: Region, x: u16, y: u16) -> Self {
        Self {
            region,
            anchor: (x, y),
            head: (x, y),
        }
    }

    pub fn extend(&mut self, x: u16, y: u16, area: Rect) {
        let clamp = |v: u16, lo: u16, len: u16| v.clamp(lo, lo + len.saturating_sub(1));
        self.head = (clamp(x, area.x, area.width), clamp(y, area.y, area.height));
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let reading_order = |(x, y): (u16, u16)| (y, x);
        if reading_order(self.anchor) <= reading_order(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn contains(&self, x: u16, y: u16) -> bool {
        let ((x0, y0), (x1, y1)) = self.ordered();
        (y0..=y1).contains(&y) && (y > y0 || x >= x0) && (y < y1 || x <= x1)
    }

    fn rows(&self) -> RangeInclusive<u16> {
        let ((_, y0), (_, y1)) = self.ordered();
        y0..=y1
    }
}

/// 選択できる範囲。枠線を含めると罫線が写るので、その内側。
pub fn area(layout: &Layout, region: Region) -> Option<Rect> {
    let inside_panel = !matches!(
        region,
        Region::TitleBar | Region::MenuBar | Region::WorktreeStrip | Region::StatusBar
    );
    inside_panel
        .then(|| layout.rect(region))?
        .map(|rect| rect.inner(Margin::new(1, 1)))
}

pub fn highlight(buffer: &mut Buffer, selection: &Selection, area: Rect, style: Style) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if selection.contains(x, y) {
                buffer[(x, y)].set_style(style);
            }
        }
    }
}

/// 全角の字形は描いたときに次のセルが空白に潰されているので、幅の分だけ飛ばす。
pub fn text(buffer: &Buffer, selection: &Selection, area: Rect) -> String {
    let mut lines = Vec::new();
    for y in selection.rows() {
        if !(area.y..area.bottom()).contains(&y) {
            continue;
        }
        let mut line = String::new();
        let mut skip = 0usize;
        for x in area.x..area.right() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if !selection.contains(x, y) {
                continue;
            }
            let symbol = buffer[(x, y)].symbol();
            skip = symbol.width().saturating_sub(1);
            line.push_str(symbol);
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// 端末のクリップボードへ書く OSC 52。tmux や SSH 越しでも端末まで届く。
pub fn osc52(text: &str) -> Vec<u8> {
    let payload = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{payload}\x07").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(lines: &[&str]) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, lines.len() as u16));
        for (y, line) in lines.iter().enumerate() {
            buffer.set_string(0, y as u16, line, Style::default());
        }
        buffer
    }

    #[test]
    fn 行をまたぐ選択は最初の行の右端まで最後の行の左端からを含む() {
        let buffer = buffer(&["abcdef", "ghijkl", "mnopqr"]);
        let area = Rect::new(0, 0, 12, 3);
        let mut selection = Selection::begin(Region::Viewer, 3, 0);
        selection.extend(1, 2, area);
        assert_eq!(text(&buffer, &selection, area), "def\nghijkl\nmn");
    }

    #[test]
    fn 上へ向かうドラッグも読む順に並べる() {
        let buffer = buffer(&["abcdef", "ghijkl"]);
        let area = Rect::new(0, 0, 12, 2);
        let mut selection = Selection::begin(Region::Viewer, 2, 1);
        selection.extend(4, 0, area);
        assert_eq!(text(&buffer, &selection, area), "ef\nghi");
    }

    #[test]
    fn 全角の字形は潰れたセルを拾わない() {
        let buffer = buffer(&["ねこ x"]);
        let area = Rect::new(0, 0, 12, 1);
        let mut selection = Selection::begin(Region::Viewer, 0, 0);
        selection.extend(5, 0, area);
        assert_eq!(text(&buffer, &selection, area), "ねこ x");
    }

    #[test]
    fn 区画の外へ出たドラッグは縁で止まる() {
        let area = Rect::new(5, 5, 10, 4);
        let mut selection = Selection::begin(Region::Viewer, 6, 6);
        selection.extend(40, 40, area);
        assert!(selection.contains(14, 8));
        assert!(!selection.contains(15, 8));
        assert!(!selection.contains(14, 9));
    }

    #[test]
    fn 帯は選べない() {
        let ws = crate::workspace::Workspace::for_test();
        let layout = crate::layout::layout(&ws, Rect::new(0, 0, 120, 40));
        assert_eq!(area(&layout, Region::StatusBar), None);
        let viewer = layout.rect(Region::Viewer).unwrap();
        assert_eq!(
            area(&layout, Region::Viewer),
            Some(viewer.inner(Margin::new(1, 1)))
        );
    }

    #[test]
    fn osc52は本文をbase64で包む() {
        assert_eq!(osc52("hi"), b"\x1b]52;c;aGk=\x07");
    }
}
