//! Tests for the gutter-glyph width defence.
//!
//! Two independent halves, because either one alone would be misleading:
//!
//! * [`marks_the_hole`] — the builder marks the right cell.
//! * [`skipping_forces_an_absolute_move`] — marking that cell actually makes
//!   the real crossterm backend emit an absolute cursor move.
//!
//! Note what these *cannot* show: whether a terminal draws `⏺` one column or
//! two. `ratatui::buffer::Buffer::set_stringn` measures with the same
//! `unicode-width` crate the builder does, so no in-process test can escape
//! that model. The point of the mechanism is that it makes the answer
//! irrelevant — the body is positioned absolutely either way — but confirming
//! it *looks* right still needs a human at a real terminal.

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use syntect::highlighting::ThemeSet;

use crate::claude_log::{DisplayBlock, LogEntry, Role};
use crate::ui::markdown::MarkdownCache;

use super::build::{BuildCtx, build_lines};
use super::glyphs::{ASSISTANT_MARKER, TOOL_RESULT_GLYPH};

/// Render `prev` → `next` through a real `CrosstermBackend` and return the
/// bytes it wrote.
fn flush(prev: &Buffer, next: &Buffer) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut backend = CrosstermBackend::new(&mut out);
        backend.draw(prev.diff(next).into_iter()).unwrap();
    }
    out
}

fn one_row(glyph: &str, skip: bool) -> Buffer {
    let mut b = Buffer::empty(Rect::new(0, 0, 20, 1));
    b[(0u16, 0u16)].set_symbol(glyph);
    b[(1u16, 0u16)].set_symbol(" ");
    b[(1u16, 0u16)].set_skip(skip);
    for (i, c) in "hello".chars().enumerate() {
        b[(2 + i as u16, 0u16)].set_char(c);
    }
    b
}

/// Skipping the cell after the glyph must make the backend jump to column 3
/// (1-based) absolutely, instead of writing straight on from the glyph. The
/// non-skipped case is asserted too: without it this test would still pass if
/// `MoveTo` were emitted unconditionally, proving nothing about `set_skip`.
#[test]
fn skipping_forces_an_absolute_move() {
    // The previous frame must *differ* at the cell under test. Diffing two
    // blank buffers would leave column 1 unchanged, so the diff would omit it
    // and emit the absolute move anyway — the control case has to actually
    // have something to overwrite for it to be a control at all.
    let mut prev = Buffer::empty(Rect::new(0, 0, 20, 1));
    prev[(1u16, 0u16)].set_char('X');

    let without = flush(&prev, &one_row(ASSISTANT_MARKER, false));
    let with = flush(&prev, &one_row(ASSISTANT_MARKER, true));

    const ABSOLUTE_MOVE_TO_COL3: &[u8] = b"\x1b[1;3H";
    assert!(
        !without.windows(6).any(|w| w == ABSOLUTE_MOVE_TO_COL3),
        "control case should write contiguously, got {:?}",
        String::from_utf8_lossy(&without)
    );
    assert!(
        with.windows(6).any(|w| w == ABSOLUTE_MOVE_TO_COL3),
        "skipped cell should force an absolute move, got {:?}",
        String::from_utf8_lossy(&with)
    );
}

/// A skipped cell is never erased either — which is why every path that can
/// leave foreign content under one has to force a hard repaint (see
/// `App::open_reflow` and `super::render`).
#[test]
fn skipped_cell_is_not_repainted() {
    let mut prev = Buffer::empty(Rect::new(0, 0, 20, 1));
    prev[(1u16, 0u16)].set_char('X');

    let bytes = flush(&prev, &one_row(ASSISTANT_MARKER, true));

    assert!(
        !String::from_utf8_lossy(&bytes).contains('X'),
        "stale cell is left alone, so nothing overwrites it"
    );
}

fn build(entries: &[LogEntry], expanded: bool) -> Vec<Option<u16>> {
    let theme = crate::theme::Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    let cache = MarkdownCache::new();
    let ctx = BuildCtx {
        entries,
        cache: &cache,
        theme: &theme,
        syntax_set: &syntax_set,
        syntect_theme: &syntect_theme,
        expanded,
    };
    build_lines(&ctx, 60)
        .meta
        .into_iter()
        .map(|m| m.skip_col)
        .collect()
}

fn entry(role: Role, blocks: Vec<DisplayBlock>) -> LogEntry {
    LogEntry {
        role,
        model: None,
        blocks,
    }
}

/// The hole goes immediately after the glyph, wherever the glyph sits — which
/// is not always column 0: a `⎿` result line indents it to column 2.
#[test]
fn marks_the_hole() {
    // Assistant prose: `⏺` at col0, so the hole is col1.
    let assistant = build(
        &[entry(
            Role::Assistant,
            vec![DisplayBlock::Text("hello".into())],
        )],
        false,
    );
    assert_eq!(assistant.first().copied().flatten(), Some(1));

    // Expanded tool result: `  ⎿  ` puts the glyph at col2, so the hole is col3.
    let result = build(
        &[entry(
            Role::User,
            vec![DisplayBlock::ToolResult {
                kind: crate::claude_log::ResultKind::Inline,
                lines: vec!["out".into()],
                is_error: false,
            }],
        )],
        true,
    );
    assert_eq!(result.first().copied().flatten(), Some(3));
}

/// A user turn is a full-width background block; an unwritten cell inside it
/// would read as a notch, and `❯` is not width-ambiguous anyway.
#[test]
fn user_turns_get_no_hole() {
    let holes = build(
        &[entry(Role::User, vec![DisplayBlock::Text("hi".into())])],
        false,
    );
    assert!(holes.iter().all(Option::is_none), "{holes:?}");
}

/// Guards the constants themselves: if a glyph were ever swapped for one the
/// hole logic doesn't know about, the defence would silently stop applying.
#[test]
fn every_ambiguous_gutter_glyph_is_registered() {
    for glyph in [ASSISTANT_MARKER, TOOL_RESULT_GLYPH, super::glyphs::THINKING_GLYPH] {
        let ch = glyph.chars().next().unwrap();
        assert!(
            super::glyphs::is_width_ambiguous(ch),
            "{glyph:?} (U+{:04X}) is drawn in the gutter but not registered as width-ambiguous",
            ch as u32
        );
    }
    // …and the two that deliberately are not.
    for glyph in [super::glyphs::USER_MARKER, super::glyphs::TEAMMATE_MESSAGE_GLYPH] {
        let ch = glyph.chars().next().unwrap();
        assert!(!super::glyphs::is_width_ambiguous(ch), "{glyph:?}");
    }
}
