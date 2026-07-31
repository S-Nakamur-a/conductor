//! Public render entry point — draws the transcript into the Claude PTY
//! panel's inner rect, rebuilding the line cache from [`build`](super::build)
//! only when the panel width changes.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::App;

use super::build::{BuildCtx, LineMeta, build_lines};
use super::helpers::truncate_to_width;
use super::palette;

/// Render the reflow transcript view into `area` (the Claude panel's inner rect).
///
/// `app` is taken as `&mut` so the render can write updated scroll / cache state
/// back to `app.reflow` after building or scrolling the line list.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    // Any path that returns before the badge is drawn must retract its hit
    // region, or a click would keep landing on a chip that is no longer there.
    app.reflow.jump_hit = None;
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Clear so a partially-filled transcript never lets the previous frame's
    // text show through (the scrollback-bleed fix, as in the live PTY render).
    frame.render_widget(ratatui::widgets::Clear, area);

    // An overlay that covered this panel painted over the cells we
    // deliberately leave unwritten; once it closes, only a hard repaint can
    // clear them (ratatui's diff compares its own buffers, which agree).
    let overlay_active = app.is_any_overlay_active();
    if app.reflow.last_overlay_active && !overlay_active {
        app.terminal.needs_clear = true;
    }
    app.reflow.last_overlay_active = overlay_active;

    // The full panel width is used. This view used to build one column
    // narrower as a safety gutter, on the theory that a glyph the terminal
    // draws wider than `unicode-width` counts would push a line's last
    // character past the panel edge. That cost a permanent one-column
    // disagreement with Claude Code's own wrap positions on *every* line, to
    // insure against a rare one. The gutter glyphs — the actual source of the
    // original bleed — are now positioned absolutely instead (see
    // `super::build::width_risk_hole`), and for stray wide content in the body
    // the damage is bounded: rows are re-anchored individually, so an overflow
    // cannot propagate past its own row, line wrap is disabled
    // (`main.rs`'s `DisableLineWrap`) and this is the rightmost column
    // (`ui::layout::cache`), so at worst one cell of this panel's own right
    // border is overdrawn.
    let render_area = area;
    let inner_width = render_area.width as usize;
    let inner_height = render_area.height as usize;

    // ── Loading placeholder ───────────────────────────────────────────────────
    // The session log is parsed on a background thread (see `open_reflow`);
    // until the entries arrive, show a centered placeholder instead of an
    // empty transcript. The entry sweep keeps animating over it, so the
    // border transition doubles as the loading indicator.
    if app.reflow.loading {
        let msg = "Loading transcript\u{2026}";
        let y = area.y + area.height / 2;
        let msg_cols = UnicodeWidthStr::width(msg).min(inner_width) as u16;
        let x = render_area.x + (render_area.width.saturating_sub(msg_cols)) / 2;
        let line = Line::from(Span::styled(
            truncate_to_width(msg, inner_width),
            Style::default().fg(palette::INACTIVE),
        ));
        frame.render_widget(Paragraph::new(line), Rect::new(x, y, msg_cols, 1));
        return;
    }

    // ── (Re)build cached lines when the panel width or expand state changed ──
    // `anchored` carries the rebuild's answer to "where did the top line go?"
    // down to the single scroll decision below; it stays `None` on the frames
    // where nothing was rebuilt.
    let mut anchored: Option<usize> = None;
    if app.reflow.last_width != render_area.width || app.reflow.needs_rebuild {
        // Block-scoped so the shared borrows on `app` drop before the
        // `cached_lines` / `total_lines` / `last_width` assignments below.
        let built = {
            let ctx = BuildCtx {
                entries: &app.reflow.entries,
                cache: &app.reflow.cache,
                theme: &app.theme,
                syntax_set: &app.syntax_set,
                syntect_theme: &app.syntect_theme,
                expanded: app.reflow.expanded,
            };
            build_lines(&ctx, inner_width)
        };
        // Remember *what* was at the top of the viewport before the rebuild.
        // `scroll` is a raw line index, and a width change (or the expand
        // toggle, which changes line counts wholesale) makes that index mean
        // something else entirely — the view would jump. Re-finding the same
        // logical position keeps the reader where they were.
        let anchor = app.reflow.line_meta.get(app.reflow.scroll).copied();
        let total = built.lines.len();

        app.reflow.cached_lines = built.lines;
        app.reflow.line_meta = built.meta;
        app.reflow.total_lines = total;
        app.reflow.last_width = render_area.width;
        app.reflow.needs_rebuild = false;
        // Line positions all moved; repaint physically so no unwritten cell
        // keeps a glyph from the previous layout.
        app.terminal.needs_clear = true;

        anchored = anchor.map(|a| anchor_index(&app.reflow.line_meta, a));
    }

    app.reflow.last_inner_height = area.height;

    // ── Place the viewport ────────────────────────────────────────────────────
    // One decision for every way the geometry can move under the reader —
    // width (rebuild above), height, and the expand toggle. A follower is
    // re-pinned to the newest line; anyone parked in the history is put back on
    // the line the anchor resolved to, never dragged to the tail. The result is
    // clamped, so the upper bound is total - inner_height rather than total - 1:
    // at the logical bottom the last content line sits on the last visual row,
    // and a log shorter than the panel collapses to 0 with no blank rows.
    app.reflow.scroll = crate::event::reflow::scroll_after_reflow(
        app.reflow.follow,
        anchored,
        app.reflow.scroll,
        app.reflow.total_lines,
        inner_height,
    );

    let scroll = app.reflow.scroll;

    // ── Transition completion ─────────────────────────────────────────────────
    // Drive the entry transition timer: once the entry sweep elapses, clear it
    // so the border rests on the steady read-mode color. The border color
    // transition itself is painted in `terminal_claude::render`, not here.
    let entry_done = app.reflow.sweep.as_ref().is_some_and(|s| {
        crate::event::reflow::sweep_progress(&s.start, crate::event::reflow::TRANSITION_DURATION_MS)
            >= 1.0
    });
    if entry_done {
        app.reflow.sweep = None;
    }

    // Blit the visible window straight from the cache by reference — no
    // per-frame clone of the line vector. No wrapping: `markdown_cache.render`
    // already produces lines ≤ `body_width` columns, and each line then
    // receives a `MARKER_COLS`-wide prefix, keeping total width ≤
    // `render_area.width`.  1 logical line == 1 visual row means scroll
    // arithmetic is exact with no invisible over-height rows.
    let buf = frame.buffer_mut();
    let rows = app
        .reflow
        .cached_lines
        .iter()
        .zip(app.reflow.line_meta.iter())
        .skip(scroll)
        .take(inner_height);
    for (i, (line, meta)) in rows.enumerate() {
        let y = render_area.y + i as u16;
        buf.set_line(render_area.x, y, line, render_area.width);
        // Leave one cell unwritten after a width-ambiguous gutter glyph. The
        // diff then skips it, so the next cell is not contiguous with the
        // glyph's and the crossterm backend emits an absolute `MoveTo` before
        // the body — pinning it to the right column however wide the terminal
        // actually drew the glyph. `skip` is cleared by `Buffer::reset` every
        // frame, so this has to be re-applied on each one.
        if let Some(col) = meta.skip_col
            && col < render_area.width
            && let Some(cell) = buf.cell_mut((render_area.x + col, y))
        {
            cell.set_skip(true);
        }
    }

    app.reflow.jump_hit = render_jump_badge(frame, render_area, app.reflow.follow);
}

