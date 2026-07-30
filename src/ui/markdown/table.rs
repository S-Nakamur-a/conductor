//! GFM table layout: borderless rendering (bold header, a rule, aligned rows)
//! with column widths fitted to the panel width and over-wide cells wrapped
//! onto extra lines.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

use super::inline::inline_spans;
use super::parse::Align;
use super::wrap::{Cell, cells_to_line, char_width, spans_to_cells, wrap_cells_raw};

/// Render a GFM table borderless: a bold header row, a horizontal rule, then
/// aligned body rows with columns separated by two spaces. Box-drawing borders
/// are intentionally omitted — they cost too much width in the narrow summary
/// column.
///
/// A cell too wide for its column **wraps** rather than truncating, so a row
/// occupies as many lines as its tallest cell needs. Truncation was the earlier
/// behaviour and silently destroyed content — in a table the cut text is often
/// the whole point of the row, and there is no way to reveal it (no horizontal
/// scroll, no expand). Height is the cheaper thing to spend: the views that
/// render markdown all scroll vertically.
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
    out.extend(render_table_row(
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
        out.extend(render_table_row(
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

/// Render one table row into as many `Line`s as its tallest cell needs.
///
/// Each cell is wrapped to its column width and padded per its alignment, so
/// every column contributes the same number of columns on every line and the
/// grid stays aligned. Short cells are padded with blank lines at the bottom.
/// Each produced line is hard-clipped to `width` as a final guard for
/// degenerate (tiny) widths.
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
    let blank = |col_w: usize| -> Vec<Cell> {
        (0..col_w).map(|_| Cell { ch: ' ', style: base }).collect()
    };

    (0..height)
        .map(|row_line| {
            let mut row: Vec<Cell> = Vec::new();
            for (k, col) in cols.iter().enumerate() {
                if k > 0 {
                    row.push(Cell { ch: ' ', style: base });
                    row.push(Cell { ch: ' ', style: base });
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

/// Wrap `text` into lines of exactly `col_w` display columns: render its inline
/// markup, wrap at word boundaries, then pad each line per `align`. Always
/// returns at least one line (empty cells become one blank line), so a row's
/// height is `max` over its cells and never zero.
fn wrap_cell(
    text: &str,
    col_w: usize,
    align: Align,
    base: Style,
    theme: &Theme,
) -> Vec<Vec<Cell>> {
    if col_w == 0 {
        return vec![Vec::new()];
    }
    let cells = spans_to_cells(&inline_spans(text, base, theme));
    wrap_cells_raw(&cells, col_w, false)
        .into_iter()
        .map(|line| pad_cell_line(line, col_w, align, base))
        .collect()
}

/// Pad one wrapped line out to `col_w` columns according to `align`.
fn pad_cell_line(cells: Vec<Cell>, col_w: usize, align: Align, base: Style) -> Vec<Cell> {
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
