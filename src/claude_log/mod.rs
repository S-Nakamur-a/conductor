//! Claude Code session log parser.
//!
//! Reads a Claude Code `.jsonl` session file and normalises its records into
//! a flat list of [`LogEntry`] values for display in the reflow transcript view.
//!
//! Parsing is line-oriented and lenient: malformed lines are silently skipped,
//! unknown `type` values are ignored, and sidechain records are excluded.
//! The module never panics regardless of input.
//!
//! Split by responsibility: [`schema`] holds the raw serde types (one-to-one
//! with the JSONL schema), [`model`] holds the display-ready types, [`convert`]
//! normalises raw records into display blocks, and [`session`] is the public
//! file-reading API ([`load_session`]).

mod convert;
mod model;
mod schema;
mod session;
#[cfg(test)]
mod tests;

pub use session::load_session;

// `DisplayBlock` and `Role` are consumed externally (via `crate::claude_log::X`)
// by `ui/reflow_view.rs`; `LogEntry` likewise by `app/reflow.rs`. The rest of
// these re-exports (`TOOL_RESULT_PREVIEW_LINES` and the raw schema types) are
// only used within this module tree today, via `super::model::X` /
// `super::schema::X`, but stay re-exported at the module root to preserve the
// pre-split `crate::claude_log::X` path for any future external caller.
#[allow(unused_imports)]
pub use model::{DisplayBlock, LogEntry, Role, TOOL_RESULT_PREVIEW_LINES};
#[allow(unused_imports)]
pub use schema::{Block, Content, LogRecord, Message, TextOnly, ToolResultContent};
