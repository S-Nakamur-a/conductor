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

use super::build::build_lines;
use super::helpers::truncate_to_width;
use super::palette;

/// Render the reflow transcript view into `area` (the Claude panel's inner rect).
///
/// `app` is taken as `&mut` so the render can write updated scroll / cache state
/// back to `app.reflow` after building or scrolling the line list.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Clear so a partially-filled transcript never lets the previous frame's
    // text show through (the scrollback-bleed fix, as in the live PTY render).
    frame.render_widget(ratatui::widgets::Clear, area);

    // Reserve one column on the right as a safety gutter. Transcript content is
    // arbitrary Unicode, and a glyph the terminal renders one column wider than
    // `unicode-width` counts (an emoji, an emoji-presented symbol) would push a
    // line's last character to the panel edge; building/rendering one column
    // narrower lets that overflow land in this reserved blank column instead.
    let render_area = Rect {
        width: area.width.saturating_sub(1).max(1),
        ..area
    };
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

    // ── (Re)build cached lines when the panel width changed ──────────────────
    if app.reflow.last_width != render_area.width {
        let new_lines = build_lines(app, inner_width);
        let total = new_lines.len();

        app.reflow.cached_lines = new_lines;
        app.reflow.total_lines = total;
        app.reflow.last_width = render_area.width;
    }

    app.reflow.last_inner_height = area.height;

    // ── Pin to bottom on first open ───────────────────────────────────────────
    if app.reflow.pending_bottom {
        app.reflow.scroll = app
            .reflow
            .total_lines
            .saturating_sub(inner_height);
        app.reflow.pending_bottom = false;
    }

    // ── Clamp scroll to valid range ───────────────────────────────────────────
    // Upper bound is total - inner_height, not total - 1: when pinned to the
    // logical bottom, the last content line sits at the last visual row, not
    // one row above.  total < inner_height (short log) collapses to 0 via
    // saturating_sub so the view stays at the top and shows no blank rows.
    app.reflow.scroll =
        crate::event::reflow::clamp_scroll(app.reflow.scroll, app.reflow.total_lines, inner_height);

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
    // arithmetic is exact with no invisible over-height rows. Rendered into
    // `render_area` (one column narrower than `area`) so the reserved safety
    // gutter stays blank.
    let buf = frame.buffer_mut();
    for (i, line) in app
        .reflow
        .cached_lines
        .iter()
        .skip(scroll)
        .take(inner_height)
        .enumerate()
    {
        buf.set_line(
            render_area.x,
            render_area.y + i as u16,
            line,
            render_area.width,
        );
    }
}
