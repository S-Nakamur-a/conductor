//! Claude Code transcript フレーバー向けの GFM テーブルレイアウト: 罫線文字の
//! グリッド（┌─┬─┐ / ├─┼─┤ / └─┴─┘）で、実物の Claude Code のデフォルトの
//! テーブル描画に合わせる — 各列は max(セル幅) + 2 にパディングし、
//! （ヘッダー下だけでなく）行と行の間すべてに区切り線を入れ、セルは左寄せ、
//! 色も太字もどこにも付けない（実物の出力はテーブルに SGR を一切含まない）。
//!
//! super::table（Rich フレーバーの罫線なしレイアウト）とは別モジュールにしていて、
//! その中の分岐にしていないのは、両者がレイアウトのコードをほとんど共有しない
//! ため — 枠線行、行ごとの区切り線配置、強制左寄せは Rich フレーバー側に
//! 対応物がない。1つの関数に畳み込むと、無関係なロジックを
//! if flavor == Transcript { .. } else { .. } で包んだだけのものになってしまう。

use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::theme::Theme;

use super::MarkdownFlavor;
use super::inline::inline_spans;
use super::wrap::{Cell, cells_to_line, spans_to_cells, wrap_cells_raw};

/// GFM テーブルを枠線付きのボックスとして描画する。列のアライメントヒント
/// (:--/--:/:-:) は無視する — 実物の Claude Code はソースのデリミタ行に
/// 関わらず、常にセルの内容を左寄せにする。
pub(crate) fn render_table_boxed(
    headers: &[String],
    rows: &[Vec<String>],
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let ncols = headers.len();
    if ncols == 0 {
        return vec![Line::from("")];
    }

    // Rich テーブルと同様に、本体行をちょうど ncols 列に正規化する。
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut row: Vec<String> = r.iter().take(ncols).cloned().collect();
            row.resize(ncols, String::new());
            row
        })
        .collect();

    let inner = fit_inner_widths(&natural_inner_widths(headers, &rows, theme), width);
    let style = Style::default().fg(Color::Reset);

    let mut out = Vec::with_capacity(rows.len() * 2 + 3);
    out.push(clip_line(
        border_cells(&inner, '\u{250c}', '\u{252c}', '\u{2510}', style),
        width,
    ));
    out.extend(render_row(headers, &inner, theme, style, width));
    out.push(clip_line(
        border_cells(&inner, '\u{251c}', '\u{253c}', '\u{2524}', style),
        width,
    ));
    for (idx, row) in rows.iter().enumerate() {
        out.extend(render_row(row, &inner, theme, style, width));
        if idx + 1 < rows.len() {
            out.push(clip_line(
                border_cells(&inner, '\u{251c}', '\u{253c}', '\u{2524}', style),
                width,
            ));
        }
    }
    out.push(clip_line(
        border_cells(&inner, '\u{2514}', '\u{2534}', '\u{2518}', style),
        width,
    ));
    out
}

/// 各列の自然な内側幅: ヘッダーとすべての本体セルにわたる
/// max(セルの表示幅) + 2（左右に1列ずつのパディング）。
fn natural_inner_widths(headers: &[String], rows: &[Vec<String>], theme: &Theme) -> Vec<usize> {
    let mut w: Vec<usize> = headers
        .iter()
        .map(|h| rendered_width(h, theme) + 2)
        .collect();
    for row in rows {
        for (k, cell) in row.iter().enumerate() {
            if let Some(col) = w.get_mut(k) {
                *col = (*col).max(rendered_width(cell, theme) + 2);
            }
        }
    }
    w
}

/// インラインマークアップを取り除いた後のテキストの表示幅。
fn rendered_width(text: &str, theme: &Theme) -> usize {
    cells_width(&spans_to_cells(&inline_spans(
        text,
        Style::default(),
        theme,
        MarkdownFlavor::Transcript,
    )))
}

/// グリッド全体（列 + 境界ごとに1文字の枠線、合計 ncols + 1 個）が width に
/// 収まるよう、自然な内側幅を縮める。super::table::fit_col_widths の
/// 「最も広い列を1ずつ削る」やり方を踏襲する。どの列も最低1列は確保する。
fn fit_inner_widths(natural: &[usize], width: usize) -> Vec<usize> {
    let ncols = natural.len();
    if ncols == 0 {
        return vec![];
    }
    let overhead = ncols + 1; // 縦の枠線文字: 列境界ごとに1個
    let avail = width.saturating_sub(overhead).max(ncols); // 各列最低1
    let mut w: Vec<usize> = natural.iter().map(|&x| x.max(1).min(avail)).collect();
    while w.iter().sum::<usize>() > avail {
        let maxw = *w.iter().max().unwrap();
        if maxw <= 1 {
            break; // これ以上は縮められない。最後のクリップが幅の上限を保証する
        }
        let idx = w.iter().position(|&x| x == maxw).unwrap();
        w[idx] -= 1;
    }
    w
}

