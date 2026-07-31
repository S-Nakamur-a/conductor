//! Public file-reading API: parse a session log into display entries.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::convert::content_to_display_blocks;
use super::model::{DisplayBlock, LogEntry, Role};
use super::schema::{Block, Content, LogRecord};
use super::tool_class::ResultKind;

/// Parse a Claude Code `.jsonl` session file and return display entries.
///
/// Malformed lines and unknown record types are silently skipped.
/// Sidechain records (`isSidechain == true`) are excluded.
/// This function never panics regardless of file contents.
pub fn load_session(path: &Path) -> Vec<LogEntry> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("reflow: cannot read session file {}: {e}", path.display());
            return Vec::new();
        }
    };

    let records: Vec<LogRecord> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            match serde_json::from_str(line) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::warn!("reflow: skipping malformed jsonl line: {e}");
                    None
                }
            }
        })
        .collect();

    // Pre-scan pass: a `tool_use`'s marker must render error-colored when its
    // paired `tool_result` reported an error, but that result record always
    // comes *after* the call in the log (they land in consecutive
    // assistant/user records) — this first pass collects every errored
    // `tool_use_id` so the entry-building pass below can look each one up
    // before it gets there, instead of needing a lookahead.
    let errored_ids = scan_errored_tool_use_ids(&records);

    let mut entries = Vec::new();
    // tool_use id → its Counted bucket (Inline/Hidden calls are not
    // inserted — their raw tool name is not retained past classification).
    let mut tool_kinds: HashMap<String, ResultKind> = HashMap::new();
    // The previous *displayed* record's timestamp — used to compute a
    // collapsed `Thinking` block's "Thought for Ns" duration. Judgment call:
    // "previous" means the previous entry that actually made it into
    // `entries`, so a skipped record (isMeta/isSidechain/empty-blocks/a
    // dequeue with no text) never becomes the diff's baseline — an assistant
    // turn immediately after a skipped one still measures its thinking time
    // against the last turn the user actually saw, not a hidden one.
    let mut prev_displayed_ts: Option<String> = None;
    for record in records {
        // Context the CLI injected on the user's behalf. Two kinds draw a `⎿`
        // one-liner; every other kind (hook output, skill listings, …) is
        // invisible in Claude Code and stays invisible here.
        if record.kind == "attachment" {
            if let Some(text) = record.attachment.as_ref().and_then(attachment_line) {
                entries.push(LogEntry {
                    role: Role::User,
                    model: None,
                    blocks: vec![DisplayBlock::Annotation { lines: vec![text] }],
                });
                // Deliberately does NOT advance `prev_displayed_ts`: an
                // attachment carries the timestamp of the compact that emitted
                // it, which would otherwise be charged to the next assistant
                // turn as thinking time.
            }
            continue;
        }

        // `✻ Conversation compacted` — the only `system` record that draws.
        if record.kind == "system" {
            if record.subtype.as_deref() == Some("compact_boundary") {
                entries.push(LogEntry {
                    role: Role::Assistant,
                    model: None,
                    blocks: vec![DisplayBlock::CompactBoundary],
                });
            }
            continue;
        }

        // Only `user` and `assistant` turns carry conversation. Every other
        // record type in the schema is a session-metadata journal that Claude
        // Code never draws — `queue-operation` (enqueue/remove bookkeeping for
        // the input queue), `mode`, `permission-mode`, `last-prompt`,
        // `ai-title`, `custom-title`, `agent-name`, `pr-link`,
        // `file-history-snapshot`, `file-history-delta`. Measured for
        // `queue-operation` specifically, because it is the one that *looks*
        // displayable: it carries the queued prompt as a bare top-level
        // `content` string. Claude Code still draws nothing for it — a prompt
        // typed while it is working is re-emitted as an ordinary `user` record
        // (`promptSource: "queued"`) once accepted, so honouring the journal
        // too would print that turn twice.
        if record.kind != "user" && record.kind != "assistant" {
            continue;
        }
        if record.is_sidechain {
            continue;
        }
        // Hidden context injections (skill dumps, caveat banners, standalone
        // reminders) that Claude Code's own UI never displays.
        if record.is_meta {
            continue;
        }
        // The `/compact` summary is threaded into the next context window as a
        // pseudo-user turn. Claude Code draws none of it — only the
        // `⎿ Compacted (ctrl+o to see full summary)` line stands in for it —
        // so replaying the body here would open the transcript onto a wall of
        // text the user never saw.
        if record.is_compact_summary {
            continue;
        }

        let this_ts = record.timestamp.clone();

        let Some(msg) = record.message else {
            continue;
        };

        let role = match msg.role.as_deref() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };

        let duration_secs = thinking_duration_secs(prev_displayed_ts.as_deref(), this_ts.as_deref());

        let blocks = content_to_display_blocks(
            msg.content,
            role == Role::User,
            &mut tool_kinds,
            &errored_ids,
            duration_secs,
        );
        if blocks.is_empty() {
            continue;
        }

        entries.push(LogEntry {
            role,
            model: msg.model,
            blocks,
        });
        prev_displayed_ts = this_ts;
    }
    entries
}

