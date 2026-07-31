//! GFM table layout for the Claude Code transcript flavor: a box-drawing grid
//! (`┌─┬─┐` / `├─┼─┤` / `└─┴─┘`) matching native Claude Code's default table
//! rendering — every column padded to `max(cell width) + 2`, a rule between
//! every row (not just under the header), left-aligned cells, and no colour
//! or bold anywhere (the native output carries no SGR at all for tables).
//!
//! This is a separate module from [`super::table`] (the Rich flavor's
//! borderless layout) rather than a branch inside it: the two share almost no
//! layout code — border rows, per-row rule placement, and forced left
//! alignment have no Rich-flavor equivalent — so folding them into one
//! function would mostly be `if flavor == Transcript { .. } else { .. }`
//! wrapped around otherwise-unrelated logic.

use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::theme::Theme;

use super::MarkdownFlavor;
use super::inline::inline_spans;
use super::wrap::{Cell, cells_to_line, spans_to_cells, wrap_cells_raw};

/// Render a GFM table as a bordered box. Column alignment hints
/// (`:--`/`--:`/`:-:`) are ignored — native Claude Code always left-aligns
/// cell content regardless of the source delimiter row.
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

    // Normalise body rows to exactly `ncols` columns, same as the Rich table.
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
    out.push(clip_line(border_cells(&inner, '\u{250c}', '\u{252c}', '\u{2510}', style), width));
    out.extend(render_row(headers, &inner, theme, style, width));
    out.push(clip_line(border_cells(&inner, '\u{251c}', '\u{253c}', '\u{2524}', style), width));
    for (idx, row) in rows.iter().enumerate() {
        out.extend(render_row(row, &inner, theme, style, width));
        if idx + 1 < rows.len() {
            out.push(clip_line(border_cells(&inner, '\u{251c}', '\u{253c}', '\u{2524}', style), width));
        }
    }
    out.push(clip_line(border_cells(&inner, '\u{2514}', '\u{2534}', '\u{2518}', style), width));
    out
}

/// Natural inner width of each column: `max(cell display width) + 2` (one
/// column of padding on each side), over the header and every body cell.
fn natural_inner_widths(headers: &[String], rows: &[Vec<String>], theme: &Theme) -> Vec<usize> {
    let mut w: Vec<usize> = headers.iter().map(|h| rendered_width(h, theme) + 2).collect();
    for row in rows {
        for (k, cell) in row.iter().enumerate() {
            if let Some(col) = w.get_mut(k) {
                *col = (*col).max(rendered_width(cell, theme) + 2);
            }
        }
    }
    w
}

/// Display width of `text` after inline markup is stripped.
fn rendered_width(text: &str, theme: &Theme) -> usize {
    cells_width(&spans_to_cells(&inline_spans(
        text,
        Style::default(),
        theme,
        MarkdownFlavor::Transcript,
    )))
}

/// Shrink natural inner widths so the whole grid (columns + one border
/// character per boundary, `ncols + 1` of them) fits `width`. Mirrors
/// [`super::table::fit_col_widths`]'s trim-the-widest-column-by-one approach;
/// every column keeps at least 1 column.
fn fit_inner_widths(natural: &[usize], width: usize) -> Vec<usize> {
    let ncols = natural.len();
    if ncols == 0 {
        return vec![];
    }
    let overhead = ncols + 1; // vertical border chars: one per column boundary
    let avail = width.saturating_sub(overhead).max(ncols); // >= 1 per column
    let mut w: Vec<usize> = natural.iter().map(|&x| x.max(1).min(avail)).collect();
    while w.iter().sum::<usize>() > avail {
        let maxw = *w.iter().max().unwrap();
        if maxw <= 1 {
            break; // can't shrink further; the final clip guards the width bound
        }
        let idx = w.iter().position(|&x| x == maxw).unwrap();
        w[idx] -= 1;
    }
    w
}

/// Render one table row into as many `Line`s as its tallest cell needs (a
/// cell too wide for its column wraps rather than truncating, same policy as
/// the Rich table).
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

/// Wrap `text` into lines of exactly `inner_w` display columns: strip inline
/// markup styling other than colour/emphasis, wrap at word boundaries inside
/// the `inner_w - 2` content area, then pad left-aligned with one space of
/// padding on each side. Always returns at least one line.
fn wrap_cell(text: &str, inner_w: usize, style: Style, theme: &Theme) -> Vec<Vec<Cell>> {
    if inner_w == 0 {
        return vec![Vec::new()];
    }
    let content_w = inner_w.saturating_sub(2).max(1);
    let cells = spans_to_cells(&inline_spans(text, style, theme, MarkdownFlavor::Transcript));
    wrap_cells_raw(&cells, content_w, false)
        .into_iter()
        .map(|line| pad_left(line, inner_w, style))
        .collect()
}

/// Pad one wrapped content line out to `inner_w` columns: one space, the
/// content, then spaces out to `inner_w - 1`.
fn pad_left(cells: Vec<Cell>, inner_w: usize, style: Style) -> Vec<Cell> {
    let mut out = vec![Cell::new(' ', style)];
    out.extend(cells.iter().cloned());
    let used = 1 + cells_width(&cells);
    if used < inner_w {
        out.extend((0..inner_w - used).map(|_| Cell::new(' ', style)));
    }
    out
}

/// One border row (top/header-rule/row-rule/bottom): `left`, then each
/// column's width in `fill`, joined by `mid`, closed with `right`.
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

/// Hard-clip `cells` to at most `width` display columns — the final guard
/// that keeps every table line within the width bound even when the column
/// math can't fit (e.g. a 4-column table in a 3-wide panel).
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

/// Total display width of a cell slice.
fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| c.width()).sum()
}
