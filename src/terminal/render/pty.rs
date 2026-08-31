//! vt100 PTY 画面のスナップショット取得と、キャッシュされた ratatui の Line への描画。

use std::sync::{Arc, Mutex};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// 毎フレーム高コストな vt100 スナップショットを取らずに済むよう、PTY の描画結果を
/// キャッシュしたもの。
///
/// ターミナルパネルがフォーカスされていないときは、vt100 パーサの mutex を
/// 再度ロックして数千セルをコピーする代わりに、以前構築した ratatui の Line
/// データを再利用する。
#[derive(Default)]
pub struct PtyRenderCache {
    pub lines: Vec<Line<'static>>,
    pub effective_offset: usize,
    /// vt100 パーサから得たカーソル位置（row, col）。IME の位置決めに使う。
    pub cursor_position: Option<(u16, u16)>,
}

/// vt100 の画面から抽出した、1セル分の内容とスタイルのスナップショット。
struct CellSnapshot {
    text: String,
    style: Style,
}

/// vt100 画面の内容のスナップショット。ロックを保持している間に取得することで、
/// （より低速な）ratatui の描画ステップの前にロックを解放できるようにしている。
struct ScreenSnapshot {
    rows: Vec<Vec<CellSnapshot>>,
    effective_offset: usize,
    /// vt100 パーサから得たカーソル位置（row, col）。
    cursor_position: (u16, u16),
}

/// リーダースレッドが mutex を持っている間ブロックしないよう try_lock を使う。競合したら
/// None を返すので、呼び出し側は前回の描画結果を使い回すこと。
fn snapshot_screen(
    screen_arc: &Arc<Mutex<vt100::Parser>>,
    scroll_offset: usize,
    max_rows: u16,
    max_cols: u16,
) -> Option<ScreenSnapshot> {
    let mut parser = match screen_arc.try_lock() {
        Ok(p) => p,
        Err(std::sync::TryLockError::WouldBlock) => return None,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
    };

    let is_alt_screen = parser.screen().alternate_screen();
    let requested_offset = if is_alt_screen { 0 } else { scroll_offset };

    parser.set_scrollback(requested_offset);
    // vt100 は内部で実際のスクロールバックバッファ長にクランプする。
    // キャッシュが実際の位置を反映するよう、実効オフセットを読み戻す。
    let effective_offset = parser.screen().scrollback();

    let screen = parser.screen();
    let (rows, cols) = screen.size();

    // デバッグ用: alternate screen の状態を定期的にログ出力する。
    if is_alt_screen {
        let has_content = (0..rows.min(5)).any(|r| {
            (0..cols).any(|c| {
                if let Some(cell) = screen.cell(r, c) {
                    let ch = cell.contents();
                    !ch.is_empty() && ch != " "
                } else {
                    false
                }
            })
        });
        let cursor = screen.cursor_position();
        log::debug!(
            "ALT_SCREEN render: has_content={has_content}, size=({rows},{cols}), area=({max_rows},{max_cols}) cursor=({},{})",
            cursor.0,
            cursor.1,
        );
    }

    // セルデータをローカルのスナップショットへ抽出する。
    let mut snapshot_rows: Vec<Vec<CellSnapshot>> = Vec::with_capacity(rows.min(max_rows) as usize);
    for row in 0..rows.min(max_rows) {
        let mut row_cells: Vec<CellSnapshot> = Vec::new();
        for col in 0..cols.min(max_cols) {
            let Some(cell) = screen.cell(row, col) else {
                break;
            };
            row_cells.push(CellSnapshot {
                text: cell.contents(),
                style: vt100_cell_to_style(cell),
            });
        }
        snapshot_rows.push(row_cells);
    }

    // スクロールバックを元に戻す前にカーソル位置を記録する。
    let cursor = screen.cursor_position();
    let cursor_position = (cursor.0, cursor.1);

    // 他の読み手が現在の画面を見られるよう、ライブビューへ戻す。
    parser.set_scrollback(0);

    // parser がスコープを抜けるここでロックが解放される。
    Some(ScreenSnapshot {
        rows: snapshot_rows,
        effective_offset,
        cursor_position,
    })
}

