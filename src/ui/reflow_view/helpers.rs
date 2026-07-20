//! Marker/indent/truncation helpers — pure functions, testable independently
//! of `App`, used by [`build`](super::build) to lay out transcript lines.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
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
    let mut width = 0usize;
    // Reserve one column for the ellipsis so the indicator fits within max_cols.
    let budget = max_cols.saturating_sub(1);
    for (i, ch) in s.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
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
