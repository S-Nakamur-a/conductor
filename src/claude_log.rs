//! Claude Code session log parser.
//!
//! Reads a Claude Code `.jsonl` session file and normalises its records into
//! a flat list of [`LogEntry`] values for display in the reflow transcript view.
//!
//! Parsing is line-oriented and lenient: malformed lines are silently skipped,
//! unknown `type` values are ignored, and sidechain records are excluded.
//! The module never panics regardless of input.

use std::path::Path;

use serde::Deserialize;

// ── Raw serde types (one-to-one with the JSONL schema) ────────────────────────

/// One record from a Claude Code `.jsonl` session file.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub message: Option<Message>,
}

/// The `message` field present on `user` and `assistant` records.
#[derive(Deserialize)]
pub struct Message {
    #[serde(default)]
    pub role: Option<String>,
    /// Model name, present on `assistant` messages.
    #[serde(default)]
    pub model: Option<String>,
    pub content: Content,
}

/// `content` is either a bare string or an array of typed blocks.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

/// A single typed block within an array-form `content`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    /// Thinking block — shown as a stub; body is empty in published logs.
    Thinking,
    ToolUse {
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolResult {
        #[serde(default)]
        content: ToolResultContent,
    },
    /// Any block type not explicitly handled above.
    #[serde(other)]
    Other,
}

/// The `content` field of a `tool_result` block, which may be a bare string,
/// an array of text-only objects, or absent.
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum ToolResultContent {
    #[default]
    None,
    Text(String),
    Blocks(Vec<TextOnly>),
}

/// A `{ "text": "..." }` object as found in `tool_result` content arrays.
#[derive(Deserialize)]
pub struct TextOnly {
    #[serde(default)]
    pub text: String,
}

// ── Display-layer types ────────────────────────────────────────────────────────

/// Speaker of a conversation turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// A display-ready conversation entry normalised from one raw log record.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub role: Role,
    /// Model name, when present on an assistant message.
    /// Retained for future use (e.g. per-entry model display); not rendered in the
    /// current CLI-style glyph layout.
    #[allow(dead_code)]
    pub model: Option<String>,
    pub blocks: Vec<DisplayBlock>,
}

