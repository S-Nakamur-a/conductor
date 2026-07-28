//! Raw serde types, one-to-one with the Claude Code `.jsonl` session schema.

use serde::Deserialize;

/// One record from a Claude Code `.jsonl` session file.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub is_sidechain: bool,
    /// Hidden context injections recorded as `user` turns — skill definition
    /// dumps (a `/skill` invocation appends the whole SKILL.md as a meta user
    /// message), caveat banners, standalone system reminders. Claude Code's
    /// live UI never displays these, so the transcript must skip them too or
    /// the reflow view opens onto walls of text the user has never seen.
    #[serde(default)]
    pub is_meta: bool,
    #[serde(default)]
    pub message: Option<Message>,
    /// `queue-operation` records carry the queued text at the top level (not in
    /// `message`) and an `operation` discriminator. A user prompt typed while
    /// Claude is still working is recorded as `operation: "enqueue"` here and
    /// never re-emitted as a `user` record, so the transcript must read it from
    /// these fields or the input is lost from the scrollback.
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
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
    /// Thinking block. Local session logs carry the full reasoning text in the
    /// `thinking` field (unlike the redacted published logs), so capture it.
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolResult {
        #[serde(default)]
        content: ToolResultContent,
        /// Set when the tool reported an error. Rendered with an error-colored
        /// connector to mirror Claude Code's transcript.
        #[serde(default)]
        is_error: bool,
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
