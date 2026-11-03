//! 区画の中で閉じるマウス選択と、離したときのクリップボードへの書き出し。
//!
//! 端末自身の選択は画面全体の矩形なので、隣の区画の文字が混ざる。マウスを捕まえて
//! いる間、端末は素のドラッグを選択にしない (Alacritty は Shift で端末側に戻る) ので、
//! ここで区画の枠の内側に切り詰めた選択を持ち、OSC 52 で端末のクリップボードへ渡す。
//! OSC 52 を黙って捨てる端末 (GNOME Terminal などの VTE) もあるので、手元に
//! クリップボードのコマンドがあればそちらにも渡す。

use std::io::Write;
use std::ops::RangeInclusive;
use std::process::{Command, Stdio};

use base64::Engine;
use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::layout::{Layout, Region};
use crate::rejoin::Break;

/// 画面座標で持つ。区画の中身が動いても選択は動かない — 端末の選択と同じ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub region: Region,
    anchor: (u16, u16),
    head: (u16, u16),
    /// 写す桁の範囲 (画面の絶対桁、両端含む)。
    columns: (u16, u16),
}

impl Selection {
    pub fn begin(region: Region, x: u16, y: u16) -> Self {
        Self {
            region,
            anchor: (x, y),
            head: (x, y),
            columns: (0, u16::MAX),
        }
    }

    /// 区画の端ではなくこの桁までを写す。押した瞬間に決めた値を持ち続けるので、
    /// ドラッグ中に区画の中身が動いても選択の意味は変わらない。
    pub fn within_columns(mut self, columns: (u16, u16)) -> Self {
        self.columns = columns;
        self
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
        if x < self.columns.0 || x > self.columns.1 {
            return false;
        }
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
        Region::TitleBar | Region::MenuBar | Region::StatusBar
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

/// 選択を写した文字。行の区切りを決める材料に、選んでいない行も区画の全行ぶん持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// 区画の上から順の各行の全体。
    pub rows: Vec<String>,
    /// 区画の最上行の画面の行。
    pub top: u16,
    /// `rows` の何行目から選んでいるか。
    pub first: usize,
    /// 選んだ各行の、選んだ部分。
    pub parts: Vec<String>,
}

impl Picked {
    /// 各行のあいだに `breaks[行]` を入れて繋ぐ。`breaks` は `rows` の境目ごと。
    pub fn join(&self, breaks: &[Break]) -> String {
        let between = |i: usize| breaks.get(self.first + i).copied();
        let last = self.parts.len().saturating_sub(1);
        let mut out = String::new();
        for (i, part) in self.parts.iter().enumerate() {
            let before = i.checked_sub(1).and_then(between);
            let after = (i < last).then(|| between(i)).flatten();
            let part = match before {
                Some(Break::Space | Break::Glue) => part.trim_start(),
                _ => part,
            };
            let part = match after {
                Some(Break::SoftWrap) => part,
                _ => part.trim_end(),
            };
            out.push_str(part);
            if i < last {
                out.push_str(match after {
                    Some(Break::Space) => " ",
                    Some(Break::Glue | Break::SoftWrap) => "",
                    Some(Break::Newline) | None => "\n",
                });
            }
        }
        out
    }

    /// 区切りが分からないときの写し方。見た目の行をそのまま改行で結ぶ。
    pub fn plain(&self) -> String {
        self.join(&[])
    }
}

/// 全角の字形は描いたときに次のセルが空白に潰されているので、幅の分だけ飛ばす。
pub fn pick(buffer: &Buffer, selection: &Selection, area: Rect) -> Picked {
    let row_text = |y: u16, picked: &dyn Fn(u16) -> bool| {
        let mut line = String::new();
        let mut skip = 0usize;
        for x in area.x..area.right() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if !picked(x) {
                continue;
            }
            let symbol = buffer[(x, y)].symbol();
            skip = symbol.width().saturating_sub(1);
            line.push_str(symbol);
        }
        line
    };
    let selected: Vec<u16> = selection
        .rows()
        .filter(|y| (area.y..area.bottom()).contains(y))
        .collect();
    Picked {
        rows: (area.y..area.bottom())
            .map(|y| row_text(y, &|_| true))
            .collect(),
        top: area.y,
        first: selected.first().map_or(0, |&y| usize::from(y - area.y)),
        parts: selected
            .iter()
            .map(|&y| row_text(y, &|x| selection.contains(x, y)))
            .collect(),
    }
}

/// 端末のクリップボードへ書く OSC 52。tmux や SSH 越しでも端末まで届く。
pub fn osc52(text: &str) -> Vec<u8> {
    let payload = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{payload}\x07").into_bytes()
}

/// 手元のクリップボードへ書くコマンドの候補。先に見つかったものを使う。
///
/// 表示サーバの無い環境 (SSH 越しなど) では空。そこは OSC 52 の受け持ち。
fn clipboard_commands(wayland: bool, x11: bool, macos: bool) -> Vec<&'static [&'static str]> {
    let mut commands: Vec<&'static [&'static str]> = Vec::new();
    if macos {
        commands.push(&["pbcopy"]);
    }
    if wayland {
        commands.push(&["wl-copy"]);
    }
    if x11 {
        commands.push(&["xclip", "-selection", "clipboard"]);
        commands.push(&["xsel", "--clipboard", "--input"]);
    }
    commands
}

