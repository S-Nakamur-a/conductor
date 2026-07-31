//! `tool_use`/`tool_result` line rendering — the `⏺`/`⎿` layouts driven by
//! `crate::claude_log`'s tool classification (§2.1 of
//! `docs/plans/2026-07-31-native-render-parity.md`).
//!
//! Split out of [`build`](super::build) because the classification-driven
//! layout rules are a distinct concern from walking entries and rendering
//! Markdown text/thinking blocks.

use std::collections::HashMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::claude_log::{
    BUCKET_ORDER, CountedBucket, DisplayBlock, LogEntry, ResultKind, ToolCategory, classify,
    unknown_tool_arg,
};

use super::glyphs::{ASSISTANT_MARKER, MARKER_COLS, TOOL_RESULT_GLYPH};
use super::helpers::{fit_styled_line, pad_glyph_to, truncate_to_width};

/// The fixed styles this module's render functions draw with, grouped into
/// one struct so each function takes one param instead of one per style
/// (clippy's `too_many_arguments` bar is 7; `render_tool_result_collapsed`
/// alone needs 5 non-style params, leaving no room for 2+ separate ones).
pub(crate) struct ToolStyles {
    pub marker: Style,
    pub marker_err: Style,
    pub name: Style,
    pub arg: Style,
    pub result: Style,
    pub result_err: Style,
}

/// Pre-count, per entry, how many `tool_result` blocks resolve to each
/// [`CountedBucket`] — used by [`render_tool_result_collapsed`] to render one
/// aggregated "{verb} N {noun}" line at a bucket's first occurrence instead
/// of one line per result.
///
/// `Counted` ignores `is_error` entirely (measured: a failed `Read` still
/// folds into the plain "Read 1 file" summary, with no error indication), so
/// every result with a bucket counts here regardless of its error flag.
///
/// Scope is the whole entry, not just a contiguous run of matching results:
/// Claude batches every result from one assistant turn into a single
/// following user entry, so an entry-wide count matches the shape actually
/// observed, and needs no separate "did the run break" tracking.
pub(crate) fn count_buckets(entry: &LogEntry) -> HashMap<CountedBucket, usize> {
    let mut native: HashMap<CountedBucket, usize> = HashMap::new();
    let mut shell: HashMap<CountedBucket, usize> = HashMap::new();
    for block in &entry.blocks {
        if let DisplayBlock::ToolResult {
            kind: ResultKind::Counted { bucket, from_bash },
            ..
        } = block
        {
            let target = if *from_bash { &mut shell } else { &mut native };
            *target.entry(*bucket).or_insert(0) += 1;
        }
    }
    // A shell invocation only counts as a *fallback*. Measured, at one call
    // site each: `cat`×1 → "Read 1 file"; `cat`×2 → "Read 2 files"; but
    // `cat`×3 + `Read`×1 → "Read 1 file", i.e. once the bucket's own tool
    // contributes at all, the shell approximations are dropped entirely.
    // (`List` has no native tool, so it always falls through to the shell
    // count; `Search` never has a shell source.)
    let mut counts = HashMap::new();
    for bucket in BUCKET_ORDER {
        let n = match native.get(&bucket).copied().unwrap_or(0) {
            0 => shell.get(&bucket).copied().unwrap_or(0),
            n => n,
        };
        if n > 0 {
            counts.insert(bucket, n);
        }
    }
    counts
}

/// Render one `tool_use` block, or `None` when it draws nothing (a
/// `Counted`/`Hidden` category in collapsed mode — those draw at the
/// `tool_result`'s position instead, or not at all).
///
/// In expanded mode every call draws, using the tool's own raw `name` (not a
/// collapsed-mode alias like `Edit` → `Update`) and a best-effort argument
/// found via [`unknown_tool_arg`]'s generic key search — expanded mode has no
/// per-tool "the" argument key the way the collapsed `Inline` categories do.
///
/// `errored` selects the marker color (measured for `Inline` in collapsed
/// mode: a failed `Bash(false)` draws its `⏺` in `palette::ERROR`, not
/// green — see `tool_class::ToolCategory::Inline`). Applied uniformly across
/// categories and to expanded mode too, since there's no measured
/// counter-example and the signal ("did this call fail") is the same either
/// way — a self-decided generalisation, not itself measured.
pub(crate) fn render_tool_use(
    name: &str,
    input: &serde_json::Value,
    errored: bool,
    expanded: bool,
    width: usize,
    styles: &ToolStyles,
) -> Option<Line<'static>> {
    let (display_name, arg) = if expanded {
        (name.to_string(), unknown_tool_arg(input))
    } else {
        match classify(name, input) {
            ToolCategory::Counted(_) | ToolCategory::Hidden => return None,
            ToolCategory::Inline { display_name, arg } => (display_name, arg),
        }
    };
    let marker_style = if errored { styles.marker_err } else { styles.marker };
    Some(tool_use_line(&display_name, arg.as_deref(), width, marker_style, styles))
}

