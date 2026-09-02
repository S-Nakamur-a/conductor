//! vt100 の画面を ratatui の行にする。
//!
//! 描画のたびにパーサをロックして読む。キャッシュを持たせると「描画が次フレームの
//! 入力の前提を作る」形に戻るし、1 画面ぶんのセル複写は毎フレーム払える。

use std::sync::{Arc, Mutex};

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use conductor_svc::pty::{PtyStore, SessionKind};

use crate::layout::Region;
use crate::strip::visible_window;
use crate::workspace::Workspace;

#[cfg(test)]
use super::TerminalPanel;

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

/// スクロールバックを当てた画面を行に写す。実際に効いたオフセットも返す。
///
/// オルタネート画面 (ページャやエディタ) は自分でスクロールバックを持つので、
/// こちらのオフセットは当てない。
fn screen_lines(
    screen: &Arc<Mutex<vt100::Parser>>,
    scroll: usize,
    max_rows: u16,
    max_cols: u16,
) -> (Vec<Line<'static>>, usize) {
    let mut parser = lock(screen);
    let wanted = if parser.screen().alternate_screen() {
        0
    } else {
        scroll
    };
    parser.set_scrollback(wanted);
    let effective = parser.screen().scrollback();

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
    (lines, effective)
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
    let panel = &ws.panels.terminal;
    let pane = panel.pane(region);
    let sessions = panel.sessions(pane.kind);
    if sessions.is_empty() {
        let hint = match pane.kind {
            SessionKind::Shell => " no shell — ctrl+t to start one ",
            _ => " no Claude Code — ctrl+n to start one ",
        };
        return Line::styled(hint, Style::default().fg(theme.muted));
    }

    let selected = sessions
        .iter()
        .position(|(_, id, _)| Some(*id) == pane.session.as_deref())
        .unwrap_or(0);
    let labels: Vec<String> = sessions
        .iter()
        .map(|(_, _, label)| format!(" {label} "))
        .collect();
    let slots: Vec<u16> = labels
        .iter()
        .map(|l| UnicodeWidthStr::width(l.as_str()) as u16)
        .collect();
    let (start, end) = visible_window(&slots, 1, width, 0, selected, true);

    let mut spans = Vec::new();
    for (i, label) in labels.iter().enumerate().take(end).skip(start) {
        let style = if i == selected {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        spans.push(Span::styled(label.clone(), style));
    }
    Line::from(spans)
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
    let pane = panel.pane(region);
    let Some(index) = panel.index_of(pane.session.as_ref()) else {
        return;
    };
    let Some(screen) = panel.pty.screen(index) else {
        return;
    };

    let content = content_area(rect);
    let (lines, effective) = screen_lines(&screen, pane.scroll, content.height, content.width);
    let buffer = frame.buffer_mut();
    for (row, line) in lines.iter().enumerate().take(content.height as usize) {
        buffer.set_line(content.x, content.y + row as u16, line, content.width);
    }

    if effective > 0 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(" \u{2191} scrollback ({effective} lines — shift+end to return) "),
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
    let Some(index) = panel.index_of(pane.session.as_ref()) else {
        return String::new();
    };
    let Some(screen) = panel.pty.screen(index) else {
        return String::new();
    };
    let (lines, _) = screen_lines(&screen, pane.scroll, pane.size.0, pane.size.1);
    lines
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
        let (lines, effective) = screen_lines(&screen, 30, 5, 20);
        assert_eq!(effective, 30);
        assert!(lines[0].to_string().starts_with("line26"), "{lines:?}");

        // 読んだあとはライブに戻っている。次に読む側が過去を見せられては困る。
        assert_eq!(lock(&screen).screen().scrollback(), 0);
    }

    #[test]
    fn オルタネート画面ではスクロールバックを当てない() {
        let screen = Arc::new(Mutex::new(vt100::Parser::new(5, 20, 100)));
        for i in 0..60 {
            lock(&screen).process(format!("line{i}\r\n").as_bytes());
        }
        lock(&screen).process(b"\x1b[?1049h");
        let (_, effective) = screen_lines(&screen, 30, 5, 20);
        assert_eq!(effective, 0);
    }

    #[test]
    fn 内容領域は枠とタブ行を除く() {
        let content = content_area(Rect::new(10, 4, 40, 12));
        assert_eq!(content, Rect::new(11, 6, 38, 9));
    }
}
