//! Display-ready types normalised from the raw session log schema.

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
    /// A tool invocation — rendered as `⏺ {name}({summary})`.
    ToolUse { name: String, summary: String },
    /// The result returned by a tool — a capped preview of the actual output
    /// plus the total line count, mirroring Claude Code's collapsed `⎿` block.
    ToolResult {
        /// First [`TOOL_RESULT_PREVIEW_LINES`] lines of output (already capped).
        preview: Vec<String>,
        /// Total number of output lines, for the `… +N lines` summary.
        total_lines: usize,
        /// Whether the tool reported an error.
        is_error: bool,
    },
    /// A thinking block — the assistant's reasoning text (may be empty).
    Thinking { text: String },
}

/// Maximum number of tool-result output lines shown before collapsing into a
/// `… +N lines` summary, matching Claude Code's transcript preview.
pub const TOOL_RESULT_PREVIEW_LINES: usize = 4;
