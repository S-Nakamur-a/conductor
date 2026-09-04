//! vt100 の画面を ratatui の行にする。
//!
//! 描画のたびにパーサをロックして読む。キャッシュを持たせると「描画が次フレームの
//! 入力の前提を作る」形に戻るし、1 画面ぶんのセル複写は毎フレーム払える。

use std::sync::{Arc, Mutex};

use ratatui::Frame;
use ratatui::layout::{Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use conductor_svc::pty::PtyStore;

use super::focus_of;
use super::tabs::SlotKind;
use crate::layout::Region;
use crate::workspace::{Focus, Workspace};

#[cfg(test)]
use super::TerminalPanel;

/// 枠だけを除いた PTY のグリッド。エディタはセッションタブを持たない。
pub fn editor_area(panel: Rect) -> Rect {
    panel.inner(Margin::new(1, 1))
}

/// セッションタブの 1 行。
pub fn tab_area(panel: Rect) -> Rect {
    let inner = panel.inner(Margin::new(1, 1));
    Rect {
        height: inner.height.min(1),
        ..inner
    }
}

/// 枠とタブ行を除いた PTY のグリッド。リサイズと描画が同じ計算を見る。
pub fn content_area(panel: Rect) -> Rect {
    let inner = panel.inner(Margin::new(1, 1));
    Rect {
        y: inner.y.saturating_add(1),
        height: inner.height.saturating_sub(1),
        ..inner
    }
}

fn lock(parser: &Mutex<vt100::Parser>) -> std::sync::MutexGuard<'_, vt100::Parser> {
    parser.lock().unwrap_or_else(|e| e.into_inner())
}

/// want に一番近い、vt100 に実際に効くオフセット。バッファ長はパーサしか知らない。
pub(super) fn clamp_scrollback(pty: &PtyStore, index: usize, want: usize) -> usize {
    let Some(screen) = pty.screen(index) else {
        return 0;
    };
    let mut parser = lock(&screen);
    parser.set_scrollback(want);
    let effective = parser.screen().scrollback();
    parser.set_scrollback(0);
    effective
}

/// 画面 1 枚の写し。
struct Snapshot {
    lines: Vec<Line<'static>>,
    /// 実際に効いたスクロールバックのオフセット。
    effective: usize,
    /// 内容領域からの相対で (行, 桁)。遡って読んでいる間は生きたカーソルの位置が
    /// 画面の内容と合わないので入らない。
    cursor: Option<(u16, u16)>,
}

/// スクロールバックを当てた画面を写す。
///
/// オルタネート画面 (ページャやエディタ) は自分でスクロールバックを持つので、
/// こちらのオフセットは当てない。
fn snapshot_screen(
    screen: &Arc<Mutex<vt100::Parser>>,
    scroll: usize,
    max_rows: u16,
    max_cols: u16,
) -> Snapshot {
    let mut parser = lock(screen);
    let wanted = if parser.screen().alternate_screen() {
        0
    } else {
        scroll
    };
    parser.set_scrollback(wanted);
    let effective = parser.screen().scrollback();
    let cursor = (effective == 0).then(|| parser.screen().cursor_position());

    let lines = {
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let mut lines = Vec::with_capacity(rows.min(max_rows) as usize);
        for row in 0..rows.min(max_rows) {
            let mut spans: Vec<Span> = Vec::new();
            let mut text = String::new();
            let mut style = Style::default();
            let mut skip = 0usize;
            for col in 0..cols.min(max_cols) {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let Some(cell) = screen.cell(row, col) else {
                    break;
                };
                let cell_style = cell_style(cell);
                if cell_style != style && !text.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut text), style));
                }
                style = cell_style;
                let contents = cell.contents();
                if contents.is_empty() {
                    text.push(' ');
                } else {
                    skip = UnicodeWidthStr::width(contents.as_str()).saturating_sub(1);
                    text.push_str(&contents);
                }
            }
            if !text.is_empty() {
                spans.push(Span::styled(text, style));
            }
            lines.push(Line::from(spans));
        }
        lines
    };

    parser.set_scrollback(0);
    Snapshot {
        lines,
        effective,
        cursor,
    }
}

/// 端末のカーソルをその区画へ置く。IME の変換窓はこれを見て出る場所を決めるので、
/// キーを取っているのがモーダルやメニューのときは置かない。
fn place_cursor(frame: &mut Frame, ws: &Workspace, content: Rect, snapshot: &Snapshot) {
    if !ws.modals.is_empty() || ws.chrome.menu.open_index().is_some() {
        return;
    }
    let Some((row, col)) = snapshot.cursor else {
        return;
    };
    if row < content.height && col < content.width {
        frame.set_cursor_position(Position::new(content.x + col, content.y + row));
    }
}

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default()
        .fg(color(cell.fgcolor()))
        .bg(color(cell.bgcolor()));
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