/// テーブルの行1本を、一番背の高いセルに必要な数だけの Line に描画する
/// （列に収まらないセルは切り詰めずに折り返す。Rich テーブルと同じ方針）。
fn render_row(
    cells: &[String],
    inner: &[usize],
    theme: &Theme,
    style: Style,
    width: usize,
) -> Vec<Line<'static>> {
    let cols: Vec<Vec<Vec<Cell>>> = inner
        .iter()
        .enumerate()
        .map(|(k, &w)| {
            let text = cells.get(k).map(String::as_str).unwrap_or("");
            wrap_cell(text, w, style, theme)
        })
        .collect();

    let height = cols.iter().map(Vec::len).max().unwrap_or(1);
    let blank = |w: usize| -> Vec<Cell> { (0..w).map(|_| Cell::new(' ', style)).collect() };

    (0..height)
        .map(|row_line| {
            let mut row: Vec<Cell> = vec![Cell::new('\u{2502}', style)];
            for (k, col) in cols.iter().enumerate() {
                match col.get(row_line) {
                    Some(line) => row.extend(line.iter().cloned()),
                    None => row.extend(blank(inner[k])),
                }
                row.push(Cell::new('\u{2502}', style));
            }
            clip_line(row, width)
        })
        .collect()
}

/// テキストを、ちょうど inner_w 桁の表示幅の行に折り返す: 色・強調以外の
/// インラインマークアップのスタイルを取り除き、inner_w - 2 の内容領域の中で
/// 単語境界で折り返し、左右に1スペースずつのパディングを付けて左寄せする。
/// 常に最低1行を返す。
fn wrap_cell(text: &str, inner_w: usize, style: Style, theme: &Theme) -> Vec<Vec<Cell>> {
    if inner_w == 0 {
        return vec![Vec::new()];
    }
    let content_w = inner_w.saturating_sub(2).max(1);
    let cells = spans_to_cells(&inline_spans(
        text,
        style,
        theme,
        MarkdownFlavor::Transcript,
    ));
    wrap_cells_raw(&cells, content_w, false)
        .into_iter()
        .map(|line| pad_left(line, inner_w, style))
        .collect()
}

/// 折り返し済みの内容行1本を inner_w 桁までパディングする: 先頭にスペース1つ、
/// 内容、その後 inner_w - 1 桁までスペースを続ける。
fn pad_left(cells: Vec<Cell>, inner_w: usize, style: Style) -> Vec<Cell> {
    let mut out = vec![Cell::new(' ', style)];
    out.extend(cells.iter().cloned());
    let used = 1 + cells_width(&cells);
    if used < inner_w {
        out.extend((0..inner_w - used).map(|_| Cell::new(' ', style)));
    }
    out
}

/// 枠線の行1本（上/ヘッダー下の区切り/行間の区切り/下）: left の後に、
/// 各列の幅ぶんを埋め文字で描き、mid でつなぎ、right で閉じる。
fn border_cells(inner: &[usize], left: char, mid: char, right: char, style: Style) -> Vec<Cell> {
    let mut out = vec![Cell::new(left, style)];
    for (i, &w) in inner.iter().enumerate() {
        if i > 0 {
            out.push(Cell::new(mid, style));
        }
        out.extend((0..w).map(|_| Cell::new('\u{2500}', style)));
    }
    out.push(Cell::new(right, style));
    out
}

/// セルを最大 width 桁の表示幅までハードクリップする — 列の計算が収まりきらない
/// 場合（例: 幅3のパネルに4列のテーブル）でも、すべてのテーブル行を幅の上限内に
/// 収める最終防御。
fn clip_line(cells: Vec<Cell>, width: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut w = 0;
    for cell in cells {
        let cw = cell.width();
        if w + cw > width {
            break;
        }
        out.push(cell);
        w += cw;
    }
    cells_to_line(&out)
}

/// セル列全体の表示幅の合計。
fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| c.width()).sum()
}
