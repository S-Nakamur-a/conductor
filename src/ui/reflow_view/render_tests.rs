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

use super::build::{BuildCtx, MAX_GUTTER_GLYPH_COL, build_lines};
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

/// A `⏺`/`⎿`/`✻` in **body text** is content, not a gutter marker. The scan
/// used to run the whole line, so a transcript quoting Claude Code output —
/// which this app's own transcripts do constantly — had a cell blanked in the
/// middle of a sentence: an unwritten cell is never painted, so the character
/// vanished and whatever the previous frame drew there stayed put.
#[test]
fn a_glyph_in_body_text_gets_no_hole() {
    // Long enough that the `⏺` lands on a continuation line, which carries a
    // blank indent instead of a marker — so nothing else on it wants a hole.
    let text = format!("{} \u{23fa} tail", "word ".repeat(20));
    let holes = build(
        &[entry(Role::Assistant, vec![DisplayBlock::Text(text)])],
        false,
    );
    // The first line legitimately has one (its own `⏺` marker at column 0);
    // no other line may.
    assert_eq!(holes.first().copied().flatten(), Some(1), "{holes:?}");
    assert!(
        holes.iter().skip(1).all(Option::is_none),
        "a glyph in body text punched a hole into content: {holes:?}"
    );
}

/// The hole belongs to the gutter glyph and must not move when the body holds
/// characters whose width is not 1 — full-width CJK, ZWJ sequences, variation
/// selectors, skin-tone modifiers, combining marks.
///
/// Note what this does *not* prove: the scan now stops at the gutter, so the
/// per-`char` → per-grapheme change in `width_risk_hole` is not independently
/// observable here (the gutter is ASCII spaces plus the marker, where the two
/// agree). It is kept for consistency with `helpers::truncate_to_width` and
/// `user_text::wrap_plain_text`; this test pins the invariant those two
/// together are meant to preserve.
#[test]
fn wide_and_multi_char_clusters_do_not_shift_the_hole() {
    // `⎿` sits at column 2 of a `  ⎿  ` prefix regardless of what follows it,
    // so the hole is at column 3 for every one of these bodies.
    for body in [
        "plain",
        "日本語の全角テキスト",                            // width-2 CJK
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} family", // ZWJ, 7 chars / 2 cols
        "\u{26a0}\u{fe0f} warn",                          // emoji-presentation selector
        "\u{1f44b}\u{1f3fd} wave",                        // skin-tone modifier
        "e\u{0301}\u{0301} combining",                    // combining marks
    ] {
        let holes = build(
            &[entry(
                Role::User,
                vec![DisplayBlock::ToolResult {
                    kind: crate::claude_log::ResultKind::Inline,
                    lines: vec![body.to_string()],
                    is_error: false,
                }],
            )],
            true,
        );
        assert_eq!(
            holes.first().copied().flatten(),
            Some(3),
            "hole moved off the gutter glyph for body {body:?}: {holes:?}"
        );
    }
}

/// The hole must land on a cell the row does not otherwise use, and every
/// built line must still fit the panel — the two invariants the mechanism
/// exists to protect, checked together on wide-character content.
#[test]
fn holes_stay_inside_the_line_for_wide_content() {
    use unicode_width::UnicodeWidthStr;

    let theme = crate::theme::Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    let cache = MarkdownCache::new();
    let entries = vec![
        entry(
            Role::User,
            vec![DisplayBlock::Text(
                "日本語の全角テキストと絵文字 \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} と \u{26a0}\u{fe0f}".into(),
            )],
        ),
        entry(
            Role::Assistant,
            vec![DisplayBlock::Text(
                "全角 日本語日本語日本語日本語日本語 and \u{1f600} tail".into(),
            )],
        ),
    ];
    for width in [20usize, 40, 60, 80] {
        let ctx = BuildCtx {
            entries: &entries,
            cache: &cache,
            theme: &theme,
            syntax_set: &syntax_set,
            syntect_theme: &syntect_theme,
            expanded: true,
        };
        let built = build_lines(&ctx, width);
        for (line, meta) in built.lines.iter().zip(built.meta.iter()) {
            let w: usize = line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(w <= width, "line overflows at width {width}: {w} cols");
            if let Some(col) = meta.skip_col {
                assert!(
                    (col as usize) <= MAX_GUTTER_GLYPH_COL + 1,
                    "hole at column {col} is past the gutter (width {width})"
                );
            }
        }
    }
}
// ── Detached badge ──────────────────────────────────────────────────────────
//
// The badge is the only signal that the view is not showing the newest turn,
// and the only pointer route back to it, so both its presence rule and its
// reported hit region are asserted rather than eyeballed.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

