//! Marker/indent/truncation helpers — pure functions, testable independently
//! of `App`, used by [`build`](super::build) to lay out transcript lines.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::glyphs::MARKER_COLS;

/// Prepend a `MARKER_COLS`-wide marker to the first line of `lines` and a
/// same-width blank indent to all continuation lines.
///
/// `glyph` is measured with `unicode_width` and padded with spaces to exactly
/// `MARKER_COLS` display columns before being inserted as the leading span.
/// Content spans on each line keep their original styling.
pub(crate) fn with_marker(
    lines: Vec<Line<'static>>,
    glyph: &str,
    marker_style: Style,
) -> Vec<Line<'static>> {
    let marker_prefix = pad_glyph_to(glyph, MARKER_COLS);
    let cont_prefix = " ".repeat(MARKER_COLS);

    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut line)| {
            let prefix = if i == 0 {
                Span::styled(marker_prefix.clone(), marker_style)
            } else {
                Span::raw(cont_prefix.clone())
            };
            line.spans.insert(0, prefix);
            line
        })
        .collect()
}

/// Pad `glyph` with trailing spaces until it occupies exactly `target_cols`
/// display columns.  If the glyph is already `target_cols` wide or wider,
/// returns it unchanged.
pub(crate) fn pad_glyph_to(glyph: &str, target_cols: usize) -> String {
    let w = UnicodeWidthStr::width(glyph);
    if w >= target_cols {
        glyph.to_string()
    } else {
        let mut s = glyph.to_string();
        for _ in 0..(target_cols - w) {
            s.push(' ');
        }
        s
    }
}

/// Truncate `s` to at most `max_cols` display columns, appending `…` if cut.
///
/// Walks Unicode scalar values, accumulates display width, and cuts before the
/// first character that would overflow.  Returns a `String` (owned) so callers
/// can pass it directly to `Span::styled`.
pub(crate) fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    // Reserve the ellipsis column only when there is something to elide.
    // Reserving it unconditionally cut strings that fit exactly —
    // `truncate_to_width("hello", 5)` used to return `"hell…"`.
    if UnicodeWidthStr::width(s) <= max_cols {
        return s.to_string();
    }
    let mut width = 0usize;
    let budget = max_cols - 1;
    // Grapheme clusters, not `char`s: cutting between a base character and
    // its variation selector (or mid-ZWJ-sequence) both mis-measures the
    // width and leaves a dangling combining mark on screen.
    for (i, cluster) in s.grapheme_indices(true) {
        let cw = UnicodeWidthStr::width(cluster);
        if width + cw > budget {
            let mut out = s[..i].to_string();
            out.push('\u{2026}'); // …
            return out;
        }
        width += cw;
    }
    // String fits within max_cols without truncation.
    s.to_string()
}

/// Assemble `parts` into one line behind `indent_cols` blank columns,
/// degrading to a single truncated span if the result would exceed `width`.
///
/// The fixed-format summary lines (`Read 3 files (ctrl+o to expand)`,
/// `Thought for 8s (ctrl+o to expand)`) are built from several spans so that
/// only the count is bold. At a narrow panel the whole thing has to be cut,
/// and cutting span-by-span would leave those bold/plain boundaries in
/// nonsense places — so the fallback keeps the text and drops the styling.
/// Without this the line is simply emitted over-width, which is the bleed the
/// corpus sweep exists to catch.
pub(crate) fn fit_styled_line(
    indent_cols: usize,
    parts: &[(String, Style)],
    width: usize,
) -> Line<'static> {
    let indent = " ".repeat(indent_cols);
    let budget = width.saturating_sub(indent_cols);
    let plain: String = parts.iter().map(|(t, _)| t.as_str()).collect();

    if UnicodeWidthStr::width(plain.as_str()) <= budget {
        let mut spans = Vec::with_capacity(parts.len() + 1);
        spans.push(Span::raw(indent));
        spans.extend(parts.iter().map(|(t, s)| Span::styled(t.clone(), *s)));
        return Line::from(spans);
    }
    let fallback = parts.first().map(|(_, s)| *s).unwrap_or_default();
    Line::from(vec![
        Span::raw(indent),
        Span::styled(truncate_to_width(&plain, budget), fallback),
    ])
}

/// [`fit_styled_line`] with `glyph` in the marker gutter instead of blanks —
/// for the single-line blocks that own a marker of their own (`⏺ {notice}`,
/// `✻ Conversation compacted …`). The glyph takes the style of the first part.
///
/// Both of `fit_styled_line`'s branches put the indent in span 0, so replacing
/// it in place keeps the fitted body untouched.
pub(crate) fn fit_glyph_line(
    glyph: &str,
    parts: &[(String, Style)],
    width: usize,
) -> Line<'static> {
    let mut line = fit_styled_line(MARKER_COLS, parts, width);
    let style = parts.first().map(|(_, s)| *s).unwrap_or_default();
    line.spans[0] = Span::styled(pad_glyph_to(glyph, MARKER_COLS), style);
    line
}
