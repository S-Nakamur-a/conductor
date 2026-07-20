//! Normalisation of raw session-log records into display-ready blocks: tool
//! summaries, ANSI/control-code sanitization, and the user-turn wrapper forms
//! (`<command-name>`, `<local-command-stdout>`, `<system-reminder>`).

use super::model::{DisplayBlock, TOOL_RESULT_PREVIEW_LINES};
use super::schema::{Block, Content, ToolResultContent};

/// Extract a one-line summary from a tool call's `input` JSON.
///
/// Tries the most common argument keys in priority order (`command`,
/// `file_path`, `path`, `pattern`) and returns an empty string if none match.
pub(super) fn summarise_tool_input(input: &serde_json::Value) -> String {
    const KEYS: &[&str] = &["command", "file_path", "path", "pattern"];
    let Some(obj) = input.as_object() else {
        return String::new();
    };
    for key in KEYS {
        if let Some(v) = obj.get(*key)
            && let Some(s) = v.as_str()
        {
            return s.to_string();
        }
    }
    String::new()
}

/// Strip characters that would desync terminal rendering from a raw tool-output
/// line: ANSI escape sequences, tabs (expanded to spaces), and other C0/C1
/// control codes. Tool output (command results, file dumps) is arbitrary text —
/// a stray tab advances the terminal cursor to the next tab stop while ratatui
/// counts it as one cell, and a color escape is zero-width to the terminal but
/// byte-width to ratatui; either shifts the rest of the line and garbles the
/// transcript panel. We render a clean, plain-text preview instead.
fn sanitize_preview_line(s: &str) -> String {
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

/// Remove every `<{tag}>…</{tag}>` span from `text` (an unterminated opening
/// tag removes through to the end of the string).
fn strip_tag_spans(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        rest = &rest[start + open.len()..];
        match rest.find(&close) {
            Some(end) => rest = &rest[end + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Normalise a user text block to what Claude Code's live UI actually shows.
///
/// The session log records several wrapper forms inside plain user turns that
/// the CLI renders specially (or not at all); left raw they make the reflow
/// transcript look nothing like the screen the user just scrolled away from:
///
/// * `<command-name>/foo</command-name>…<command-args>bar</command-args>` —
///   a slash-command invocation; the CLI shows it as `> /foo bar`.
/// * `<local-command-stdout>…</local-command-stdout>` — output of a local
///   command, shown unwrapped (sanitized here: it can carry raw ANSI).
/// * `<system-reminder>…</system-reminder>` — hidden context; stripped.
///
/// Returns `None` when nothing displayable remains.
fn normalise_user_text(text: String) -> Option<String> {
    // The wrapper forms are only recognised at the very start of the message
    // (that is where the CLI writes them); a user merely *mentioning* one of
    // these tags mid-prompt keeps their text untouched.
    let lead = text.trim_start();
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
        return (!display.is_empty()).then_some(display);
    }
    if lead.starts_with("<local-command-stdout>")
        && let Some(stdout) = tag_inner(lead, "local-command-stdout")
    {
        let cleaned: Vec<String> = stdout.trim().lines().map(sanitize_preview_line).collect();
        let joined = cleaned.join("\n").trim().to_string();
        return (!joined.is_empty()).then_some(joined);
    }
    let stripped = strip_tag_spans(&text, "system-reminder");
    let trimmed = stripped.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Convert a [`Content`] value into display blocks, normalising the two surface
/// forms (bare string and typed block array) into the same flat representation.
///
/// `is_user` applies the user-turn wrapper normalisation (slash commands,
/// local-command stdout, system reminders — see [`normalise_user_text`]).
/// Assistant text is left untouched: it may legitimately *quote* those tags.
pub(super) fn content_to_display_blocks(content: Content, is_user: bool) -> Vec<DisplayBlock> {
    let text_block = |text: String| -> Option<DisplayBlock> {
        if text.is_empty() {
            return None;
        }
        if is_user {
            normalise_user_text(text).map(DisplayBlock::Text)
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
                Block::Thinking { thinking } => Some(DisplayBlock::Thinking { text: thinking }),
                Block::ToolUse { name, input } => {
                    let summary = summarise_tool_input(&input);
                    Some(DisplayBlock::ToolUse { name, summary })
                }
                Block::ToolResult { content, is_error } => {
                    let lines = result_lines(&content);
                    let total_lines = lines.len();
                    let preview = lines.into_iter().take(TOOL_RESULT_PREVIEW_LINES).collect();
                    Some(DisplayBlock::ToolResult {
                        preview,
                        total_lines,
                        is_error,
                    })
                }
                Block::Other => None,
            })
            .collect(),
    }
}