/// セッションタブ。枠の内側の 1 行目に置く。
fn tab_row(ws: &Workspace, region: Region, width: u16) -> Line<'static> {
    let theme = &ws.theme;
    let spans = ws
        .panels
        .terminal
        .tab_slots(region, width)
        .into_iter()
        .map(|slot| {
            let style = match slot.kind {
                SlotKind::Tab { selected: true, .. } => Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD),
                SlotKind::Tab { .. } | SlotKind::Hint => Style::default().fg(theme.muted),
                // 選んでいるタブの塗りつぶしの中には入れない。アクセント背景に赤を
                // 乗せるとコントラストが落ちて、危険な操作が読めなくなる。
                SlotKind::Close { .. } => Style::default().fg(theme.error),
                SlotKind::Add => Style::default().fg(theme.accent),
            };
            Span::styled(slot.label, style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// 埋め込みエディタの中身。タブ行もスクロールバックも無く、PTY のグリッドだけ。
pub fn editor(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let panel = &ws.panels.terminal;
    let content = editor_area(rect);
    if content.width == 0 || content.height == 0 {
        return;
    }
    let Some(index) = panel.index_of(panel.editor.as_ref().map(|e| e.session.as_str())) else {
        return;
    };
    let Some(screen) = panel.pty.screen(index) else {
        return;
    };
    let snapshot = snapshot_screen(&screen, 0, content.height, content.width);
    let buffer = frame.buffer_mut();
    for (row, line) in snapshot.lines.iter().enumerate() {
        buffer.set_line(content.x, content.y + row as u16, line, content.width);
    }
    if ws.focus == Focus::Editor {
        place_cursor(frame, ws, content, &snapshot);
    }
}

/// 枠の内側 (タブ行 + PTY のグリッド) を描く。枠そのものは [crate::render] が描く。
pub fn pane(frame: &mut Frame, rect: Rect, ws: &Workspace, region: Region) {
    let inner = rect.inner(Margin::new(1, 1));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(tab_row(ws, region, inner.width)),
        Rect { height: 1, ..inner },
    );

    let panel = &ws.panels.terminal;
    // トランスクリプトは Claude 区画の中身を置き換える。タブ行は残す —
    // どのセッションの履歴を読んでいるのかが分からなくなる。
    if let Some(reflow) = panel
        .transcript()
        .filter(|_| region == Region::TerminalClaude)
    {
        super::reflow::render(frame, content_area(rect), reflow);
        return;
    }

    let pane = panel.pane(region);
    let Some(index) = panel.index_of(pane.session.as_deref()) else {
        return;
    };
    let Some(screen) = panel.pty.screen(index) else {
        return;
    };

    let content = content_area(rect);
    let snapshot = snapshot_screen(&screen, pane.scroll, content.height, content.width);
    let buffer = frame.buffer_mut();
    for (row, line) in snapshot
        .lines
        .iter()
        .enumerate()
        .take(content.height as usize)
    {
        buffer.set_line(content.x, content.y + row as u16, line, content.width);
    }
    if ws.focus == focus_of(region) {
        place_cursor(frame, ws, content, &snapshot);
    }

    if snapshot.effective > 0 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(
                    " \u{2191} scrollback ({} lines — shift+end to return) ",
                    snapshot.effective
                ),
                Style::default()
                    .fg(ws.theme.selected_fg)
                    .bg(ws.theme.accent),
            )),
            Rect {
                height: 1,
                ..content
            },
        );
    }
}

/// テスト用: いま画面に見えている文字だけを取り出す。
#[cfg(test)]
pub(super) fn visible_text(panel: &TerminalPanel, region: Region) -> String {
    let pane = panel.pane(region);
    let Some(index) = panel.index_of(pane.session.as_deref()) else {
        return String::new();
    };
    let Some(screen) = panel.pty.screen(index) else {
        return String::new();
    };
    let snapshot = snapshot_screen(&screen, pane.scroll, pane.size.0, pane.size.1);
    snapshot
        .lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cargo.toml で vt100 の overflow-checks を外している設定が効いているかの番人。
    #[test]
    fn 一画面より深いスクロールバックも読める() {
        let screen = Arc::new(Mutex::new(vt100::Parser::new(5, 20, 100)));
        for i in 0..60 {
            lock(&screen).process(format!("line{i}\r\n").as_bytes());
        }
        let snapshot = snapshot_screen(&screen, 30, 5, 20);
        assert_eq!(snapshot.effective, 30);
        assert!(
            snapshot.lines[0].to_string().starts_with("line26"),
            "{:?}",
            snapshot.lines
        );
        assert_eq!(
            snapshot.cursor, None,
            "遡っている間は生きたカーソルを出さない"
        );

        // 読んだあとはライブに戻っている。次に読む側が過去を見せられては困る。
        assert_eq!(lock(&screen).screen().scrollback(), 0);
    }

    #[test]
    fn ライブ表示ではカーソルの位置を返す() {
        let screen = Arc::new(Mutex::new(vt100::Parser::new(5, 20, 100)));
        lock(&screen).process(b"line0\r\nab");
        assert_eq!(snapshot_screen(&screen, 0, 5, 20).cursor, Some((1, 2)));
    }

    #[test]
    fn オルタネート画面ではスクロールバックを当てない() {
        let screen = Arc::new(Mutex::new(vt100::Parser::new(5, 20, 100)));
        for i in 0..60 {
            lock(&screen).process(format!("line{i}\r\n").as_bytes());
        }
        lock(&screen).process(b"\x1b[?1049h");
        let snapshot = snapshot_screen(&screen, 30, 5, 20);
        assert_eq!(snapshot.effective, 0);
    }

    #[test]
    fn 内容領域は枠とタブ行を除く() {
        let content = content_area(Rect::new(10, 4, 40, 12));
        assert_eq!(content, Rect::new(11, 6, 38, 9));
    }
}
