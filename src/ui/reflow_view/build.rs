//! Line builder — turns a [`BuildCtx`]'s session-log entries into the cached
//! `Vec<Line<'static>>` that [`render`](super::render::render) blits each
//! frame. Rebuilt only when the panel width (or the expand toggle) changes.
//! Independent of `App` so it can be constructed and tested without one.


use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::claude_log::{DisplayBlock, LogEntry, Role};

use super::glyphs::{
    ASSISTANT_MARKER, MARKER_COLS, TEAMMATE_MESSAGE_GLYPH, THINKING_GLYPH, USER_MARKER,
};
use super::helpers::{fit_glyph_line, fit_styled_line, pad_glyph_to, with_marker};
use super::palette;
use super::palette::claude_markdown_theme;
use super::tool_lines::{
    ToolStyles, count_buckets, render_annotation, render_tool_result_collapsed,
    render_tool_result_expanded, render_tool_use,
};
use super::user_text::render_user_text;

/// Everything [`build_lines`] needs to turn a session log into rendered
/// lines, borrowed independently of `App` so the builder can be called (and
/// tested) without constructing one. All fields are shared references —
/// [`crate::ui::markdown::MarkdownCache::render_flavored`] takes `&self`
/// (its cache is a `RefCell` internally), so no field needs `&mut`.
pub(crate) struct BuildCtx<'a> {
    pub entries: &'a [LogEntry],
    pub cache: &'a crate::ui::markdown::MarkdownCache,
    pub theme: &'a crate::theme::Theme,
    pub syntax_set: &'a syntect::parsing::SyntaxSet,
    pub syntect_theme: &'a syntect::highlighting::Theme,
    /// Whether to expand tool_use/tool_result blocks (conductor's own
    /// ctrl+o-equivalent toggle; wired up in S1).
    pub expanded: bool,
}

/// Block index standing for the blank separator line between entries.
pub(crate) const SEPARATOR_BLOCK: usize = usize::MAX;

/// Rightmost column a gutter marker can start at, and so the last column
/// [`width_risk_hole`] scans. Markers are emitted at column 0 by
/// [`helpers::with_marker`](super::helpers::with_marker) and
/// [`fit_glyph_line`](super::helpers::fit_glyph_line), and at column 2 by
/// [`tool_lines`](super::tool_lines)' `"  ⎿  "` / `" ⎿  "` prefixes. Anything
/// past this is body text, where the same characters are content.
pub(crate) const MAX_GUTTER_GLYPH_COL: usize = 2;

/// Text of the `✻` line marking where a `/compact` cut the context (measured
/// verbatim from a resumed transcript). Conductor cannot honour the `ctrl+o`
/// it advertises — the reflow view is the scrollback, and the full history is
/// already above this line rather than behind a keystroke — but the wording is
/// reproduced as-is, since parity with what the user saw on screen is the goal.
const COMPACT_BOUNDARY_TEXT: &str = "Conversation compacted (ctrl+o for history)";

/// Where one rendered line came from, plus the one thing the renderer needs
/// to know about its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMeta {
    /// Index into `ctx.entries`.
    pub entry: usize,
    /// Index of the block within that entry, or [`SEPARATOR_BLOCK`].
    pub block: usize,
    /// Index of this line within its block — the third component of the
    /// scroll anchor, so a rebuild at a different width lands back inside the
    /// same block rather than just at its top.
    pub offset: usize,
    /// Column that must be left *unwritten* so ratatui's diff sees a
    /// discontinuity and the crossterm backend emits an absolute cursor move
    /// before the text that follows. See [`width_risk_hole`].
    pub skip_col: Option<u16>,
}

/// [`build_lines`]'s output: the lines to blit and one [`LineMeta`] each.
pub(crate) struct BuiltLines {
    pub lines: Vec<Line<'static>>,
    pub meta: Vec<LineMeta>,
}