/// The single `⎿` line an attachment draws, or `None` for the ~27 kinds that
/// draw nothing.
///
/// Measured against a resumed transcript:
/// ```text
///   ⎿  Read alpha.rs (42 lines)
///   ⎿  Referenced file beta.yml
/// ```
/// `displayPath` is used verbatim — Claude Code has already relativised it
/// against the session's `cwd`, including the long `../../..` prefixes a file
/// outside the worktree gets. A `file` attachment with no line count drops the
/// parenthesised clause rather than printing a zero.
fn attachment_line(attachment: &super::schema::Attachment) -> Option<String> {
    let path = attachment
        .display_path
        .as_deref()
        .or(attachment.filename.as_deref())
        .map(str::trim)
        .filter(|p| !p.is_empty())?;
    match attachment.kind.as_str() {
        "file" => {
            let n = attachment
                .content
                .as_ref()
                .and_then(|c| c.file.as_ref())
                .and_then(|f| f.num_lines);
            Some(match n {
                Some(1) => format!("Read {path} (1 line)"),
                Some(n) => format!("Read {path} ({n} lines)"),
                None => format!("Read {path}"),
            })
        }
        "compact_file_reference" => Some(format!("Referenced file {path}")),
        _ => None,
    }
}

/// Whole-second diff between `prev` and `this` RFC3339 timestamps, for a
/// collapsed `Thinking` block's "Thought for Ns" line. Falls back to `1`
/// (never `0`, per the spec) when either timestamp is missing or fails to
/// parse, or when the computed difference is zero or negative (e.g. clock
/// skew, or two records landing in the same second).
fn thinking_duration_secs(prev: Option<&str>, this: Option<&str>) -> u64 {
    let (Some(prev), Some(this)) = (prev, this) else {
        return 1;
    };
    let (Ok(prev), Ok(this)) = (
        chrono::DateTime::parse_from_rfc3339(prev),
        chrono::DateTime::parse_from_rfc3339(this),
    ) else {
        return 1;
    };
    let diff = this.signed_duration_since(prev).num_seconds();
    if diff <= 0 { 1 } else { diff as u64 }
}

/// Collect every `tool_use_id` whose paired `tool_result` block reported an
/// error, across the whole (unfiltered) record list. Used to resolve a
/// `tool_use`'s marker color before the entry-building pass reaches its
/// (later) `tool_result` record — see the pre-scan comment in
/// [`load_session`].
fn scan_errored_tool_use_ids(records: &[LogRecord]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for record in records {
        let Some(msg) = &record.message else { continue };
        let Content::Blocks(blocks) = &msg.content else {
            continue;
        };
        for block in blocks {
            if let Block::ToolResult {
                tool_use_id,
                is_error: true,
                ..
            } = block
            {
                ids.insert(tool_use_id.clone());
            }
        }
    }
    ids
}
