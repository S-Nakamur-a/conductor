//! Display-width-aware span wrapping. Rendering works at the granularity of a
//! [`Cell`] (one char plus its style) so styles survive line breaks, then
//! coalesces adjacent same-style cells back into `Span`s.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// A single display cell: one char carrying its style. The wrapping helpers
/// work at this granularity so styles survive line breaks.
#[derive(Clone)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) style: Style,
}

pub(crate) fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

pub(crate) fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

pub(crate) fn spans_to_cells(spans: &[Span<'static>]) -> Vec<Cell> {
    spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |ch| Cell { ch, style: s.style }))
        .collect()
}

/// Merge a run of cells back into a `Line`, coalescing adjacent same-style cells
/// into one `Span`.
pub(crate) fn cells_to_line(cells: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    for cell in cells {
        match cur {
            Some(s) if s == cell.style => buf.push(cell.ch),
            _ => {
                if let Some(s) = cur {
                    spans.push(Span::styled(std::mem::take(&mut buf), s));
                }
                buf.push(cell.ch);
                cur = Some(cell.style);
            }
        }
    }
    if let Some(s) = cur {
        spans.push(Span::styled(buf, s));
    }
    if spans.is_empty() {
        Line::from("")
    } else {
        Line::from(spans)
    }
}

/// Wrap `cells` to `width` display columns. When `hard` is set (code blocks),
/// breaks fall on any cell boundary; otherwise breaks prefer word boundaries
/// and only overlong single words are hard-split. Display width is measured with
/// `unicode-width`, so full-width (CJK) text wraps correctly.
pub(crate) fn wrap_cells(cells: &[Cell], width: usize, hard: bool) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut cur_w = 0usize;

    let mut push_line = |cur: &mut Vec<Cell>, cur_w: &mut usize| {
        lines.push(cells_to_line(cur));
        cur.clear();
        *cur_w = 0;
    };

    if hard {
        for cell in cells {
            let cw = char_width(cell.ch);
            if cur_w + cw > width && !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            cur.push(cell.clone());
            cur_w += cw;
        }
        if !cur.is_empty() {
            push_line(&mut cur, &mut cur_w);
        }
        if lines.is_empty() {
            lines.push(Line::from(""));
        }
        return lines;
    }

    let n = cells.len();
    let mut i = 0;
    while i < n {
        if cells[i].ch == ' ' {
            // A space: keep it only if it fits on the current (non-empty) line;
            // otherwise drop it as the wrap point so the next line has no
            // leading space.
            if !cur.is_empty() {
                if cur_w < width {
                    cur.push(cells[i].clone());
                    cur_w += 1;
                } else {
                    push_line(&mut cur, &mut cur_w);
                }
            }
            i += 1;
            continue;
        }

        // Gather the next word (run of non-space cells).
        let start = i;
        let mut word_w = 0;
        while i < n && cells[i].ch != ' ' {
            word_w += char_width(cells[i].ch);
            i += 1;
        }
        let word = &cells[start..i];

        if word_w > width {
            // Word longer than a line: flush, then hard-split it.
            if !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            for cell in word {
                let cw = char_width(cell.ch);
                if cur_w + cw > width && !cur.is_empty() {
                    push_line(&mut cur, &mut cur_w);
                }
                cur.push(cell.clone());
                cur_w += cw;
            }
        } else {
            if cur_w + word_w > width && !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            for cell in word {
                cur.push(cell.clone());
                cur_w += char_width(cell.ch);
            }
        }
    }
    if !cur.is_empty() {
        push_line(&mut cur, &mut cur_w);
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

/// Prepend a gutter/marker prefix to wrapped lines: `first` on the first line,
/// `cont` (a same-width pad) on continuation lines.
pub(crate) fn with_prefix(
    lines: Vec<Line<'static>>,
    first: Span<'static>,
    cont: Span<'static>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(if idx == 0 { first.clone() } else { cont.clone() });
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}