use super::render::{JUMP_BADGE_LABELS, render_jump_badge};

/// Draw the badge into a `width x height` frame and return what it reported
/// plus the rendered screen.
fn draw_badge(width: u16, height: u16, following: bool) -> (Option<Rect>, Buffer) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut hit = None;
    terminal
        .draw(|f| {
            hit = render_jump_badge(f, Rect::new(0, 0, width, height), following);
        })
        .unwrap();
    (hit, terminal.backend().buffer().clone())
}

fn screen_text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Following the newest turn is the quiet state: no badge, and — just as
/// importantly — no hit region, or a click on that spot would keep firing.
#[test]
fn no_badge_while_following() {
    let (hit, buf) = draw_badge(40, 6, true);
    assert_eq!(hit, None);
    assert!(
        !screen_text(&buf).contains("(G)"),
        "{}",
        screen_text(&buf)
    );
}

#[test]
fn detached_draws_the_badge_bottom_right_and_reports_its_rect() {
    let (hit, buf) = draw_badge(40, 6, false);
    let rect = hit.expect("detached view must offer a way back");

    // Bottom-right corner, flush with the right edge.
    assert_eq!(rect.y, 5, "badge belongs on the last row");
    assert_eq!(rect.x + rect.width, 40, "badge is right-aligned");
    assert_eq!(rect.height, 1);

    // The reported rect is where the text actually is — this is the contract
    // the click handler relies on.
    let text = screen_text(&buf);
    let last_row = text.lines().last().unwrap();
    assert!(last_row.contains("Jump to latest (G)"), "{last_row:?}");
    assert_eq!(
        rect.width as usize,
        UnicodeWidthStr::width(JUMP_BADGE_LABELS[0])
    );
}

/// The badge steps down through shorter labels rather than being truncated
/// into something unreadable, and disappears entirely when even the shortest
/// would not fit — the Claude column can be narrow.
#[test]
fn badge_shrinks_with_the_panel_and_eventually_gives_up() {
    // Wide enough for the full label (20 cols + 1 of slack).
    assert_eq!(draw_badge(21, 3, false).0.map(|r| r.width), Some(20));
    // One column short of it: falls back to " Latest (G) " (12).
    assert_eq!(draw_badge(20, 3, false).0.map(|r| r.width), Some(12));
    // Only room for " (G) " (5).
    assert_eq!(draw_badge(12, 3, false).0.map(|r| r.width), Some(5));
    // Not even that.
    assert_eq!(draw_badge(5, 3, false).0, None);
}

/// Every label must measure exactly what `unicode-width` says, because the
/// badge is positioned against the panel's right edge — a glyph the terminal
/// draws wider would push its tail onto the border.
#[test]
fn badge_labels_are_plain_ascii() {
    for label in JUMP_BADGE_LABELS {
        assert!(
            label.is_ascii(),
            "{label:?} is not ASCII; see the note on JUMP_BADGE_LABELS"
        );
        assert_eq!(UnicodeWidthStr::width(label), label.len());
    }
}

/// Labels are ordered longest-first; `render_jump_badge` picks the first that
/// fits, so a mis-ordered list would silently prefer a shorter one.
#[test]
fn badge_labels_are_ordered_longest_first() {
    let widths: Vec<usize> = JUMP_BADGE_LABELS
        .iter()
        .map(|l| UnicodeWidthStr::width(*l))
        .collect();
    assert!(
        widths.windows(2).all(|w| w[0] > w[1]),
        "labels must strictly shrink, got {widths:?}"
    );
}

// ── Scroll placement across a reflow ────────────────────────────────────────
//
// The pure arithmetic is covered in `event::reflow`; these run the real line
// builder at two widths and check the two outcomes that matter to a reader:
// a detached one stays on their line, a following one stays on the newest.

use crate::event::reflow::{at_bottom, scroll_after_reflow};

use super::build::BuiltLines;
use super::render::anchor_index;

