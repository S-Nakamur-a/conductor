//! GFM table layout: borderless rendering (bold header, a rule, aligned rows)
//! with column widths fitted — and over-wide cells truncated — to the panel
//! width.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

use super::inline::inline_spans;
use super::parse::Align;
use super::wrap::{Cell, cells_to_line, char_width, spans_to_cells};

/// Render a GFM table borderless: a bold header row, a horizontal rule, then
/// aligned body rows with columns separated by two spaces. Box-drawing borders
/// are intentionally omitted — they cost too much width in the narrow summary
/// column. Over-wide cells are truncated with `…`. (A future refinement could
/// fall back to a `key: value` list when even truncation can't fit.)
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

    // Normalise alignments and body rows to exactly `ncols` columns so the
    // render side never indexes past the header column count.
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

    let header_style = Style::default().fg(theme.accent).add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(theme.fg);

    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(render_table_row(
        headers,
        &widths,
        &aligns,
        header_style,
        width,
        theme,
    ));
    // Rule under the header, clamped to the panel width.
    let rule_w = (widths.iter().sum::<usize>() + 2 * ncols.saturating_sub(1)).min(width);
    out.push(Line::from(Span::styled(
        "\u{2500}".repeat(rule_w),
        Style::default().fg(theme.muted),
    )));
    for row in &rows {
        out.push(render_table_row(
            row,
            &widths,
            &aligns,
            body_style,
            width,
            theme,
        ));
    }
    out
}

/// Natural width of each column = max rendered (markup-stripped) display width
/// over the header and every body cell.
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

/// Display width of `text` after inline markup is stripped, so column widths
/// match what actually renders (not the raw `**bold**` source).
fn rendered_width(text: &str, theme: &Theme) -> usize {
    cells_width(&spans_to_cells(&inline_spans(text, Style::default(), theme)))
}

/// Shrink natural column widths so the row (cells + 2-space separators) fits
/// `width`. Repeatedly trims the widest column by one (not proportional —
/// overkill for a rare wide table); every column keeps at least 1 column.
fn fit_col_widths(natural: &[usize], width: usize) -> Vec<usize> {
    let ncols = natural.len();
    if ncols == 0 {
        return vec![];
    }
    let seps = 2 * (ncols - 1);
    let avail = width.saturating_sub(seps).max(ncols); // >= 1 per column
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

/// Render one table row into a `Line`: each cell fitted to its column width and
/// alignment, columns joined by two spaces, then the whole hard-clipped to
/// `width` as a final guard for degenerate (tiny) widths.
fn render_table_row(
    cells: &[String],
    widths: &[usize],
    aligns: &[Align],
    base: Style,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let mut row: Vec<Cell> = Vec::new();
    for (k, &col_w) in widths.iter().enumerate() {
        if k > 0 {
            row.push(Cell { ch: ' ', style: base });
            row.push(Cell { ch: ' ', style: base });
        }
        let text = cells.get(k).map(String::as_str).unwrap_or("");
        let align = aligns.get(k).copied().unwrap_or(Align::Left);
        row.extend(fit_cell(text, col_w, align, base, theme));
    }
    cells_to_line(&clip_cells(row, width))
}

/// Fit `text` into exactly `col_w` display columns: render its inline markup,
/// truncate (with a trailing `…`) when too wide, then pad per `align`. Returns
/// cells whose total display width is `col_w` (empty when `col_w` is 0).
/// Works on `Cell`s, so truncation never splits a multibyte char.
fn fit_cell(text: &str, col_w: usize, align: Align, base: Style, theme: &Theme) -> Vec<Cell> {
    if col_w == 0 {
        return Vec::new();
    }
    let cells = truncate_cells(spans_to_cells(&inline_spans(text, base, theme)), col_w);
    let pad = col_w.saturating_sub(cells_width(&cells));
    let space = |n: usize| -> Vec<Cell> {
        (0..n).map(|_| Cell { ch: ' ', style: base }).collect()
    };
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

/// Truncate `cells` to at most `max_w` display columns, appending `…` (in the
/// last kept cell's style) only when content was actually cut. Truncates by
/// display width on char boundaries, so multibyte/CJK content never panics.
fn truncate_cells(cells: Vec<Cell>, max_w: usize) -> Vec<Cell> {
    if cells_width(&cells) <= max_w {
        return cells;
    }
    if max_w == 0 {
        return Vec::new();
    }
    let budget = max_w - 1; // reserve one column for the ellipsis
    let mut out: Vec<Cell> = Vec::new();
    let mut w = 0;
    for cell in cells {
        let cw = char_width(cell.ch);
        if w + cw > budget {
            break;
        }
        out.push(cell);
        w += cw;
    }
    let style = out.last().map(|c| c.style).unwrap_or_default();
    out.push(Cell {
        ch: '\u{2026}',
        style,
    });
    out
}

/// Hard-clip `cells` to at most `width` display columns (no ellipsis). The final
/// guard that keeps every table line within the width bound even when the column
/// math can't fit (e.g. a 4-column table in a 3-wide panel).
fn clip_cells(cells: Vec<Cell>, width: usize) -> Vec<Cell> {
    let mut out = Vec::new();
    let mut w = 0;
    for cell in cells {
        let cw = char_width(cell.ch);
        if w + cw > width {
            break;
        }
        out.push(cell);
        w += cw;
    }
    out
}

/// Total display width of a cell slice.
fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| char_width(c.ch)).sum()
}
