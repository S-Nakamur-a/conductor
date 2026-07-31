//! Display-ready types normalised from the raw session log schema.

use super::tool_class::ResultKind;

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
    /// A tool invocation. The renderer classifies it (see
    /// `crate::claude_log::classify`) using `name` and the raw `input` JSON
    /// to decide how — or whether — it draws a line.
    ToolUse {
        name: String,
        input: serde_json::Value,
        /// Whether this call's paired `tool_result` reported an error.
        /// Resolved by a pre-scan pass over the whole session (see
        /// `session.rs::scan_errored_tool_use_ids`), since the result record
        /// always comes *after* the call — `false` if the call had no id, or
        /// no matching result was found (e.g. a truncated log).
        errored: bool,
    },
    /// The result returned by a tool, mirroring Claude Code's collapsed `⎿`
    /// block (or, for a `Counted` tool, folding into the result-side count).
    ToolResult {
        /// What this result draws, resolved from its paired `tool_use` at
        /// parse time (the call's `input` — which `Bash`'s classification
        /// depends on — is gone by render time). `Hidden` when pairing failed:
        /// an unpaired result has no way to know its tool, and drawing a bare
        /// error block for it would be noise.
        kind: ResultKind,
        /// The tool's full output, one entry per line (unlike the old capped
        /// preview, expansion needs every line).
        lines: Vec<String>,
        /// Whether the tool reported an error.
        is_error: bool,
    },
    /// A thinking block — the assistant's reasoning text (may be empty).
    Thinking {
        text: String,
        /// Collapsed-mode "Thought for {N}s" duration: the whole-second diff
        /// between this record's timestamp and the previous *displayed*
        /// record's (see `session.rs`), minimum 1.
        duration_secs: u64,
    },
    /// A message from another agent teammate, embedded in a user turn via
    /// Conductor's own `<teammate-message teammate_id="...">` wrapper (not a
    /// Claude Code CLI construct — see `crate::claude_log::convert`). The
    /// wrapper's `summary` attribute, if present, is always ignored; `body`
    /// is the full message text, shown only when expanded.
    TeammateMessage { id: String, body: String },
    /// A `⎿`-prefixed annotation attached to the block above it: the output of
    /// a slash command (`<local-command-stdout>`) or a file the CLI carried
    /// into the conversation (a `file`/`compact_file_reference` attachment).
    ///
    /// Measured — `/model` followed by its stdout renders as
    /// ```text
    /// ❯ /model
    ///   ⎿  Set model to Opus 5
    /// ```
    /// with **no** blank line between them, which is why an entry made only of
    /// these suppresses the separator that would normally precede it (see the
    /// line builder).
    ///
    /// Multi-line stdout is unmeasured; it is laid out like an expanded tool
    /// result (glyph on the first line, 5-column indent after).
    Annotation { lines: Vec<String> },
    /// A one-line `⏺` notice the CLI generated itself rather than the model —
    /// currently only a background-task completion, whose `<task-notification>`
    /// wrapper collapses to just its `<summary>` text.
    ///
    /// Measured: the whole XML wrapper is replaced by
    /// `⏺ Background command "…" completed (exit code 0)`.
    Notice(String),
    /// The `✻ Conversation compacted` marker written where a `/compact` cut
    /// the context.
    CompactBoundary,
}