/// 手元のクリップボードへ書く。xclip / xsel / wl-copy は選択を持ち続けるために
/// 自分を fork して残るので、待つのは別スレッドにして UI を止めない。
pub fn system_copy(text: &str) {
    let env_set = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    let commands = clipboard_commands(
        env_set("WAYLAND_DISPLAY"),
        env_set("DISPLAY"),
        cfg!(target_os = "macos"),
    );
    if commands.is_empty() {
        return;
    }
    let text = text.to_owned();
    std::thread::spawn(move || {
        for argv in commands {
            let child = Command::new(argv[0])
                .args(&argv[1..])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            let mut child = match child {
                Ok(child) => child,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    log::warn!("clipboard: {} failed to start: {e}", argv[0]);
                    continue;
                }
            };
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            match child.wait() {
                Ok(status) if status.success() => return,
                Ok(status) => log::warn!("clipboard: {} exited with {status}", argv[0]),
                Err(e) => log::warn!("clipboard: {} failed: {e}", argv[0]),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(buffer: &Buffer, selection: &Selection, area: Rect) -> String {
        pick(buffer, selection, area).plain()
    }

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

    fn picked_of(lines: &[&str], from: (u16, u16), to: (u16, u16)) -> Picked {
        let buffer = buffer(lines);
        let area = Rect::new(0, 0, 12, lines.len() as u16);
        let mut selection = Selection::begin(Region::TerminalClaude, from.0, from.1);
        selection.extend(to.0, to.1, area);
        pick(&buffer, &selection, area)
    }

    #[test]
    fn 語の切れ目の折り返しは字下げを落として空白で繋ぐ() {
        let picked = picked_of(&["⏺ one two", "  three"], (0, 0), (11, 1));
        assert_eq!(picked.join(&[Break::Space]), "⏺ one two three");
    }

    #[test]
    fn 語の途中の折り返しは区切りなしで繋ぐ() {
        let picked = picked_of(&["  https://ex", "  ample.com"], (2, 0), (11, 1));
        assert_eq!(picked.join(&[Break::Glue]), "https://example.com");
    }

    #[test]
    fn 端末の折り返しは行末の空白も中身として残す() {
        let picked = picked_of(&["echo a     ", "b"], (0, 0), (11, 1));
        assert_eq!(picked.join(&[Break::SoftWrap]), "echo a      b");
    }

    #[test]
    fn 区切りは選んでいない行も含めた区画の行の並びで引く() {
        let picked = picked_of(&["head", "one two", "  three", "tail"], (4, 1), (3, 2));
        assert_eq!(picked.first, 1);
        assert_eq!(
            picked.join(&[Break::Newline, Break::Space, Break::Newline]),
            "two th"
        );
    }

    #[test]
    fn 区切りが分からなければ見た目の行を改行で結ぶ() {
        let picked = picked_of(&["one two ", "  three"], (0, 0), (11, 1));
        assert_eq!(picked.plain(), "one two\n  three");
    }

    #[test]
    fn 空白だけを選ぶと空の文字になる() {
        let picked = picked_of(&["a    b"], (1, 0), (4, 0));
        assert_eq!(picked.plain(), "");
    }

    #[test]
    fn 空白だけの複数行は区切りどおりに繋ぐ() {
        let picked = picked_of(&["a", "", " c"], (1, 0), (0, 2));
        assert_eq!(picked.join(&[Break::Space, Break::Newline]), " \n");
        assert_eq!(picked.join(&[Break::Glue, Break::Glue]), "");
        let soft = picked.join(&[Break::SoftWrap, Break::SoftWrap]);
        assert!(
            !soft.is_empty() && soft.chars().all(|c| c == ' '),
            "{soft:?}"
        );
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
    fn 桁を絞った選択は行番号もつまみも写さない() {
        let buffer = buffer(&["1 │ alpha █", "2 │ bravo █"]);
        let area = Rect::new(0, 0, 12, 2);
        let mut selection = Selection::begin(Region::Viewer, 4, 0).within_columns((4, 8));
        selection.extend(11, 1, area);
        assert_eq!(text(&buffer, &selection, area), "alpha\nbravo");
    }

    #[test]
    fn 桁を絞った選択はガターへ引き戻しても本文で止まる() {
        let buffer = buffer(&["1 │ alpha █", "2 │ bravo █"]);
        let area = Rect::new(0, 0, 12, 2);
        let mut selection = Selection::begin(Region::Viewer, 8, 1).within_columns((4, 8));
        selection.extend(0, 0, area);
        assert_eq!(text(&buffer, &selection, area), "alpha\nbravo");
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
    fn 表示サーバが無ければクリップボードのコマンドを使わない() {
        assert!(clipboard_commands(false, false, false).is_empty());
    }

    #[test]
    fn x11ではxclipを先に試しxselに落ちる() {
        let commands = clipboard_commands(false, true, false);
        assert_eq!(commands[0][0], "xclip");
        assert_eq!(commands[1][0], "xsel");
    }

    #[test]
    fn waylandではwl_copyを先に試す() {
        let commands = clipboard_commands(true, true, false);
        assert_eq!(commands[0][0], "wl-copy");
    }

    #[test]
    fn osc52は本文をbase64で包む() {
        assert_eq!(osc52("hi"), b"\x1b]52;c;aGk=\x07");
    }
}
