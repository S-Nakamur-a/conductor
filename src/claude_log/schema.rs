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
    /// Set on the pseudo-user turn that carries a `/compact` summary into the
    /// next context window. Claude Code never draws it (measured: the summary
    /// body appears nowhere in a resumed transcript — only the
    /// `⎿ Compacted (ctrl+o to see full summary)` line does), so the reflow
    /// view must skip it too rather than replaying the whole summary as if the
    /// user had typed it.
    ///
    /// Across the local corpus this flag is always accompanied by
    /// `isVisibleInTranscriptOnly` (102 of 102 occurrences); keying on this one
    /// alone is therefore equivalent, and says what it means.
    #[serde(default)]
    pub is_compact_summary: bool,
    /// Discriminator on `type: "system"` records. Only `compact_boundary` is
    /// displayed (as `✻ Conversation compacted`).
    #[serde(default)]
    pub subtype: Option<String>,
    /// Present on `type: "attachment"` records — context the CLI injected on
    /// the user's behalf. Claude Code draws a `⎿` one-liner for a couple of
    /// these; see [`Attachment`].
    #[serde(default)]
    pub attachment: Option<Attachment>,
    #[serde(default)]
    pub message: Option<Message>,
    /// RFC3339 wall-clock time the record was written. Used only to compute
    /// a collapsed `Thinking` block's "Thought for Ns" duration (the diff
    /// against the previous *displayed* record's timestamp) — see
    /// `session.rs`. Absent on older/malformed records, in which case the
    /// duration falls back to a fixed 1s.
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// A `type: "attachment"` record's payload — context the CLI injected into the
/// conversation on the user's behalf (files carried across a compact, hook
/// output, skill listings, …).
///
/// Claude Code draws almost none of these. Measured against a resumed
/// transcript, exactly two produce a visible line:
///
/// * `file` → `⎿  Read {displayPath} ({numLines} lines)`
/// * `compact_file_reference` → `⎿  Referenced file {displayPath}`
///
/// The local corpus holds 27 other `type` values (`hook_success` alone appears
/// ~47k times); none was observed to draw anything, so the renderer works from
/// an allowlist of the two above rather than trying to cover the rest.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    #[serde(rename = "type", default)]
    pub kind: String,
    /// Path as Claude Code displays it — already relative to the session's
    /// `cwd` (with `../` segments when the file lives outside it), so it is
    /// used verbatim. Absent on some types; [`filename`](Self::filename) is
    /// the fallback.
    #[serde(default)]
    pub display_path: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub content: Option<AttachmentContent>,
}

/// The `content` wrapper on a `file` attachment.
#[derive(Deserialize)]
pub struct AttachmentContent {
    #[serde(default)]
    pub file: Option<AttachmentFile>,
}

/// The file payload of a `file` attachment. Only the line count is displayed;
/// the file's text is already in the transcript's context and is not drawn.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentFile {
    #[serde(default)]
    pub num_lines: Option<u64>,
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
        /// The API's per-call id, used only to pair this call with its
        /// matching `tool_result` block (by `tool_use_id`) while parsing —
        /// see `session.rs`. Absent on malformed/older records, in which
        /// case pairing simply fails for that call.
        #[serde(default)]
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolResult {
        /// The `id` of the `tool_use` block this result answers.
        #[serde(default)]
        tool_use_id: String,
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
