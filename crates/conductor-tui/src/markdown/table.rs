//! GFM テーブルのレイアウト (Rich フレーバー)。

use conductor_core::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::Flavor;
use super::inline::inline_spans;
use super::parse::Align;
use super::wrap::{Cell, cells_to_line, spans_to_cells, wrap_cells_raw};

/// 罫線を意図的に省いているのは、幅の狭いサマリー列では罫線のコストが大きすぎるため。
pub(crate) fn render_table(
    headers: &[String],
    aligns: &[Align],
    rows: &[Vec<String>],
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let ncols = headers.len();
    if ncols == 0 {
        return vec![Line::from("")];
    }

    // アライメントと本体行を、ちょうど ncols 列に正規化する。描画側が
    // ヘッダーの列数を超えてインデックスすることがないようにするため。
    let aligns: Vec<Align> = (0..ncols)
        .map(|k| aligns.get(k).copied().unwrap_or(Align::Left))
        .collect();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut row: Vec<String> = r.iter().take(ncols).cloned().collect();
            row.resize(ncols, String::new());
            row
        })
        .collect();

    let widths = fit_col_widths(&natural_col_widths(headers, &rows, theme), width);

    let header_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(theme.fg);

    let mut out = Vec::with_capacity(rows.len() + 2);
    out.extend(render_table_row(
        headers,
        &widths,
        &aligns,
        header_style,
        width,
        theme,
    ));
    let rule_w = (widths.iter().sum::<usize>() + 2 * ncols.saturating_sub(1)).min(width);
    out.push(Line::from(Span::styled(
        "\u{2500}".repeat(rule_w),
        Style::default().fg(theme.muted),
    )));
    for row in &rows {
        out.extend(render_table_row(
            row, &widths, &aligns, body_style, width, theme,
        ));
    }
    out
}

fn natural_col_widths(headers: &[String], rows: &[Vec<String>], theme: &Theme) -> Vec<usize> {
    let mut w: Vec<usize> = headers.iter().map(|h| rendered_width(h, theme)).collect();
    for row in rows {
        for (k, cell) in row.iter().enumerate() {
            if let Some(col) = w.get_mut(k) {
                *col = (*col).max(rendered_width(cell, theme));
            }
        }
    }
    w
}

/// インラインマークアップを取り除いた後のテキストの表示幅。列幅が実際の
/// 描画結果に一致するようにする（生の **bold** ソースとは一致させない）。
fn rendered_width(text: &str, theme: &Theme) -> usize {
    cells_width(&spans_to_cells(&inline_spans(
        text,
        Style::default(),
        theme,
        Flavor::Rich,
    )))
}

/// 最も広い列を 1 ずつ削る。比例配分にしないのは、稀にしかない幅広テーブルのために
/// そこまでやる必要が無いため。どの列も最低 1 列は確保する。
fn fit_col_widths(natural: &[usize], width: usize) -> Vec<usize> {
    let ncols = natural.len();
    if ncols == 0 {
        return vec![];
    }
    let seps = 2 * (ncols - 1);
    let avail = width.saturating_sub(seps).max(ncols); // 各列最低1
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

fn render_table_row(
    cells: &[String],
    widths: &[usize],
    aligns: &[Align],
    base: Style,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let cols: Vec<Vec<Vec<Cell>>> = widths
        .iter()
        .enumerate()
        .map(|(k, &col_w)| {
            let text = cells.get(k).map(String::as_str).unwrap_or("");
            let align = aligns.get(k).copied().unwrap_or(Align::Left);
            wrap_cell(text, col_w, align, base, theme)
        })
        .collect();

    let height = cols.iter().map(Vec::len).max().unwrap_or(0);
    let blank = |col_w: usize| -> Vec<Cell> { (0..col_w).map(|_| Cell::new(' ', base)).collect() };

    (0..height)
        .map(|row_line| {
            let mut row: Vec<Cell> = Vec::new();
            for (k, col) in cols.iter().enumerate() {
                if k > 0 {
                    row.push(Cell::new(' ', base));
                    row.push(Cell::new(' ', base));
                }
                match col.get(row_line) {
                    Some(line) => row.extend(line.iter().cloned()),
                    None => row.extend(blank(widths[k])),
                }
            }
            cells_to_line(&clip_cells(row, width))
        })
        .collect()
}

/// 常に最低 1 行を返す (空セルは空行 1 つ)。行の高さがセルの最大値になり、ゼロにならない。
fn wrap_cell(text: &str, col_w: usize, align: Align, base: Style, theme: &Theme) -> Vec<Vec<Cell>> {
    if col_w == 0 {
        return vec![Vec::new()];
    }
    let cells = spans_to_cells(&inline_spans(text, base, theme, Flavor::Rich));
    wrap_cells_raw(&cells, col_w, false)
        .into_iter()
        .map(|line| pad_cell_line(line, col_w, align, base))
        .collect()
}

fn pad_cell_line(cells: Vec<Cell>, col_w: usize, align: Align, base: Style) -> Vec<Cell> {
    let pad = col_w.saturating_sub(cells_width(&cells));
    let space = |n: usize| -> Vec<Cell> { (0..n).map(|_| Cell::new(' ', base)).collect() };
    match align {
        Align::Left => {
            let mut out = cells;
            out.extend(space(pad));
            out
        }
        Align::Right => {
            let mut out = space(pad);
            out.extend(cells);
            out
        }
        Align::Center => {
            let left = pad / 2;
            let mut out = space(left);
            out.extend(cells);
            out.extend(space(pad - left));
            out
        }
    }
}

/// 列の計算が収まりきらない場合 (幅 3 のパネルに 4 列など) でも、全行を幅の上限に収める最終防御。
fn clip_cells(cells: Vec<Cell>, width: usize) -> Vec<Cell> {
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
    out
}

fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| c.width()).sum()
}