/// `⏺ {display_name}({arg})` — bullet (color given by `marker_style`), bold
/// name, then the argument in its own style (parens omitted entirely when
/// there is no argument).
fn tool_use_line(
    display_name: &str,
    arg: Option<&str>,
    width: usize,
    marker_style: Style,
    styles: &ToolStyles,
) -> Line<'static> {
    let marker_prefix = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
    let remaining = width.saturating_sub(MARKER_COLS);
    let name_cols = UnicodeWidthStr::width(display_name);

    // Budget the parenthesised argument only if the name leaves room for it,
    // and drop the parens entirely when nothing of the argument survives.
    // Pushing the name unbudgeted used to overflow on a long MCP tool name at
    // a narrow panel — `⏺ mcp__ccgrep__search()` is 23 columns at width 20 —
    // and a saturated budget rendered a bare `Name()`.
    let arg_display = arg
        .filter(|s| !s.is_empty())
        .and_then(|a| {
            let budget = remaining.checked_sub(name_cols + 2).filter(|b| *b > 0)?;
            Some(truncate_to_width(a, budget))
        })
        .filter(|s| !s.is_empty());

    match arg_display {
        None => Line::from(vec![
            Span::styled(marker_prefix, marker_style),
            Span::styled(truncate_to_width(display_name, remaining), styles.name),
        ]),
        Some(arg) => Line::from(vec![
            Span::styled(marker_prefix, marker_style),
            Span::styled(display_name.to_string(), styles.name),
            Span::styled(format!("({arg})"), styles.arg),
        ]),
    }
}

/// Render one `tool_result` block in collapsed mode, or an empty `Vec` when
/// it draws nothing (a non-error `Inline`/`Hidden` result, or a repeat
/// occurrence of a `Counted` bucket already aggregated earlier in this
/// entry).
///
/// [`ResultKind::Counted`] **ignores `is_error` completely** — measured, not
/// guessed: a failed `Read` still folds into the plain "Read 1 file (ctrl+o
/// to expand)" summary with no error styling at all. Every `Counted` result
/// in the entry shares **one** line (several buckets render as one
/// comma-joined clause list), so `summary_emitted` is a single per-entry
/// latch: the first such result draws [`bucket_summary_line`], the rest draw
/// nothing.
///
/// [`ResultKind::Inline`] draws only on error, using the measured multi-line
/// `⎿ Error: …` layout. [`ResultKind::Hidden`] draws nothing at all, error or
/// not — also measured (a `TodoWrite` with `is_error` produced no output).
pub(crate) fn render_tool_result_collapsed(
    kind: ResultKind,
    lines: &[String],
    is_error: bool,
    counts: &HashMap<CountedBucket, usize>,
    summary_emitted: &mut bool,
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    match kind {
        // Measured: a `TodoWrite` whose result carried `is_error` produced not
        // one line of output. Hidden stays hidden even when it fails.
        ResultKind::Hidden => Vec::new(),
        ResultKind::Inline => {
            if is_error {
                inline_error_lines(lines, width, styles)
            } else {
                Vec::new()
            }
        }
        ResultKind::Counted { .. } => {
            // Every `Counted` result in the entry folds into one shared line,
            // drawn at the first of them.
            if std::mem::replace(summary_emitted, true) {
                Vec::new()
            } else {
                vec![bucket_summary_line(counts, width, styles)]
            }
        }
    }
}

/// Lowercase only the first character of `s`, leaving the rest untouched
/// (`"Searched for"` → `"searched for"`).
fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

const EXPAND_HINT: &str = " (ctrl+o to expand)";

/// The single aggregated line for every `Counted` bucket in one entry, e.g.
/// `Searched for 1 pattern, read 1 file, listed 2 directories (ctrl+o to expand)`.
///
/// Clause order is [`BUCKET_ORDER`]; the first clause's verb keeps its capital
/// and later ones are lowercased; each count is bold and the rest is
/// `styles.result`. All measured.
fn bucket_summary_line(
    counts: &HashMap<CountedBucket, usize>,
    width: usize,
    styles: &ToolStyles,
) -> Line<'static> {
    let mut parts: Vec<(String, Style)> = Vec::new();
    for bucket in BUCKET_ORDER {
        let Some(&n) = counts.get(&bucket) else {
            continue;
        };
        let (verb, singular, plural) = bucket.labels();
        let noun = if n == 1 { singular } else { plural };
        let lead = if parts.is_empty() {
            format!("{verb} ")
        } else {
            format!(", {} ", lower_first(verb))
        };
        parts.push((lead, styles.result));
        parts.push((
            n.to_string(),
            styles.result.add_modifier(Modifier::BOLD),
        ));
        parts.push((format!(" {noun}"), styles.result));
    }
    parts.push((EXPAND_HINT.to_string(), styles.result));
    fit_styled_line(MARKER_COLS, &parts, width)
}