/// Text of the detached badge at each width tier, longest first. Deliberately
/// ASCII: the badge is positioned against the panel's right edge, so a glyph
/// the terminal draws wider than `unicode-width` counts (the `⏺`/`⎿` problem
/// the gutter solves with an unwritten cell) would push its tail onto the
/// border. There is no arrow here for that reason.
pub(super) const JUMP_BADGE_LABELS: [&str; 3] = [" Jump to latest (G) ", " Latest (G) ", " (G) "];

/// Draw the "you are not at the newest turn" badge, returning its screen rect
/// so a click can be routed back to it.
///
/// Only visible while detached — that *is* the feedback. Following, the badge
/// is absent and the function reports no hit region, so a stale rect can never
/// keep swallowing clicks after the reader returns to the tail.
///
/// It is deliberately the quietest thing on screen that is still a target:
/// one right-aligned chip on the last row, in the transcript's own dim grey on
/// the user-turn block's background. Anything louder would compete with the
/// transcript for attention on every scroll-up, which is the common case.
pub(super) fn render_jump_badge(frame: &mut Frame, area: Rect, following: bool) -> Option<Rect> {
    if following || area.height == 0 {
        return None;
    }
    // Widest label that fits, leaving one column of slack against the panel
    // edge; if even the shortest needs more room than the panel has, the badge
    // is dropped rather than truncated into something unreadable.
    let label = JUMP_BADGE_LABELS
        .iter()
        .find(|l| UnicodeWidthStr::width(**l) < area.width as usize)?;
    let w = UnicodeWidthStr::width(*label) as u16;

    let rect = Rect::new(
        area.x + area.width - w,
        area.y + area.height - 1,
        w,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            *label,
            Style::default()
                .fg(palette::INACTIVE)
                .bg(palette::USER_BG),
        ))),
        rect,
    );
    Some(rect)
}

/// Index of the line matching `anchor` after a rebuild — the first line whose
/// `(entry, block, offset)` is at or past the anchor's, so a block that got
/// shorter (or vanished) lands on whatever now occupies that position rather
/// than scrolling somewhere unrelated.
pub(super) fn anchor_index(meta: &[LineMeta], anchor: LineMeta) -> usize {
    let key = (anchor.entry, anchor.block, anchor.offset);
    meta.iter()
        .position(|m| (m.entry, m.block, m.offset) >= key)
        .unwrap_or_else(|| meta.len().saturating_sub(1))
}