/// vt100 PTY 画面のスナップショットから ratatui の Line を構築する。
///
/// これが高コストな処理: vt100 パーサの mutex をロックし、セルデータを
/// コピーしてからスタイル付きの Line オブジェクトを構築する。結果は
/// [PtyRenderCache] にキャッシュしてフレームをまたいで再利用できる。
///
/// vt100 パーサの mutex を現在 PTY リーダースレッドが保持している場合は
/// None を返す。呼び出し側はメインスレッドをブロックする代わりに、
/// 以前のキャッシュを使い続けるべき。
pub fn build_pty_lines(
    screen_arc: &Arc<Mutex<vt100::Parser>>,
    scroll_offset: usize,
    max_rows: u16,
    max_cols: u16,
) -> Option<PtyRenderCache> {
    let snapshot = snapshot_screen(screen_arc, scroll_offset, max_rows, max_cols)?;
    let lines = lines_from_snapshot(&snapshot);
    let cursor_position = if snapshot.effective_offset == 0 {
        Some(snapshot.cursor_position)
    } else {
        None
    };
    Some(PtyRenderCache {
        lines,
        effective_offset: snapshot.effective_offset,
        cursor_position,
    })
}

/// [PtyRenderCache] から、以前構築した PTY の Line を描画する。
///
/// これは低コスト: キャッシュされた Line を参照のままフレームバッファへ
/// 直接転送するだけ。（以前は行ベクタ全体を clone() して Paragraph に
/// 渡していた — ターミナルフォーカス時の tick レートで毎フレーム2回、
/// すべての span の文字列を丸ごとディープコピーしていたが、得るものは
/// 何もなかった。）
pub fn render_pty_cached(frame: &mut Frame, area: Rect, cache: &PtyRenderCache, theme: &Theme) {
    // まずクリアする: スクロールバックしているときはスナップショットが
    // ライブビューより行数や幅が少なくなることがあり、そのまま行を転送すると
    // 覆われないセルに前フレームのテキストが残ってしまう
    // （「スクロールバックのにじみ」）。同じ理由でクリアしている viewer
    // パネルと同様の対処。
    frame.render_widget(ratatui::widgets::Clear, area);
    let buf = frame.buffer_mut();
    for (i, line) in cache.lines.iter().enumerate().take(area.height as usize) {
        buf.set_line(area.x, area.y + i as u16, line, area.width);
    }

    if cache.effective_offset > 0 {
        let indicator = Line::from(Span::styled(
            format!(
                " ↑ scrollback ({} lines — Shift+End to return) ",
                cache.effective_offset
            ),
            Style::default().fg(theme.selected_fg).bg(theme.accent),
        ));
        frame.render_widget(Paragraph::new(indicator), Rect { height: 1, ..area });
    }
}

/// ScreenSnapshot から Vec<Line<'static>> を構築する。
fn lines_from_snapshot(snapshot: &ScreenSnapshot) -> Vec<Line<'static>> {
    let mut text_lines: Vec<Line> = Vec::new();
    for row_cells in &snapshot.rows {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        let mut skip_cols: usize = 0;
        for cell in row_cells {
            if skip_cols > 0 {
                skip_cols -= 1;
                continue;
            }
            let ch = &cell.text;
            let style = cell.style;

            if style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
                current_style = style;
            }
            if ch.is_empty() {
                current_text.push(' ');
            } else {
                let w = UnicodeWidthStr::width(ch.as_str());
                if w > 1 {
                    skip_cols = w - 1;
                }
                current_text.push_str(ch);
            }
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }
        text_lines.push(Line::from(spans));
    }
    text_lines
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn vt100_cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();
    style = style.fg(vt100_color_to_ratatui(cell.fgcolor()));
    style = style.bg(vt100_color_to_ratatui(cell.bgcolor()));
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
