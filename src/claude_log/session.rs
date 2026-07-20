//! Public file-reading API: parse a session log into display entries, and
//! read its first/last top-level timestamps for mid-session rotation
//! detection.

use std::path::Path;

use super::convert::content_to_display_blocks;
use super::model::{DisplayBlock, LogEntry, Role};
use super::schema::LogRecord;

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

    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: LogRecord = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("reflow: skipping malformed jsonl line: {e}");
                continue;
            }
        };

        // A prompt queued while Claude is working is stored as a
        // `queue-operation` enqueue, with the text at the top level. Surface it
        // as a user turn so the user's own input appears in the transcript; the
        // matching `remove` (dequeue) carries no text and is ignored.
        if record.kind == "queue-operation" {
            if record.operation.as_deref() == Some("enqueue")
                && let Some(text) = record.content.filter(|s| !s.is_empty())
            {
                entries.push(LogEntry {
                    role: Role::User,
                    model: None,
                    blocks: vec![DisplayBlock::Text(text)],
                });
            }
            continue;
        }

        // Only process `user` and `assistant` turns; skip sidechain records.
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

        let Some(msg) = record.message else {
            continue;
        };

        let role = match msg.role.as_deref() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };

        let blocks = content_to_display_blocks(msg.content, role == Role::User);
        if blocks.is_empty() {
            continue;
        }

        entries.push(LogEntry {
            role,
            model: msg.model,
            blocks,
        });
    }
    entries
}

/// First top-level `timestamp` in a session log (the session's start time).
///
/// Scans from the top and stops at the first record that carries one — the
/// leading `mode` / `file-history-snapshot` bookkeeping records have none, so a
/// few lines are read before the first real turn. Returns `None` for an
/// unreadable file or one with no timestamps (a pre-timestamp log format).
pub fn session_first_timestamp(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<LogRecord>(line)
            && let Some(ts) = record.timestamp
        {
            return Some(ts);
        }
    }
    None
}

/// Last top-level `timestamp` in a session log (the session's last activity).
///
/// Reads the whole file and keeps the final timestamp seen. Returns `None` for
/// an unreadable file or one with no timestamps.
pub fn session_last_timestamp(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut last = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<LogRecord>(line)
            && let Some(ts) = record.timestamp
        {
            last = Some(ts);
        }
    }
    last
}
