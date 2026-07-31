//! Tests for the marker/indent/truncation helpers in [`super::helpers`] and
//! the gutter-width invariant of [`super::glyphs`].

use ratatui::style::{Color, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::glyphs::{
    ASSISTANT_MARKER, MARKER_COLS, TEAMMATE_MESSAGE_GLYPH, THINKING_GLYPH, TOOL_RESULT_GLYPH,
    USER_MARKER,
};
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
    // The gutter markers MUST measure as a single column, because every line's
    // body budget is computed as `width - MARKER_COLS`. If a marker measured
    // >1 here, each transcript line would be one column short and its last
    // char would bleed past the panel edge.
    //
    // Measuring 1 is not the same as *rendering* as 1: `⏺`/`⎿`/`✻` are drawn
    // two columns wide by many terminals and fonts. That gap is not closed
    // here (an earlier comment claimed a U+FE0E text-presentation suffix did
    // it — no such suffix exists on these constants). It is handled instead by
    // leaving the cell after the glyph unwritten so the body is positioned
    // absolutely; see `glyphs::WIDTH_AMBIGUOUS_GLYPHS` and
    // `build::width_risk_hole`.
    for (name, m) in [
        ("assistant", ASSISTANT_MARKER),
        ("tool-result", TOOL_RESULT_GLYPH),
        ("thinking", THINKING_GLYPH),
        ("user", USER_MARKER),
        ("teammate-message", TEAMMATE_MESSAGE_GLYPH),
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

#[test]
fn truncate_keeps_a_string_that_fits_exactly() {
    // Used to reserve the ellipsis column unconditionally and return "hell…".
    assert_eq!(truncate_to_width("hello", 5), "hello");
}

#[test]
fn truncate_does_not_cut_inside_a_cluster() {
    // Cutting between `⚠` and its U+FE0F selector would leave a dangling
    // combining mark and mis-measure the result.
    let s = "\u{26a0}\u{fe0f}\u{26a0}\u{fe0f}\u{26a0}\u{fe0f}";
    let out = truncate_to_width(s, 5);
    assert!(UnicodeWidthStr::width(out.as_str()) <= 5, "{out:?}");
    assert!(!out.contains('\u{fe0f}') || out.contains('\u{26a0}'));
}
