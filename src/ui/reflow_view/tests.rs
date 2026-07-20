//! Tests for the marker/indent/truncation helpers in [`super::helpers`] and
//! the gutter-width invariant of [`super::glyphs`].

use ratatui::style::{Color, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::glyphs::{ASSISTANT_MARKER, MARKER_COLS, THINKING_GLYPH, TOOL_RESULT_GLYPH, USER_MARKER};
use super::helpers::{pad_glyph_to, truncate_to_width, with_marker};

// ── pad_glyph_to ─────────────────────────────────────────────────────────

#[test]
fn pad_glyph_ascii_pads_to_target() {
    // ">" is 1 col wide; padded to 2 should give "> ".
    assert_eq!(pad_glyph_to(">", 2), "> ");
}

#[test]
fn pad_glyph_already_at_target_unchanged() {
    assert_eq!(pad_glyph_to("=>", 2), "=>");
}

#[test]
fn pad_glyph_wider_than_target_unchanged() {
    assert_eq!(pad_glyph_to("abc", 2), "abc");
}

#[test]
fn pad_glyph_assistant_marker_produces_two_cols() {
    // ⏺ (U+23FA) has unicode_width of 1; padded to 2 should append one space.
    let padded = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
    assert_eq!(UnicodeWidthStr::width(padded.as_str()), MARKER_COLS);
}

#[test]
fn gutter_markers_are_exactly_one_column() {
    // The gutter markers MUST measure as a single column. They render with
    // emoji presentation (2 cols) in many fonts; the VS15 text-presentation
    // selector forces narrow rendering to match this width. If a marker ever
    // measures >1 here, every transcript line will be one column short and
    // its last char will bleed past the panel edge (the regression that the
    // VS15 suffix fixes).
    for (name, m) in [
        ("assistant", ASSISTANT_MARKER),
        ("tool-result", TOOL_RESULT_GLYPH),
        ("thinking", THINKING_GLYPH),
        ("user", USER_MARKER),
    ] {
        assert_eq!(
            UnicodeWidthStr::width(m),
            1,
            "marker {name} must be exactly 1 display column"
        );
    }
}

// ── with_marker ──────────────────────────────────────────────────────────

#[test]
fn with_marker_prepends_glyph_to_first_line() {
    let style = Style::default().fg(Color::Green);
    let lines = vec![
        Line::from("hello"),
        Line::from("world"),
    ];
    let result = with_marker(lines, ">", style);
    assert_eq!(result.len(), 2);
    // First span of first line is the marker.
    assert_eq!(result[0].spans[0].content, "> ");
    // Second line gets a blank indent.
    assert_eq!(result[1].spans[0].content, "  ");
}

#[test]
fn with_marker_empty_input_returns_empty() {
    let style = Style::default();
    let result = with_marker(vec![], ">", style);
    assert!(result.is_empty());
}

#[test]
fn with_marker_single_line_no_continuation() {
    let style = Style::default();
    let result = with_marker(vec![Line::from("only")], ">", style);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].spans[0].content, "> ");
}

// ── truncate_to_width ────────────────────────────────────────────────────

#[test]
fn truncate_fits_returns_unchanged() {
    assert_eq!(truncate_to_width("hello", 10), "hello");
}

#[test]
fn truncate_over_limit_appends_ellipsis() {
    let result = truncate_to_width("hello world", 6);
    assert!(result.ends_with('\u{2026}'));
    assert!(UnicodeWidthStr::width(result.as_str()) <= 6);
}

#[test]
fn truncate_zero_budget_returns_empty() {
    assert_eq!(truncate_to_width("anything", 0), "");
}
