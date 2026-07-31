//! Normalisation of raw session-log records into display-ready blocks: tool
//! summaries, ANSI/control-code sanitization, and the user-turn wrapper forms
//! (`<command-name>`, `<local-command-stdout>`, `<task-notification>`).

use std::collections::{HashMap, HashSet};

use super::model::DisplayBlock;
use super::schema::{Block, Content, ToolResultContent};
use super::tool_class::{ResultKind, result_kind};

/// Strip characters that would desync terminal rendering from a raw tool-output
/// line: ANSI escape sequences, tabs (expanded to spaces), and other C0/C1
/// control codes. Tool output (command results, file dumps) is arbitrary text —
/// a stray tab advances the terminal cursor to the next tab stop while ratatui
/// counts it as one cell, and a color escape is zero-width to the terminal but
/// byte-width to ratatui; either shifts the rest of the line and garbles the
/// transcript panel. We render a clean, plain-text preview instead.
pub(super) fn sanitize_preview_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // ANSI escape — drop the whole sequence so it isn't rendered as text.
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: consume until a final byte in 0x40–0x7E.
                    for cc in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&cc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: consume until BEL or the ST terminator (ESC \).
                    while let Some(cc) = chars.next() {
                        if cc == '\u{07}' {
                            break;
                        }
                        if cc == '\u{1b}' {
                            if matches!(chars.peek(), Some('\\')) {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Lone ESC or other escape form: drop the following byte too.
                _ => {
                    chars.next();
                }
            },
            '\t' => out.push_str("    "),
            c if c.is_control() => {} // drop CR and other control codes
            c => out.push(c),
        }
    }
    out
}

/// Split a `ToolResultContent` into its individual output lines, sanitized for
/// safe single-row rendering (see [`sanitize_preview_line`]).
pub(super) fn result_lines(content: &ToolResultContent) -> Vec<String> {
    match content {
        ToolResultContent::None => Vec::new(),
        ToolResultContent::Text(s) => s.lines().map(sanitize_preview_line).collect(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .flat_map(|b| b.text.lines().map(sanitize_preview_line))
            .collect(),
    }
}

/// Return the text between `<{tag}>` and `</{tag}>`, if the opening tag exists.
/// An unterminated tag captures to the end of the string.
fn tag_inner<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    Some(match rest.find(&close) {
        Some(end) => &rest[..end],
        None => rest,
    })
}

/// Extract the value of `attr="..."` from a tag's attribute text (the
/// substring between the tag name and its closing `>`). A simple string
/// search, not a general attribute parser — matches `tag_inner` above, which
/// is deliberately not a full XML/HTML parser either.
fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Parse a `<teammate-message teammate_id="...">…</teammate-message>`
/// wrapper — Conductor's own multi-agent construct, not a Claude Code CLI
/// form — at the start of `lead` into its `(id, body)`. The wrapper's
/// `summary` attribute, if present, is always ignored (S4): only `body`,
/// shown when expanded, carries the message. An unterminated closing tag
/// captures through to the end of the string, matching `tag_inner`'s
/// convention. Returns `None` if `lead` doesn't open with this tag, or the
/// tag is malformed (no closing `>` on the opening tag, or no `teammate_id`
/// attribute) — the caller then falls through to treating the text as
/// ordinary prose.
fn parse_teammate_message(lead: &str) -> Option<(String, String)> {
    const OPEN_PREFIX: &str = "<teammate-message";
    const CLOSE: &str = "</teammate-message>";
    if !lead.starts_with(OPEN_PREFIX) {
        return None;
    }
    let tag_end = lead.find('>')?;
    let id = attr_value(&lead[OPEN_PREFIX.len()..tag_end], "teammate_id")?;
    let rest = &lead[tag_end + 1..];
    let body = match rest.find(CLOSE) {
        Some(end) => &rest[..end],
        None => rest,
    };
    Some((id.to_string(), body.trim().to_string()))
}

