//! Line builder — turns `app.reflow.entries` into the cached
//! `Vec<Line<'static>>` that [`render`](super::render::render) blits each
//! frame. Rebuilt only when the panel width changes.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::claude_log::{DisplayBlock, Role};

use super::glyphs::{ASSISTANT_MARKER, MARKER_COLS, THINKING_GLYPH, TOOL_RESULT_GLYPH, USER_MARKER};
use super::helpers::{pad_glyph_to, truncate_to_width, with_marker};
use super::palette;
use super::palette::claude_markdown_theme;

/// Rebuild the full `Vec<Line<'static>>` from `app.reflow.entries`.
///
/// Called only when the panel width changes.  Uses an `Rc` clone of the
/// entries (refcount bump only, no deep copy) to release the immutable borrow
/// on `app.reflow` before calling `app.reflow.cache.render`, which also needs
/// `&app.reflow.cache` (another field of `app.reflow`).
pub(crate) fn build_lines(app: &mut App, width: usize) -> Vec<Line<'static>> {
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
                    let md_lines = app.reflow.cache.render_flavored(
                        &key,
                        text,
                        body_width,
                        &md_theme,
                        &app.syntax_set,
                        &app.syntect_theme,
                        crate::ui::markdown::MarkdownFlavor::Transcript,
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
                        let md_lines = app.reflow.cache.render_flavored(
                            &key,
                            text,
                            body_width,
                            &md_theme,
                            &app.syntax_set,
                            &app.syntect_theme,
                            crate::ui::markdown::MarkdownFlavor::Transcript,
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