/// `Inline`-category error block, measured column-for-column from a failed
/// `Bash(false)` capture:
/// ```text
/// ⏺ Bash(false)
///  ⎿ Error: bash: command failed with exit code 1
///     second line of the error
/// ```
/// (the `⏺` line itself is drawn by [`render_tool_use`], not here). First
/// line: col0 one gray space, `⎿` at col2, body (with a prepended `"Error: "`)
/// from col5. Continuation lines: body from col5 too, no `"Error: "` prefix.
fn inline_error_lines(lines: &[String], width: usize, styles: &ToolStyles) -> Vec<Line<'static>> {
    let first_budget = width.saturating_sub(5);
    let cont_budget = width.saturating_sub(5);
    let cont_indent = " ".repeat(5);

    let first_raw = lines.first().map(String::as_str).unwrap_or("(no content)");
    let first_body = truncate_to_width(&format!("Error: {first_raw}"), first_budget);
    let mut out = vec![Line::from(vec![
        Span::styled(" ".to_string(), styles.result), // col0: one gray space
        Span::styled(format!(" {TOOL_RESULT_GLYPH}  "), styles.result_err), // cols1-4
        Span::styled(first_body, styles.result_err),  // col5+
    ])];

    for raw in lines.iter().skip(1) {
        let body = truncate_to_width(raw, cont_budget);
        out.push(Line::from(vec![
            Span::raw(cont_indent.clone()),
            Span::styled(body, styles.result_err),
        ]));
    }
    out
}

/// Render a [`DisplayBlock::Annotation`] — the `⎿` lines the CLI attaches to
/// the block above (a slash command's stdout, a file it carried across a
/// compact).
///
/// Same gutter as a tool result (`  ⎿  ` = 5 columns, continuations aligned
/// under the body), but the text **wraps** instead of being truncated.
/// Measured: a file outside the worktree gets a `../../../…` path long enough
/// to overflow any panel, and Claude Code runs it onto a second line rather
/// than eliding it —
/// ```text
///   ⎿  Read ../../../../private/tmp/claude-501/-Users-…-plan/82e28e51-5e62-421c-aa82-d6
///      b09226bf7b/scratchpad/try-release.sh (82 lines)
/// ```
/// — note the break falls mid-path, i.e. a hard column split, which is what
/// `wrap_plain_text` does to a word wider than the budget.
pub(crate) fn render_annotation(
    lines: &[String],
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    let first_prefix = format!("  {TOOL_RESULT_GLYPH}  ");
    let prefix_cols = UnicodeWidthStr::width(first_prefix.as_str());
    let cont_indent = " ".repeat(prefix_cols);
    let budget = width.saturating_sub(prefix_cols);

    let mut out = Vec::new();
    for raw in lines {
        for wrapped in super::user_text::wrap_plain_text(raw, budget) {
            let prefix = if out.is_empty() {
                Span::styled(first_prefix.clone(), styles.result)
            } else {
                Span::raw(cont_indent.clone())
            };
            out.push(Line::from(vec![
                prefix,
                Span::styled(wrapped, styles.result),
            ]));
        }
    }
    out
}

/// Render a `tool_result` block in expanded mode: every output line, laid
/// out like Claude Code's collapsed `⎿` block but with no preview cap —
/// showing everything is the point of expanding.
pub(crate) fn render_tool_result_expanded(
    lines: &[String],
    is_error: bool,
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    let body_style = styles.result;
    // "  ⎿  " — 2-space indent + 1-col glyph + 2 spaces = 5 columns; continuation
    // lines indent by the same amount so output text stays left-aligned.
    let first_prefix = format!("  {TOOL_RESULT_GLYPH}  ");
    let prefix_cols = UnicodeWidthStr::width(first_prefix.as_str());
    let cont_indent = " ".repeat(prefix_cols);
    let connector_style = if is_error { styles.result_err } else { body_style };

    if lines.is_empty() {
        let s = truncate_to_width(&format!("{first_prefix}(no content)"), width);
        return vec![Line::from(Span::styled(s, connector_style))];
    }

    lines
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let body = truncate_to_width(raw, width.saturating_sub(prefix_cols));
            if i == 0 {
                Line::from(vec![
                    Span::styled(first_prefix.clone(), connector_style),
                    Span::styled(body, body_style),
                ])
            } else {
                Line::from(vec![
                    Span::raw(cont_indent.clone()),
                    Span::styled(body, body_style),
                ])
            }
        })
        .collect()
}
