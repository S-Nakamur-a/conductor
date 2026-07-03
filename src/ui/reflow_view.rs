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
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::claude_log::{DisplayBlock, Role};
use crate::theme::Theme;

// ── Glyph constants ───────────────────────────────────────────────────────────

/// Display columns reserved for the left-hand marker gutter.
const MARKER_COLS: usize = 2;

// Gutter markers MUST measure 1 column in `unicode-width` AND render 1 column in
// the terminal, or every transcript line ends up one short and its last char
// spills past the panel edge. Claude Code's glyphs (⏺ ✻ ⎿) are width-Narrow but
// many terminals/fonts render them 2 columns wide (⏺ even carries the Emoji
// property), so the count and the render disagree — the source of the scrollback
// "bleed". We use only glyphs this terminal provably renders at the width we
// count: ASCII for the bullet/prompt (always 1 col) and a box-drawing corner for
// tool results (the panel borders use the same block, so it renders narrow too).
// The host terminal's line-wrap is also disabled (see `enter_tui`) so even a
// wide glyph in message *content* can't wrap into a neighbouring panel.

/// Bullet/record marker for assistant messages and tool invocations.
/// ASCII so it can't be widened by emoji presentation (Claude Code uses ⏺).
const ASSISTANT_MARKER: &str = "*";

/// Prompt marker for user turns (Claude Code shows `>` before user input).
const USER_MARKER: &str = ">";

/// Corner glyph for tool result lines (box-drawing `└`, same block as the panel
/// borders so it renders one column wide; Claude Code uses ⎿).
const TOOL_RESULT_GLYPH: &str = "\u{2514}";

/// Marker for thinking blocks (Claude Code uses ✻; ASCII keeps the width honest).
const THINKING_GLYPH: &str = "*";

// ── Claude Code fixed palette (dark theme) ─────────────────────────────────────
//
// Lifted verbatim from the Claude Code CLI's hardcoded dark theme so the
// transcript reads like the real thing regardless of the user's conductor theme.
// (Conductor's own theme drives every other panel; only this overlay pins the
// Claude palette.)
mod palette {
    use ratatui::style::Color;

    /// Claude's signature coral/orange accent — `claude` token.
    pub const CLAUDE: Color = Color::Rgb(215, 119, 87);
    /// Primary text — `text` token (white).
    pub const TEXT: Color = Color::Rgb(255, 255, 255);
    /// Tool-invocation bullet — `success` token (green).
    pub const SUCCESS: Color = Color::Rgb(78, 186, 101);
    /// Error connector — `error` token (coral red).
    pub const ERROR: Color = Color::Rgb(255, 107, 128);
    /// Dimmed/secondary text — `inactive` token (grey).
    pub const INACTIVE: Color = Color::Rgb(153, 153, 153);
    /// Accent for headings/links — `permission` token (periwinkle).
    pub const PERMISSION: Color = Color::Rgb(177, 185, 249);
    /// Inline-code / very dim — `subtle` token.
    pub const SUBTLE: Color = Color::Rgb(80, 80, 80);
}

/// Build a Claude-flavored [`Theme`] for the Markdown renderer so prose,
/// headings, links and code in the transcript adopt Claude Code's palette
/// instead of the active conductor theme. Only the fields the Markdown
/// renderer consults are overridden; the rest are inherited from `base`.
fn claude_markdown_theme(base: &Theme) -> Theme {
    let mut t = base.clone();
    t.fg = palette::TEXT;
    t.muted = palette::INACTIVE;
    t.hint = palette::INACTIVE;
    t.accent = palette::CLAUDE;
    t.info = palette::PERMISSION;
    t.success = palette::SUCCESS;
    t.error = palette::ERROR;
    t.warning = Color::Rgb(255, 193, 7);
    t.border_secondary = palette::SUBTLE;
    t.code_fg = palette::TEXT;
    t.code_bg = Color::Rgb(43, 43, 43);
    t
}