/// A display-ready content fragment within a [`LogEntry`].
#[derive(Debug, Clone)]
pub enum DisplayBlock {
    /// Markdown prose (user input or assistant text response).
    Text(String),
    /// A tool invocation — rendered as `▸ {name}: {summary}`.
    ToolUse { name: String, summary: String },
    /// The result returned by a tool — rendered as a line-count summary.
    ToolResult { lines: usize },
    /// A thinking block — rendered as a placeholder stub.
    Thinking,
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Extract a one-line summary from a tool call's `input` JSON.
///
/// Tries the most common argument keys in priority order (`command`,
/// `file_path`, `path`, `pattern`) and returns an empty string if none match.
fn summarise_tool_input(input: &serde_json::Value) -> String {
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

/// Count the total number of output lines in a `ToolResultContent`.
fn count_result_lines(content: &ToolResultContent) -> usize {
    match content {
        ToolResultContent::None => 0,
        ToolResultContent::Text(s) => s.lines().count(),
        ToolResultContent::Blocks(blocks) => blocks.iter().map(|b| b.text.lines().count()).sum(),
    }
}

/// Convert a [`Content`] value into display blocks, normalising the two surface
/// forms (bare string and typed block array) into the same flat representation.
fn content_to_display_blocks(content: Content) -> Vec<DisplayBlock> {
    match content {
        Content::Text(s) if !s.is_empty() => vec![DisplayBlock::Text(s)],
        Content::Text(_) => vec![],
        Content::Blocks(blocks) => blocks
            .into_iter()
            .filter_map(|b| match b {
                Block::Text { text } if !text.is_empty() => Some(DisplayBlock::Text(text)),
                Block::Text { .. } => None,
                Block::Thinking => Some(DisplayBlock::Thinking),
                Block::ToolUse { name, input } => {
                    let summary = summarise_tool_input(&input);
                    Some(DisplayBlock::ToolUse { name, summary })
                }
                Block::ToolResult { content } => {
                    let lines = count_result_lines(&content);
                    Some(DisplayBlock::ToolResult { lines })
                }
                Block::Other => None,
            })
            .collect(),
    }
}

// ── Public API ─────────────────────────────────────────────────────────────────

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

        // Only process `user` and `assistant` turns; skip sidechain records.
        if record.kind != "user" && record.kind != "assistant" {
            continue;
        }
        if record.is_sidechain {
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

        let blocks = content_to_display_blocks(msg.content);
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

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_msg_content(json: &str) -> Vec<DisplayBlock> {
        let r: LogRecord = serde_json::from_str(json).expect("valid test json");
        content_to_display_blocks(r.message.unwrap().content)
    }

    #[test]
    fn string_content_becomes_single_text_block() {
        let blocks = parse_msg_content(
            r#"{"type":"user","message":{"role":"user","content":"hello world"}}"#,
        );
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], DisplayBlock::Text(_)));
    }

    #[test]
    fn array_content_multiple_block_types() {
        let blocks = parse_msg_content(
            r#"{
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "model": "claude-opus-4",
                    "content": [
                        {"type":"text","text":"Here is the plan."},
                        {"type":"thinking","thinking":"","signature":"abc"},
                        {"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls -la"}},
                        {"type":"tool_result","tool_use_id":"tu1","content":"file1\nfile2\nfile3"}
                    ]
                }
            }"#,
        );
        assert_eq!(blocks.len(), 4);
        assert!(matches!(blocks[0], DisplayBlock::Text(_)));
        assert!(matches!(blocks[1], DisplayBlock::Thinking));
        assert!(matches!(blocks[2], DisplayBlock::ToolUse { .. }));
        assert!(matches!(blocks[3], DisplayBlock::ToolResult { lines: 3 }));
    }

    #[test]
    fn sidechain_flag_is_parsed() {
        let r: LogRecord = serde_json::from_str(
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"secret"}}"#,
        )
        .unwrap();
        assert!(r.is_sidechain);
    }

    #[test]
    fn tool_use_summary_picks_command_key() {
        let input = serde_json::json!({"command": "cargo build", "cwd": "/project"});
        assert_eq!(summarise_tool_input(&input), "cargo build");
    }

    #[test]
    fn tool_use_summary_falls_back_to_file_path() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});
        assert_eq!(summarise_tool_input(&input), "/src/main.rs");
    }

    #[test]
    fn tool_use_summary_falls_back_to_pattern() {
        let input = serde_json::json!({"pattern": "fn open_reflow"});
        assert_eq!(summarise_tool_input(&input), "fn open_reflow");
    }

    #[test]
    fn tool_result_string_counts_lines() {
        let content = ToolResultContent::Text("a\nb\nc".to_string());
        assert_eq!(count_result_lines(&content), 3);
    }

    #[test]
    fn tool_result_block_array_counts_lines() {
        let content = ToolResultContent::Blocks(vec![
            TextOnly {
                text: "a\nb\nc".to_string(),
            },
            TextOnly {
                text: "d\ne".to_string(),
            },
        ]);
        assert_eq!(count_result_lines(&content), 5);
    }

    #[test]
    fn empty_text_block_excluded() {
        let blocks = parse_msg_content(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#,
        );
        assert!(blocks.is_empty());
    }

    #[test]
    fn unknown_block_type_is_skipped() {
        let blocks = parse_msg_content(
            r#"{"type":"assistant","message":{"role":"assistant","content":[
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"..."}},
                {"type":"text","text":"done"}
            ]}}"#,
        );
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], DisplayBlock::Text(_)));
    }

    // ── load_session integration tests (write a temp .jsonl, call load_session) ──

    fn write_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("tmp file");
        for line in lines {
            writeln!(f, "{line}").expect("write");
        }
        f
    }

    #[test]
    fn load_session_skips_non_user_assistant_types() {
        let f = write_jsonl(&[
            r#"{"type":"system","message":{"role":"system","content":"sys prompt"}}"#,
            r#"{"type":"summary","message":{"role":"assistant","content":"summary"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"hello"}}"#,
        ]);
        let entries = load_session(f.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, Role::User);
    }

    #[test]
    fn load_session_skips_sidechain_records() {
        let f = write_jsonl(&[
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"hidden"}}"#,
            r#"{"type":"assistant","isSidechain":false,"message":{"role":"assistant","content":"visible"}}"#,
        ]);
        let entries = load_session(f.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, Role::Assistant);
    }

    #[test]
    fn load_session_skips_role_mismatch() {
        // A record with type=user but role=system should be silently dropped.
        let f = write_jsonl(&[
            r#"{"type":"user","message":{"role":"system","content":"not a user turn"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"real user"}}"#,
        ]);
        let entries = load_session(f.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, Role::User);
    }

    #[test]
    fn load_session_skips_empty_blocks() {
        // A message whose content produces zero display blocks is not emitted.
        let f = write_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"valid"}}"#,
        ]);
        let entries = load_session(f.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, Role::User);
    }

    #[test]
    fn load_session_mixed_records_correct_count() {
        let f = write_jsonl(&[
            r#"{"type":"user","message":{"role":"user","content":"q1"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":"a1"}}"#,
            r#"{"type":"system","message":{"role":"system","content":"noise"}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"chain"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"q2"}}"#,
            r#"not-json-at-all"#,
        ]);
        let entries = load_session(f.path());
        // system noise, sidechain, and malformed line are excluded → 3 remain.
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].role, Role::User);
        assert_eq!(entries[1].role, Role::Assistant);
        assert_eq!(entries[2].role, Role::User);
    }

    #[test]
    fn load_session_missing_file_returns_empty() {
        let entries = load_session(std::path::Path::new("/nonexistent/path.jsonl"));
        assert!(entries.is_empty());
    }
}