/// Normalise a user text block to what Claude Code's live UI actually shows
/// (or, for the Conductor-specific `<teammate-message>` wrapper, to the
/// display block S4 defines for it).
///
/// The session log records several wrapper forms inside plain user turns that
/// the CLI renders specially (or not at all); left raw they make the reflow
/// transcript look nothing like the screen the user just scrolled away from:
///
/// * `<teammate-message teammate_id="...">…</teammate-message>` — a message
///   from another agent teammate; folds into [`DisplayBlock::TeammateMessage`].
/// * `<command-name>/foo</command-name>…<command-args>bar</command-args>` —
///   a slash-command invocation; the CLI shows it as `> /foo bar`.
/// * `<local-command-stdout>…</local-command-stdout>` — output of a local
///   command, shown unwrapped (sanitized here: it can carry raw ANSI).
/// * `<task-notification>…</task-notification>` — a background task finishing;
///   collapses the whole message to the notification's `<summary>` line.
///
/// `<system-reminder>` is *not* in that list — see the comment at the tail of
/// this function for why it is left in place.
///
/// Returns `None` when nothing displayable remains.
fn normalise_user_text(text: String) -> Option<DisplayBlock> {
    // The wrapper forms are only recognised at the very start of the message
    // (that is where they're written); a user merely *mentioning* one of
    // these tags mid-prompt keeps their text untouched.
    let lead = text.trim_start();
    if lead.starts_with("<teammate-message")
        && let Some((id, body)) = parse_teammate_message(lead)
    {
        return Some(DisplayBlock::TeammateMessage { id, body });
    }
    // Measured: `<task-notification>` is matched *anywhere* in the message, not
    // just at its start, and collapsing it discards everything else the message
    // held. Claude Code checks neither the tag's position nor who wrote the
    // record — a screen dump pasted by hand collapses exactly like the CLI's own
    // notification, taking the prose typed around it with it. Hence this runs
    // before the leading-tag forms below, and takes only the *first* summary
    // when a message carries several.
    //
    // With no usable `<summary>` the message vanishes entirely rather than
    // falling back to its raw text — also measured, for both a missing tag and
    // an empty one.
    if lead.contains("<task-notification>") {
        let summary = tag_inner(lead, "summary")
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        return Some(DisplayBlock::Notice(sanitize_preview_line(summary)));
    }
    // The CLI always writes a terminated tag; an unterminated one at the start
    // of a message is user prose, not a command record — leave it alone rather
    // than swallowing the whole message as a "command name".
    if lead.starts_with("<command-name>")
        && lead.contains("</command-name>")
        && let Some(name) = tag_inner(lead, "command-name")
    {
        let args = tag_inner(lead, "command-args").unwrap_or("").trim();
        let display = if args.is_empty() {
            name.trim().to_string()
        } else {
            format!("{} {}", name.trim(), args)
        };
        return (!display.is_empty()).then_some(DisplayBlock::Text(display));
    }
    if lead.starts_with("<local-command-stdout>")
        && let Some(stdout) = tag_inner(lead, "local-command-stdout")
    {
        // Measured: a command's stdout is drawn as a `⎿` continuation of the
        // `❯ /command` line above it, not as a user turn of its own.
        let lines: Vec<String> = stdout
            .trim()
            .lines()
            .map(sanitize_preview_line)
            .filter(|l| !l.trim().is_empty())
            .collect();
        return (!lines.is_empty()).then_some(DisplayBlock::Annotation { lines });
    }
    // `<system-reminder>` spans are deliberately *not* removed: measured, Claude
    // Code draws them verbatim, whether they sit inline in a turn's text or
    // arrive as a block of their own. The reminders a reader never sees on
    // screen are hidden by their record's `isMeta` flag instead (10 of the 11
    // reminder-only records in the local corpus carry it), which `session.rs`
    // skips before reaching here.
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| DisplayBlock::Text(trimmed.to_string()))
}

/// Convert a [`Content`] value into display blocks, normalising the two surface
/// forms (bare string and typed block array) into the same flat representation.
///
/// `is_user` applies the user-turn wrapper normalisation (slash commands,
/// local-command stdout, task notifications — see [`normalise_user_text`]).
/// Assistant text is left untouched: it may legitimately *quote* those tags.
///
/// `tool_kinds` is the session-wide `tool_use` id → [`ResultKind`]
/// pairing map (see `session.rs`): a `Counted`-category `tool_use` block
/// writes its bucket in under its id; a `tool_result` block looks its id up
/// to recover it (`None` for `Inline`/`Hidden` calls — their raw tool name is
/// not retained here, only `log::debug!`-able at classification time if ever
/// needed — or if pairing failed to find the matching `tool_use`, e.g.
/// truncated logs). The map outlives a single call — it is threaded through
/// every record in a session so a `tool_use` in one record can be found by a
/// `tool_result` in a later one.
///
/// `errored_ids` is the session-wide pre-scan of `tool_use_id`s whose
/// `tool_result` reported an error (see `session.rs::scan_errored_tool_use_ids`),
/// needed because a `tool_use` renders before its `tool_result` is reached.
///
/// `thinking_duration_secs` is this record's precomputed "Thought for Ns"
/// value (see `session.rs::thinking_duration_secs`), applied to every
/// `Thinking` block found here — a record carries one timestamp for all its
/// content blocks, so multiple `Thinking` blocks in one record share it.
pub(super) fn content_to_display_blocks(
    content: Content,
    is_user: bool,
    tool_kinds: &mut HashMap<String, ResultKind>,
    errored_ids: &HashSet<String>,
    thinking_duration_secs: u64,
) -> Vec<DisplayBlock> {
    let text_block = |text: String| -> Option<DisplayBlock> {
        if text.is_empty() {
            return None;
        }
        if is_user {
            normalise_user_text(text)
        } else {
            Some(DisplayBlock::Text(text))
        }
    };
    match content {
        Content::Text(s) => text_block(s).into_iter().collect(),
        Content::Blocks(blocks) => blocks
            .into_iter()
            .filter_map(|b| match b {
                Block::Text { text } => text_block(text),
                Block::Thinking { thinking } => Some(DisplayBlock::Thinking {
                    text: thinking,
                    duration_secs: thinking_duration_secs,
                }),
                Block::ToolUse { id, name, input } => {
                    let errored = !id.is_empty() && errored_ids.contains(&id);
                    if !id.is_empty() {
                        tool_kinds.insert(id, result_kind(&name, &input));
                    }
                    Some(DisplayBlock::ToolUse { name, input, errored })
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // An unpaired result (truncated log, id missing) falls back
                    // to `Hidden`: without its `tool_use` there is no way to
                    // know which category it belongs to, and guessing `Inline`
                    // would emit a stray error block.
                    let kind = tool_kinds
                        .get(&tool_use_id)
                        .copied()
                        .unwrap_or(ResultKind::Hidden);
                    let lines = result_lines(&content);
                    Some(DisplayBlock::ToolResult {
                        kind,
                        lines,
                        is_error,
                    })
                }
                Block::Other => None,
            })
            .collect(),
    }
}
