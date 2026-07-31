//! Full-width background block rendering for user turns (S3, measured):
//! Claude Code's live UI paints the whole row behind a user prompt, not just
//! the marker and text — this module reproduces that here.
//!
//! User input is raw text, not prose to be parsed: it bypasses the Markdown
//! renderer entirely (see [`build`](super::build)) and is word-wrapped by
//! this module instead, preserving every source newline as its own wrapped
//! line rather than reflowing them the way Markdown's paragraph-fill would.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::glyphs::MARKER_COLS;
use super::helpers::pad_glyph_to;

/// Columns of background-only padding kept to the right of a user turn's
/// text, inside the block. Measured: at width 60 the body is 57 columns
/// (`60 - MARKER_COLS - 1`) and column 59 is a bare background cell; at
/// width 100 it is 97. Assistant text has no such reserve — it wraps at
/// `width - MARKER_COLS` — so this is specific to the user block.
const USER_TRAILING_PAD: usize = 1;

/// Render one user-turn text block as full-width background lines: `glyph`
/// on the first line only (two-space blank indent on continuations, same as
/// every other block type's gutter), body text word-wrapped at
/// `width - MARKER_COLS - USER_TRAILING_PAD` but padded back out to
/// `width - MARKER_COLS` so the background still reaches the panel edge.
/// `marker_style` and `body_style` must already carry the background color —
/// this function only supplies the text content, so the caller controls the
/// palette (mirrors `tool_lines::ToolStyles`).
pub(crate) fn render_user_text(
    text: &str,
    width: usize,
    glyph: &str,
    marker_style: Style,
    body_style: Style,
) -> Vec<Line<'static>> {
    // Painted width of the body column (background reaches the panel edge)…
    let body_width = width.saturating_sub(MARKER_COLS);
    // …but text stops one column short of it.
    let wrap_width = body_width.saturating_sub(USER_TRAILING_PAD);
    let marker_prefix = pad_glyph_to(glyph, MARKER_COLS);
    let cont_prefix = " ".repeat(MARKER_COLS);

    wrap_plain_text(text, wrap_width)
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let prefix = if i == 0 {
                marker_prefix.clone()
            } else {
                cont_prefix.clone()
            };
            // Pad the body out to the full column budget so the background
            // color fills the row instead of stopping at the last glyph — a
            // `Style` with only `bg()` set doesn't paint columns a span
            // doesn't cover.
            let padded_body = pad_to_width(&body, body_width);
            Line::from(vec![
                Span::styled(prefix, marker_style),
                Span::styled(padded_body, body_style),
            ])
        })
        .collect()
}

/// Greedily word-wrap `text` to `width` display columns (measured with
/// `unicode_width`), splitting on existing newlines first so the source's
/// own line breaks survive as independent wrapped lines rather than being
/// folded into a reflowed paragraph.
///
/// A word wider than `width` is **hard-split at the column boundary**, not
/// left to overflow: measured, `W`×150 at width 60 comes back as 57 / 57 / 36.
/// (This differs from `ui::walkthrough_pane::wrap_text`, which lets such a
/// word overflow — parity with Claude Code wins here.) The split walks
/// grapheme clusters and never breaks one across two lines, and it **fills
/// the remainder of the current line first** rather than flushing it: measured
/// on a `⎿ Read <very long path>` annotation, where the path begins on the
/// `Read` line and breaks at the column boundary.
pub(crate) fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for source_line in text.split('\n') {
        if source_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_w = 0usize;
        for word in source_line.split_whitespace() {
            let word_w = UnicodeWidthStr::width(word);
            if word_w > width {
                // Fill the rest of the current line before spilling over,
                // rather than flushing it first. Measured on a `⎿ Read <long
                // path>` annotation: the path starts on the `Read` line and
                // breaks at the column boundary, so the verb never sits alone.
                if !current.is_empty() {
                    if current_w + 1 < width {
                        current.push(' ');
                        current_w += 1;
                    } else {
                        out.push(std::mem::take(&mut current));
                        current_w = 0;
                    }
                }
                for cluster in word.graphemes(true) {
                    let cw = UnicodeWidthStr::width(cluster);
                    if current_w + cw > width && !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                        current_w = 0;
                    }
                    current.push_str(cluster);
                    current_w += cw;
                }
                continue;
            }
            let candidate_w = if current.is_empty() {
                word_w
            } else {
                current_w + 1 + word_w
            };
            if candidate_w > width && !current.is_empty() {
                out.push(std::mem::take(&mut current));
                current_w = 0;
            }
            if !current.is_empty() {
                current.push(' ');
                current_w += 1;
            }
            current.push_str(word);
            current_w += word_w;
        }
        out.push(current);
    }
    out
}

/// Pad `s` with trailing spaces until it fills exactly `width` display
/// columns; left unchanged if it already meets or exceeds `width`.
pub(crate) fn pad_to_width(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        let mut out = s.to_string();
        for _ in 0..(width - w) {
            out.push(' ');
        }
        out
    }
}