// ── Public render entry point ─────────────────────────────────────────────────

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

    // Claude's fixed palette (Color is Copy) drives the transcript chrome.
    let style_assistant = Style::default().fg(palette::TEXT);
    let style_user = Style::default().fg(palette::CLAUDE);
    let style_tool_marker = Style::default().fg(palette::SUCCESS);
    let style_name = Style::default()
        .fg(palette::TEXT)
        .add_modifier(Modifier::BOLD);
    let style_args = Style::default().fg(palette::INACTIVE);
    let style_result = Style::default().fg(palette::INACTIVE);
    let style_result_err = Style::default().fg(palette::ERROR);
    let style_thinking = Style::default()
        .fg(palette::INACTIVE)
        .add_modifier(Modifier::ITALIC);

    // A Claude-flavored theme so the Markdown body matches the chrome. Cloned
    // once up front so the loop only borrows the (disjoint) cache/syntax fields.
    let md_theme = claude_markdown_theme(&app.theme);

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
                    // app.reflow.cache and the syntax fields are disjoint members
                    // of `app`, so the simultaneous &mut / & borrows are fine
                    // under NLL. `md_theme` is a local, independent of `app`.
                    let md_lines = app.reflow.cache.render(
                        &key,
                        text,
                        body_width,
                        &md_theme,
                        &app.syntax_set,
                        &app.syntect_theme,
                    );
                    let (glyph, marker_style) = if is_user {
                        (USER_MARKER, style_user)
                    } else {
                        (ASSISTANT_MARKER, style_assistant)
                    };
                    all_lines.extend(with_marker(md_lines, glyph, marker_style));
                }
                DisplayBlock::ToolUse { name, summary } => {
                    // ⏺ Name(summary) — green bullet, bold name, dim args.
                    let marker_prefix = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
                    let remaining = width.saturating_sub(MARKER_COLS);
                    let line = if summary.is_empty() {
                        let name_display = truncate_to_width(name, remaining);
                        Line::from(vec![
                            Span::styled(marker_prefix, style_tool_marker),
                            Span::styled(name_display, style_name),
                        ])
                    } else {
                        let name_cols = name.width();
                        // Budget for summary: remaining minus name cols and two parens.
                        let summary_budget = remaining.saturating_sub(name_cols + 2);
                        let summary_display = truncate_to_width(summary, summary_budget);
                        Line::from(vec![
                            Span::styled(marker_prefix, style_tool_marker),
                            Span::styled(name.clone(), style_name),
                            Span::styled(format!("({summary_display})"), style_args),
                        ])
                    };
                    all_lines.push(line);
                }
                DisplayBlock::ToolResult {
                    preview,
                    total_lines,
                    is_error,
                } => {
                    all_lines.extend(render_tool_result(
                        preview,
                        *total_lines,
                        *is_error,
                        width,
                        style_result,
                        style_result_err,
                    ));
                }
                DisplayBlock::Thinking { text } => {
                    // ✻ Thinking… header, then the (dimmed, italic) reasoning body.
                    let marker_prefix = pad_glyph_to(THINKING_GLYPH, MARKER_COLS);
                    all_lines.push(Line::from(vec![
                        Span::styled(marker_prefix, style_thinking),
                        Span::styled("Thinking\u{2026}", style_thinking),
                    ]));
                    if !text.trim().is_empty() {
                        let key = format!("{ei}:{bi}:think");
                        let md_lines = app.reflow.cache.render(
                            &key,
                            text,
                            body_width,
                            &md_theme,
                            &app.syntax_set,
                            &app.syntect_theme,
                        );
                        // Recolor the Markdown output to dim italic and indent it
                        // under the gutter (blank marker, so no glyph repeats).
                        let dimmed = md_lines
                            .into_iter()
                            .map(|mut line| {
                                for span in &mut line.spans {
                                    span.style = Style::default()
                                        .fg(palette::INACTIVE)
                                        .add_modifier(Modifier::ITALIC);
                                }
                                line
                            })
                            .collect();
                        all_lines.extend(with_marker(dimmed, " ", style_thinking));
                    }
                }
            }
        }

        // ── Blank separator between entries ──────────────────────────────────
        all_lines.push(Line::from(""));
    }

    all_lines
}

/// Render a tool result as Claude Code's collapsed `⎿` block: the first line
/// hangs off the corner glyph, continuations align under it, and any overflow
/// beyond the captured preview collapses into a `… +N lines` summary.
fn render_tool_result(
    preview: &[String],
    total_lines: usize,
    is_error: bool,
    width: usize,
    body_style: Style,
    err_style: Style,
) -> Vec<Line<'static>> {
    // "  ⎿  " — 2-space indent + 1-col glyph + 2 spaces = 5 columns; continuation
    // lines indent by the same amount so output text stays left-aligned.
    let first_prefix = format!("  {TOOL_RESULT_GLYPH}  ");
    let prefix_cols = UnicodeWidthStr::width(first_prefix.as_str());
    let cont_indent = " ".repeat(prefix_cols);
    let connector_style = if is_error { err_style } else { body_style };

    if total_lines == 0 {
        let s = truncate_to_width(&format!("{first_prefix}(no content)"), width);
        return vec![Line::from(vec![Span::styled(s, connector_style)])];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    for (i, raw) in preview.iter().enumerate() {
        let body = truncate_to_width(raw, width.saturating_sub(prefix_cols));
        if i == 0 {
            out.push(Line::from(vec![
                Span::styled(first_prefix.clone(), connector_style),
                Span::styled(body, body_style),
            ]));
        } else {
            out.push(Line::from(vec![
                Span::raw(cont_indent.clone()),
                Span::styled(body, body_style),
            ]));
        }
    }
    if total_lines > preview.len() {
        let more = total_lines - preview.len();
        let s = truncate_to_width(&format!("{cont_indent}\u{2026} +{more} lines"), width);
        out.push(Line::from(vec![Span::styled(s, body_style)]));
    }
    out
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
}