/// Rebuild the full line list from `ctx.entries`.
///
/// Called only when the panel width changes (or the expand toggle flips).
pub(crate) fn build_lines(ctx: &BuildCtx<'_>, width: usize) -> BuiltLines {
    let entries = ctx.entries;

    // Claude's fixed palette (Color is Copy) drives the transcript chrome.
    let style_assistant = Style::default().fg(palette::TEXT);
    // S3 (measured): a user turn is a full-width background block, not a
    // coral `>` prefix on plain text — both the marker and body carry the
    // block's background color.
    let style_user_marker = Style::default()
        .fg(palette::USER_MARKER_FG)
        .bg(palette::USER_BG);
    let style_user_body = Style::default()
        .fg(palette::USER_TEXT)
        .bg(palette::USER_BG);
    let style_tool_marker = Style::default().fg(palette::SUCCESS);
    let style_tool_marker_err = Style::default().fg(palette::ERROR);
    let style_name = Style::default()
        .fg(palette::TEXT)
        .add_modifier(Modifier::BOLD);
    // Tool arguments render in the body's default color, not dimmed — the
    // native capture shows `⏺ Write(/tmp/out.txt)` with `(...)` in the same
    // color as ordinary text, not grey.
    let style_tool_arg = Style::default().fg(palette::TEXT);
    let style_result = Style::default().fg(palette::INACTIVE);
    let style_result_err = Style::default().fg(palette::ERROR);
    let style_thinking = Style::default()
        .fg(palette::INACTIVE)
        .add_modifier(Modifier::ITALIC);
    let tool_styles = ToolStyles {
        marker: style_tool_marker,
        marker_err: style_tool_marker_err,
        name: style_name,
        arg: style_tool_arg,
        result: style_result,
        result_err: style_result_err,
    };

    // A Claude-flavored theme so the Markdown body matches the chrome.
    let md_theme = claude_markdown_theme(ctx.theme);

    // Content width after reserving the marker gutter.
    let body_width = width.saturating_sub(MARKER_COLS);

    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut meta: Vec<LineMeta> = Vec::new();

    for (ei, entry) in entries.iter().enumerate() {
        let is_user = entry.role == Role::User;

        // Per-entry aggregation state for collapsed `Counted` tool results
        // (§2.1): pre-counted occurrences per bucket, and which buckets have
        // already emitted their one summary line in this entry. Built even
        // when `ctx.expanded` is true but only consulted when it's false —
        // the cost is one small pass over blocks already being iterated.
        let bucket_counts = count_buckets(entry);
        // One shared summary line covers every `Counted` result in the entry
        // (measured: several buckets render as one comma-joined clause list), so
        // this is a single latch rather than a per-bucket set.
        let mut summary_emitted = false;

        // Tracks whether this entry rendered anything, so the separator
        // below can be skipped for an entry whose blocks all render nothing
        // (e.g. a `TodoWrite`-only or `Counted`-only `tool_use` entry) —
        // otherwise it would leave a stray blank line with no content on
        // either side of it.
        let lines_before_entry = all_lines.len();

        // ── Content blocks (no role header; marker glyphs carry the role) ────
        for (bi, block) in entry.blocks.iter().enumerate() {
            let lines_before_block = all_lines.len();
            // A labeled block, not bare `continue`, so each arm can still bail
            // out early while the per-block bookkeeping below always runs.
            'block: {
            match block {
                DisplayBlock::Text(text) => {
                    if is_user {
                        // Measured: each text block of a user message is drawn
                        // as a turn of its own, with a blank line between them
                        // — a message carrying a prompt plus an appended
                        // `<system-reminder>` block renders as two separated
                        // `❯` turns, not one packed pair. The entry-level
                        // separator further down only fires *between* entries,
                        // so the gap inside one has to come from here.
                        if all_lines.len() > lines_before_entry {
                            all_lines.push(Line::from(""));
                        }
                        // User text bypasses Markdown entirely (S3, measured)
                        // — it's raw input, not prose to parse — and wraps
                        // its own full-width background block instead of
                        // sharing the assistant/tool gutter's plain marker.
                        all_lines.extend(render_user_text(
                            text,
                            width,
                            USER_MARKER,
                            style_user_marker,
                            style_user_body,
                        ));
                        break 'block;
                    }
                    let key = format!("{ei}:{bi}");
                    let md_lines = ctx.cache.render_flavored(
                        &key,
                        text,
                        body_width,
                        &md_theme,
                        ctx.syntax_set,
                        ctx.syntect_theme,
                        crate::ui::markdown::MarkdownFlavor::Transcript,
                    );
                    all_lines.extend(with_marker(md_lines, ASSISTANT_MARKER, style_assistant));
                }
                DisplayBlock::ToolUse { name, input, errored } => {
                    if let Some(line) = render_tool_use(
                        name,
                        input,
                        *errored,
                        ctx.expanded,
                        width,
                        &tool_styles,
                    ) {
                        all_lines.push(line);
                    }
                }
                DisplayBlock::ToolResult {
                    kind,
                    lines,
                    is_error,
                } => {
                    if ctx.expanded {
                        all_lines.extend(render_tool_result_expanded(
                            lines,
                            *is_error,
                            width,
                            &tool_styles,
                        ));
                    } else {
                        all_lines.extend(render_tool_result_collapsed(
                            *kind,
                            lines,
                            *is_error,
                            &bucket_counts,
                            &mut summary_emitted,
                            width,
                            &tool_styles,
                        ));
                    }
                }
                DisplayBlock::Thinking { text, duration_secs } => {
                    if !ctx.expanded {
                        // Collapsed mode: one line, no glyph, indented to the
                        // marker column — "  Thought for {N}s (ctrl+o to
                        // expand)", the whole line INACTIVE except the
                        // duration itself, which is bold.
                        all_lines.push(fit_styled_line(
                            MARKER_COLS,
                            &[
                                ("Thought for ".to_string(), style_result),
                                (
                                    format!("{duration_secs}s"),
                                    style_result.add_modifier(Modifier::BOLD),
                                ),
                                (" (ctrl+o to expand)".to_string(), style_result),
                            ],
                            width,
                        ));
                        break 'block;
                    }

                    // Expanded mode: ✻ Thinking… header, then the (dimmed,
                    // italic) reasoning body — unchanged from before S2b.
                    let marker_prefix = pad_glyph_to(THINKING_GLYPH, MARKER_COLS);
                    all_lines.push(Line::from(vec![
                        Span::styled(marker_prefix, style_thinking),
                        Span::styled("Thinking\u{2026}", style_thinking),
                    ]));
                    if !text.trim().is_empty() {
                        let key = format!("{ei}:{bi}:think");
                        let md_lines = ctx.cache.render_flavored(
                            &key,
                            text,
                            body_width,
                            &md_theme,
                            ctx.syntax_set,
                            ctx.syntect_theme,
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
                DisplayBlock::TeammateMessage { id, body } => {
                    // S4 (Conductor's own construct, not a Claude Code CLI
                    // form): collapsed to one line, no background block —
                    // the `›` glyph carries the whole thing in `INACTIVE`.
                    let marker_prefix = pad_glyph_to(TEAMMATE_MESSAGE_GLYPH, MARKER_COLS);
                    if !ctx.expanded {
                        all_lines.push(Line::from(vec![
                            Span::styled(marker_prefix, style_result),
                            Span::styled(
                                format!("Message from @{id} (ctrl+o to expand)"),
                                style_result,
                            ),
                        ]));
                        break 'block;
                    }

                    // Expanded mode: the header line drops the toggle hint
                    // (already expanded), then the full body follows indented
                    // 2 cols — Markdown-rendered like any other prose block,
                    // since a teammate message is ordinary chat content, not
                    // a secondary annotation the way a thinking block is.
                    all_lines.push(Line::from(vec![
                        Span::styled(marker_prefix, style_result),
                        Span::styled(format!("Message from @{id}"), style_result),
                    ]));
                    if !body.trim().is_empty() {
                        let key = format!("{ei}:{bi}:teammate");
                        let md_lines = ctx.cache.render_flavored(
                            &key,
                            body,
                            body_width,
                            &md_theme,
                            ctx.syntax_set,
                            ctx.syntect_theme,
                            crate::ui::markdown::MarkdownFlavor::Transcript,
                        );
                        all_lines.extend(with_marker(md_lines, " ", style_result));
                    }
                }
                DisplayBlock::Annotation { lines } => {
                    all_lines.extend(render_annotation(lines, width, &tool_styles));
                }
                DisplayBlock::Notice(text) => {
                    // `⏺ {text}`, in the assistant body color: measured, a
                    // task-notification is drawn with the same bullet as an
                    // assistant turn rather than a tool's green one. (The
                    // exact hue is not recoverable from a byte capture; the
                    // assistant color is the reading that matches its
                    // position in the transcript.)
                    all_lines.push(fit_glyph_line(
                        ASSISTANT_MARKER,
                        &[(text.clone(), style_assistant)],
                        width,
                    ));
                }
                DisplayBlock::CompactBoundary => {
                    all_lines.push(fit_glyph_line(
                        THINKING_GLYPH,
                        &[(COMPACT_BOUNDARY_TEXT.to_string(), style_result)],
                        width,
                    ));
                }
            }
            }
            meta.extend(
                (0..all_lines.len() - lines_before_block).map(|offset| LineMeta {
                    entry: ei,
                    block: bi,
                    offset,
                    skip_col: None,
                }),
            );
        }

        // ── Blank separator between entries ──────────────────────────────────
        // An annotation-only entry is a *continuation* of the entry above it,
        // not a turn of its own — the CLI records a slash command, its stdout
        // and each carried-over file as separate records but draws them as one
        // uninterrupted group:
        //
        //     ❯ /compact
        //       ⎿  Compacted (ctrl+o to see full summary)
        //       ⎿  Read alpha.rs (42 lines)
        //
        // so the separator that would normally land in front of one is
        // suppressed.
        let next_is_continuation = entries.get(ei + 1).is_some_and(is_annotation_only);
        if !next_is_continuation && all_lines.len() > lines_before_entry {
            all_lines.push(Line::from(""));
            meta.push(LineMeta {
                entry: ei,
                block: SEPARATOR_BLOCK,
                offset: 0,
                skip_col: None,
            });
        }
    }

    // One final pass resolves each line's width-risk hole; doing it here keeps
    // every producer above free of gutter-geometry concerns.
    for (line, m) in all_lines.iter().zip(meta.iter_mut()) {
        m.skip_col = width_risk_hole(line);
    }

    debug_assert_eq!(all_lines.len(), meta.len());
    BuiltLines { lines: all_lines, meta }
}

/// Whether `entry` consists solely of [`DisplayBlock::Annotation`] blocks, and
/// so glues to the entry above it instead of starting a new turn.
fn is_annotation_only(entry: &LogEntry) -> bool {
    !entry.blocks.is_empty()
        && entry
            .blocks
            .iter()
            .all(|b| matches!(b, DisplayBlock::Annotation { .. }))
}

/// The column immediately after the first width-ambiguous gutter glyph on
/// `line`, if it has one.
///
/// `⏺`/`⎿`/`✻` measure one column in `unicode-width` but many terminals draw
/// them two columns wide, which used to shift the whole row (the scrollback
/// "bleed"). Claude Code itself sidesteps this by emitting an absolute column
/// (CHA) right after the glyph; leaving this one cell unwritten makes
/// ratatui's diff discontinuous there, which makes the crossterm backend emit
/// an absolute `MoveTo` — the same trick. Verified against the real backend
/// in `super::render`'s tests.
///
/// Two things this must get right, both of which a naive scan gets wrong:
///
/// * **Only the gutter counts.** `⏺`/`⎿`/`✻` are also ordinary characters that
///   appear in body text — this app's own transcripts are full of pasted Claude
///   Code output. A hole is an *unwritten cell*, so punching one into body text
///   both drops a character and leaves whatever the previous frame had there.
///   A marker only ever sits at column 0 (`helpers::with_marker`,
///   `helpers::fit_glyph_line`) or column 2 (`tool_lines`' `"  ⎿  "` and
///   `" ⎿  "` prefixes), so the scan stops after [`MAX_GUTTER_GLYPH_COL`].
/// * **Columns advance by grapheme cluster, not by `char`.** Summing per `char`
///   over-counts a ZWJ sequence (a family emoji is 2 columns but 7 `char`s) and
///   under-counts an emoji-presentation sequence, which would put the hole on
///   the wrong cell. Same reasoning as `helpers::truncate_to_width` and
///   `user_text::wrap_plain_text`.
fn width_risk_hole(line: &Line<'_>) -> Option<u16> {
    let mut col: usize = 0;
    for span in &line.spans {
        for cluster in span.content.graphemes(true) {
            if col > MAX_GUTTER_GLYPH_COL {
                return None; // past the gutter — everything here is content
            }
            let w = UnicodeWidthStr::width(cluster);
            // `w == 1` *is* the ambiguity: the defence exists for glyphs
            // `unicode-width` calls one column while the terminal may draw two.
            // A marker carrying a variation selector already measures two, so
            // measurement and terminal agree and no hole is wanted — punching
            // one there would blank the body's first cell instead.
            if w == 1
                && cluster
                    .chars()
                    .next()
                    .is_some_and(super::glyphs::is_width_ambiguous)
            {
                return u16::try_from(col + w).ok();
            }
            col += w;
        }
    }
    None
}
