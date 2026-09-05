//! セッションログ全体を表示用エントリの列にする。

use std::path::Path;

use super::convert::{ToolPairing, content_to_display_blocks};
use super::model::{DisplayBlock, LogEntry, Role};
use super::schema::{Attachment, LogRecord};

/// .jsonl を読んで [parse_jsonl] にかける。読めなければ空。
pub fn load_session(path: &Path) -> Vec<LogEntry> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_jsonl(&text),
        Err(e) => {
            log::warn!(
                "claude_log: cannot read session file {}: {e}",
                path.display()
            );
            Vec::new()
        }
    }
}

/// 壊れた行と未知のレコード種別は黙って飛ばす。isSidechain / isMeta / isCompactSummary の
/// レコードは Claude Code 自身が描かないので除く。入力によらず panic しない。
pub fn parse_jsonl(text: &str) -> Vec<LogEntry> {
    let records: Vec<LogRecord> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            serde_json::from_str(line)
                .map_err(|e| log::warn!("claude_log: skipping malformed jsonl line: {e}"))
                .ok()
        })
        .collect();
    let mut pairing = ToolPairing::scan(&records);

    let mut entries = Vec::new();
    // 直前に表示したレコードの時刻。飛ばしたレコードは Thinking の秒数の基準にしない。
    let mut prev_displayed_ts: Option<String> = None;
    for record in records {
        match record.kind.as_str() {
            // attachment の時刻はそれを発行した compact のものなので、基準を進めない。
            "attachment" => {
                if let Some(line) = record.attachment.as_ref().and_then(attachment_line) {
                    entries.push(LogEntry {
                        role: Role::User,
                        blocks: vec![DisplayBlock::Annotation { lines: vec![line] }],
                    });
                }
            }
            "system" if record.subtype.as_deref() == Some("compact_boundary") => {
                entries.push(LogEntry {
                    role: Role::Assistant,
                    blocks: vec![DisplayBlock::CompactBoundary],
                });
            }
            "user" | "assistant"
                if !record.is_sidechain && !record.is_meta && !record.is_compact_summary =>
            {
                let Some(msg) = record.message else { continue };
                let role = match msg.role.as_deref() {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => continue,
                };
                let secs = thinking_duration_secs(
                    prev_displayed_ts.as_deref(),
                    record.timestamp.as_deref(),
                );
                let blocks = content_to_display_blocks(msg.content, &role, &mut pairing, secs);
                if blocks.is_empty() {
                    continue;
                }
                entries.push(LogEntry { role, blocks });
                prev_displayed_ts = record.timestamp;
            }
            // queue-operation / mode / last-prompt などのジャーナル。Claude Code はどれも描かない。
            _ => {}
        }
    }
    entries
}

fn attachment_line(attachment: &Attachment) -> Option<String> {
    let path = attachment
        .display_path
        .as_deref()
        .or(attachment.filename.as_deref())
        .map(str::trim)
        .filter(|p| !p.is_empty())?;
    match attachment.kind.as_str() {
        "file" => {
            let num_lines = attachment
                .content
                .as_ref()
                .and_then(|c| c.file.as_ref())
                .and_then(|f| f.num_lines);
            Some(match num_lines {
                Some(1) => format!("Read {path} (1 line)"),
                Some(n) => format!("Read {path} ({n} lines)"),
                None => format!("Read {path}"),
            })
        }
        "compact_file_reference" => Some(format!("Referenced file {path}")),
        _ => None,
    }
}

/// 時刻が無い・壊れている・差が 0 以下のときは 1。
fn thinking_duration_secs(prev: Option<&str>, this: Option<&str>) -> u64 {
    let parse = |s: Option<&str>| chrono::DateTime::parse_from_rfc3339(s?).ok();
    match (parse(prev), parse(this)) {
        (Some(prev), Some(this)) => this
            .signed_duration_since(prev)
            .num_seconds()
            .try_into()
            .ok()
            .filter(|&secs| secs > 0)
            .unwrap_or(1),
        _ => 1,
    }
}
