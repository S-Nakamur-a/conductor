//! Reflow transcript view — read-only, word-wrapped rendering of a Claude Code
//! session log inside the Claude PTY panel.
//!
//! `render` is called from `terminal_claude::render` whenever `app.reflow.active`
//! is true.  It maintains a `cached_lines` vector inside `app.reflow` and
//! rebuilds it only when the panel width changes, so there is no per-frame
//! re-parse of the `.jsonl` file or re-invocation of the Markdown renderer.
//!
//! ## Layout grammar
//!
//! Each conversation block is rendered in a two-column gutter layout:
//!
//! ```text
//! ⏺ assistant text line 1
//!   continuation line 2
//! ⏺ Bash(cargo build)
//!   ⎿  12 lines
//! > user text line 1
//!   continuation line 2
//! ```
//!
//! The gutter (`MARKER_COLS = 2`) is always 2 display columns: marker glyph
//! padded to 2 cols for the first line, two spaces for continuations.
//! Markdown content is rendered at `width - MARKER_COLS` so the combined width
//! is exactly `width`, preserving the "1 logical line = 1 visual row" invariant.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, SweepDir};
use crate::claude_log::{DisplayBlock, Role};

// ── Glyph constants ───────────────────────────────────────────────────────────

/// Display columns reserved for the left-hand marker gutter.
const MARKER_COLS: usize = 2;

/// Bullet/record marker for assistant messages and tool invocations (U+23FA ⏺).
const ASSISTANT_MARKER: &str = "\u{23fa}";

/// Corner glyph for tool result lines (U+23BF ⎿).
const TOOL_RESULT_GLYPH: &str = "\u{23bf}";

// ── Public render entry point ─────────────────────────────────────────────────

/// Render the reflow transcript view into `area` (the Claude panel's inner rect).
///
/// `app` is taken as `&mut` so the render can write updated scroll / cache state
/// back to `app.reflow` after building or scrolling the line list.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let inner_width = area.width as usize;
    let inner_height = area.height as usize;

    // ── (Re)build cached lines when the panel width changed ──────────────────
    if app.reflow.last_width != area.width {
        let new_lines = build_lines(app, inner_width);
        let total = new_lines.len();

        app.reflow.cached_lines = new_lines;
        app.reflow.total_lines = total;
        app.reflow.last_width = area.width;
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

    // ── Slice visible lines ───────────────────────────────────────────────────
    let scroll = app.reflow.scroll;
    let visible: Vec<Line<'static>> = app
        .reflow
        .cached_lines
        .iter()
        .skip(scroll)
        .take(inner_height)
        .cloned()
        .collect();

    // ── Transition completion ─────────────────────────────────────────────────
    // Drive the entry/exit transition timer. `SweepDir` is Copy so we snapshot
    // direction and progress before dropping the borrow on `app.reflow`, then
    // apply any completion side-effects. The border color transition is painted
    // in `terminal_claude::render`, not here.
    let transition_state = app.reflow.sweep.as_ref().map(|s| {
        (
            s.dir,
            crate::event::reflow::sweep_progress(
                &s.start,
                crate::event::reflow::TRANSITION_DURATION_MS,
            ),
        )
    });

    if let Some((dir, p)) = transition_state
        && p >= 1.0
    {
        match dir {
            SweepDir::In => {
                app.reflow.sweep = None;
            }
            SweepDir::Out => {
                // close_reflow sets active=false; the next frame renders
                // the live PTY instead of this view.
                app.close_reflow();
            }
        }
    }

    // No .wrap(): `markdown_cache.render` already produces lines ≤ `body_width`
    // columns, and each line then receives a `MARKER_COLS`-wide prefix, keeping
    // total width ≤ `area.width`.  1 logical line == 1 visual row means scroll
    // arithmetic is exact with no invisible over-height rows.
    frame.render_widget(Paragraph::new(visible), area);
}

// ── Line builder ─────────────────────────────────────────────────────────────