/// Prose long enough that the wrap positions genuinely differ between the two
/// widths under test — the fixture's whole job.
fn reflow_fixture() -> Vec<LogEntry> {
    // Long enough that a 20-row viewport is a small slice of it — with a short
    // log, "three quarters down" is already the bottom and the clamp, not the
    // anchor, would decide where the reader lands.
    (0..40)
        .map(|i| {
            entry(
                if i % 2 == 0 { Role::Assistant } else { Role::User },
                vec![DisplayBlock::Text(format!(
                    "Turn {i}: {}",
                    "the quick brown fox jumps over the lazy dog ".repeat(4)
                ))],
            )
        })
        .collect()
}

fn build_at(entries: &[LogEntry], width: usize) -> BuiltLines {
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
        expanded: false,
    };
    build_lines(&ctx, width)
}

/// The reported bug, end to end: a reader parked in the history must come out
/// of a width change looking at the same turn — not at the newest one, and not
/// at whatever unrelated text inherited their old line number.
#[test]
fn narrowing_keeps_a_detached_reader_on_the_same_turn() {
    const INNER: usize = 20;
    let entries = reflow_fixture();

    let before = build_at(&entries, 80);
    // Three quarters in: far enough down that the extra wrapped lines the
    // narrower width introduces have accumulated into real drift.
    let scroll = before.meta.len() * 3 / 4;
    let anchor = before.meta[scroll];

    let after = build_at(&entries, 50);
    assert!(
        after.lines.len() > before.lines.len(),
        "fixture must wrap into more lines at the narrower width"
    );

    // Carrying the raw line number across is the behaviour being replaced —
    // assert it would actually have been wrong, or this test proves nothing.
    let naive = after.meta[scroll];
    assert_ne!(
        (naive.entry, naive.block, naive.offset),
        (anchor.entry, anchor.block, anchor.offset),
        "fixture no longer renumbers lines; the test would pass vacuously"
    );

    let placed = scroll_after_reflow(
        false,
        Some(anchor_index(&after.meta, anchor)),
        scroll,
        after.lines.len(),
        INNER,
    );
    let landed = after.meta[placed];
    assert_eq!(
        (landed.entry, landed.block),
        (anchor.entry, anchor.block),
        "reader was moved off their turn: {anchor:?} -> {landed:?}"
    );
    assert!(
        !at_bottom(placed, after.lines.len(), INNER),
        "a reader in the middle of the log must not end up at the live tail"
    );
}

/// The other half: someone riding the newest turn must still be riding it, or
/// the fix would have traded one broken case for another.
#[test]
fn narrowing_keeps_a_follower_on_the_newest_turn() {
    const INNER: usize = 20;
    let entries = reflow_fixture();

    let before = build_at(&entries, 80);
    let scroll = before.lines.len().saturating_sub(INNER);
    assert!(at_bottom(scroll, before.lines.len(), INNER));
    let anchor = before.meta[scroll];

    let after = build_at(&entries, 50);
    let placed = scroll_after_reflow(
        true,
        Some(anchor_index(&after.meta, anchor)),
        scroll,
        after.lines.len(),
        INNER,
    );

    assert!(at_bottom(placed, after.lines.len(), INNER));
    assert_eq!(
        placed + INNER,
        after.lines.len(),
        "the last built line must sit on the last visual row"
    );
    // And the anchor alone would have left the newest lines off-screen — which
    // is exactly why `follow` overrides it.
    let anchored_only = anchor_index(&after.meta, anchor);
    assert!(
        anchored_only < placed,
        "fixture does not exercise the follow override"
    );
}

/// Growing the panel is the mirror case: fewer wrapped lines, and the follower
/// must not be left short of the end.
#[test]
fn widening_keeps_a_follower_on_the_newest_turn() {
    const INNER: usize = 20;
    let entries = reflow_fixture();

    let before = build_at(&entries, 50);
    let scroll = before.lines.len().saturating_sub(INNER);
    let anchor = before.meta[scroll];

    let after = build_at(&entries, 100);
    assert!(after.lines.len() < before.lines.len());

    let placed = scroll_after_reflow(
        true,
        Some(anchor_index(&after.meta, anchor)),
        scroll,
        after.lines.len(),
        INNER,
    );
    assert_eq!(placed + INNER, after.lines.len());
}
