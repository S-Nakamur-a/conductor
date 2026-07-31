//! Tests for [`super::user_text`] — the user-turn full-width background
//! block: word-wrapping, column padding, and marker/continuation layout.

use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthStr;

use super::glyphs::MARKER_COLS;
use super::palette;
use super::user_text::{pad_to_width, render_user_text, wrap_plain_text};

// ── wrap_plain_text ─────────────────────────────────────────────────────

#[test]
fn wrap_fits_on_one_line_unchanged() {
    assert_eq!(wrap_plain_text("hello world", 20), vec!["hello world"]);
}

#[test]
fn wrap_breaks_at_word_boundaries() {
    assert_eq!(
        wrap_plain_text("one two three four", 9),
        vec!["one two", "three", "four"]
    );
}

#[test]
fn wrap_preserves_source_newlines_as_independent_lines() {
    // Two short source lines, each well under the width budget — they must
    // stay on separate output lines, not get joined into one reflowed
    // paragraph the way Markdown prose would.
    assert_eq!(
        wrap_plain_text("first line\nsecond line", 40),
        vec!["first line", "second line"]
    );
}

#[test]
fn wrap_preserves_blank_source_lines() {
    assert_eq!(
        wrap_plain_text("a\n\nb", 10),
        vec!["a", "", "b"]
    );
}

#[test]
fn wrap_overlong_single_word_is_hard_split() {
    // Measured against Claude Code: `W`x150 at a 57-column budget comes back
    // as 57 / 57 / 36, so an unbreakable run is cut at the column boundary
    // rather than allowed to overflow (which is what
    // `ui::walkthrough_pane::wrap_text` does — parity wins here).
    assert_eq!(
        wrap_plain_text("supercalifragilistic", 5),
        vec!["super", "calif", "ragil", "istic"]
    );
}

#[test]
fn wrap_hard_split_matches_the_measured_native_shape() {
    let chunks = wrap_plain_text(&"W".repeat(150), 57);
    let widths: Vec<usize> = chunks.iter().map(|c| c.chars().count()).collect();
    assert_eq!(widths, vec![57, 57, 36]);
}

#[test]
fn wrap_hard_split_never_breaks_a_full_width_glyph() {
    // Budget 5 with 2-column glyphs: 2 per line, never a half-glyph line.
    let chunks = wrap_plain_text(&"あ".repeat(5), 5);
    for c in &chunks {
        assert!(unicode_width::UnicodeWidthStr::width(c.as_str()) <= 5, "{c:?}");
    }
    assert_eq!(chunks.concat(), "あ".repeat(5));
}

// ── pad_to_width ─────────────────────────────────────────────────────────

#[test]
fn pad_short_string_fills_with_trailing_spaces() {
    let padded = pad_to_width("hi", 5);
    assert_eq!(padded, "hi   ");
    assert_eq!(UnicodeWidthStr::width(padded.as_str()), 5);
}

#[test]
fn pad_string_already_at_width_unchanged() {
    assert_eq!(pad_to_width("hello", 5), "hello");
}

#[test]
fn pad_string_wider_than_target_unchanged() {
    assert_eq!(pad_to_width("hello world", 5), "hello world");
}

// ── render_user_text ────────────────────────────────────────────────────

fn marker_style() -> Style {
    Style::default().fg(palette::USER_MARKER_FG).bg(palette::USER_BG)
}

fn body_style() -> Style {
    Style::default().fg(palette::USER_TEXT).bg(palette::USER_BG)
}

#[test]
fn first_line_gets_the_marker_continuation_lines_get_blank_indent() {
    let lines = render_user_text(
        "one two three four five six seven",
        12,
        "\u{276f}",
        marker_style(),
        body_style(),
    );
    assert!(lines.len() > 1, "expected the body to wrap onto multiple lines");
    assert_eq!(lines[0].spans[0].content, "\u{276f} ");
    assert_eq!(lines[1].spans[0].content, "  ");
}

#[test]
fn every_line_is_padded_to_the_full_panel_width_for_the_background() {
    let width = 20;
    let lines = render_user_text("short", width, "\u{276f}", marker_style(), body_style());
    for line in &lines {
        let total: usize = line
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(total, width, "line {line:?} must fill the full width for its background");
    }
}

#[test]
fn marker_and_body_spans_carry_the_background_color() {
    let lines = render_user_text("hi", 10, "\u{276f}", marker_style(), body_style());
    let line = &lines[0];
    assert_eq!(line.spans[0].style.bg, Some(Color::Rgb(55, 55, 55)));
    assert_eq!(line.spans[1].style.bg, Some(Color::Rgb(55, 55, 55)));
}

#[test]
fn source_newlines_survive_as_separate_lines_each_with_their_own_gutter_slot() {
    let lines = render_user_text("first\nsecond", 20, "\u{276f}", marker_style(), body_style());
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].spans[0].content, "\u{276f} ");
    assert_eq!(lines[1].spans[0].content, "  ");
}

#[test]
fn body_wraps_at_width_minus_marker_cols() {
    // width=10 leaves body_width = 10 - MARKER_COLS(2) = 8 columns for text.
    let lines = render_user_text("abcdefgh ijkl", 10, "\u{276f}", marker_style(), body_style());
    assert!(lines.len() >= 2, "8-col budget must force a wrap: {lines:?}");
    // Guard the constant this test relies on so it fails loudly if MARKER_COLS
    // ever changes instead of silently asserting the wrong budget.
    assert_eq!(MARKER_COLS, 2);
}

// ── Grapheme-cluster width accounting ────────────────────────────────────
//
// These were found by the corpus sweep, not by inspection: a per-`char` sum
// disagrees with the per-string width in both directions, and either way the
// wrapped line no longer matches the budget it was wrapped to.

#[test]
fn emoji_presentation_sequence_counts_as_two_columns() {
    // `⚠` is 1 column; `⚠` + U+FE0F is 2. Summing per `char` sees only the
    // base (the selector is zero-width), so the line came out one column
    // over-wide — the exact bleed the panel-width invariant catches.
    let warn = "\u{26a0}\u{fe0f}";
    assert_eq!(unicode_width::UnicodeWidthStr::width(warn), 2);

    let wrapped = wrap_plain_text(&warn.repeat(5), 4);
    for line in &wrapped {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.as_str()) <= 4,
            "{line:?} exceeds the budget"
        );
    }
    assert_eq!(wrapped.concat(), warn.repeat(5), "no cluster was dropped");
}

#[test]
fn zwj_sequence_is_never_split() {
    // A family emoji is 2 columns but seven `char`s; splitting between them
    // would both mis-measure and leave half a sequence on screen.
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let wrapped = wrap_plain_text(&family.repeat(3), 4);
    assert_eq!(wrapped.concat(), family.repeat(3));
    for line in &wrapped {
        assert!(unicode_width::UnicodeWidthStr::width(line.as_str()) <= 4, "{line:?}");
    }
}