/// Rebuild the full `Vec<Line<'static>>` from `app.reflow.entries`.
///
/// Called only when the panel width changes.  Uses an `Rc` clone of the
/// entries (refcount bump only, no deep copy) to release the immutable borrow
/// on `app.reflow` before calling `app.reflow.cache.render`, which also needs
/// `&app.reflow.cache` (another field of `app.reflow`).
fn build_lines(app: &mut App, width: usize) -> Vec<Line<'static>> {
    // Rc clone: O(1), releases the borrow on app.reflow.entries.
    let entries = std::rc::Rc::clone(&app.reflow.entries);

    // Cache theme colors up front (Color is Copy); this lets us call
    // app.reflow.cache.render() later without conflicting borrows.
    let style_assistant = Style::default().fg(app.theme.success);
    let style_user = Style::default().fg(app.theme.muted);
    let style_name = Style::default().fg(app.theme.info);
    let style_dim = Style::default()
        .fg(app.theme.muted)
        .add_modifier(Modifier::DIM);
    let style_thinking = Style::default()
        .fg(app.theme.muted)
        .add_modifier(Modifier::DIM | Modifier::ITALIC);

    // Content width after reserving the marker gutter.
    let body_width = width.saturating_sub(MARKER_COLS);

    let mut all_lines: Vec<Line<'static>> = Vec::new();

    for (ei, entry) in entries.iter().enumerate() {
        let is_user = entry.role == Role::User;

        // ── Content blocks (no role header; marker glyphs carry the role) ────
        for (bi, block) in entry.blocks.iter().enumerate() {
            match block {
                DisplayBlock::Text(text) => {
                    let key = format!("{ei}:{bi}");
                    // app.reflow.cache and app.theme/syntax_set/syntect_theme are
                    // disjoint fields — shared borrows of different struct members
                    // are fine simultaneously in Rust 2021+ (NLL).
                    let md_lines = app.reflow.cache.render(
                        &key,
                        text,
                        body_width,
                        &app.theme,
                        &app.syntax_set,
                        &app.syntect_theme,
                    );
                    let (glyph, marker_style) = if is_user {
                        (">", style_user)
                    } else {
                        (ASSISTANT_MARKER, style_assistant)
                    };
                    all_lines.extend(with_marker(md_lines, glyph, marker_style));
                }
                DisplayBlock::ToolUse { name, summary } => {
                    // ⏺ Name(summary) — single line, marker width + body truncated.
                    let marker_prefix = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
                    let remaining = width.saturating_sub(MARKER_COLS);
                    let line = if summary.is_empty() {
                        // ⏺ Name
                        let name_display = truncate_to_width(name, remaining);
                        Line::from(vec![
                            Span::styled(marker_prefix, style_assistant),
                            Span::styled(name_display, style_name),
                        ])
                    } else {
                        // ⏺ Name(summary)
                        let name_cols = name.width();
                        // Budget for summary: remaining minus name cols and two parens.
                        let summary_budget = remaining.saturating_sub(name_cols + 2);
                        let summary_display = truncate_to_width(summary, summary_budget);
                        Line::from(vec![
                            Span::styled(marker_prefix, style_assistant),
                            Span::styled(name.clone(), style_name),
                            Span::styled(format!("({summary_display})"), style_dim),
                        ])
                    };
                    all_lines.push(line);
                }
                DisplayBlock::ToolResult { lines } => {
                    // "  ⎿  {n} lines" — 2-space indent + corner glyph + 2 spaces + count.
                    let result_str = format!("  {TOOL_RESULT_GLYPH}  {lines} lines");
                    let result_display = truncate_to_width(&result_str, width);
                    all_lines.push(Line::from(vec![Span::styled(result_display, style_dim)]));
                }
                DisplayBlock::Thinking => {
                    // ⏺ Thinking… — italic dim body, assistant-colored marker.
                    let marker_prefix = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
                    all_lines.push(Line::from(vec![
                        Span::styled(marker_prefix, style_assistant),
                        Span::styled("Thinking\u{2026}", style_thinking),
                    ]));
                }
            }
        }

        // ── Blank separator between entries ──────────────────────────────────
        all_lines.push(Line::from(""));
    }

    all_lines
}

// ── Marker/indent helpers (pure, testable independently of App) ───────────────

/// Prepend a `MARKER_COLS`-wide marker to the first line of `lines` and a
/// same-width blank indent to all continuation lines.
///
/// `glyph` is measured with `unicode_width` and padded with spaces to exactly
/// `MARKER_COLS` display columns before being inserted as the leading span.
/// Content spans on each line keep their original styling.
fn with_marker(
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
fn pad_glyph_to(glyph: &str, target_cols: usize) -> String {
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
fn truncate_to_width(s: &str, max_cols: usize) -> String {
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

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
}
