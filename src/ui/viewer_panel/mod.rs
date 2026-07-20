//! Viewer panel — file content display with diff highlights and review comments.
//!
//! Shows the content of the selected file in the middle column. Lines that
//! have been modified (according to diff_state) are highlighted inline.
//! Review comments are shown as inline badges.
//!
//! Split by rendering responsibility: [`file_view`] draws the plain/annotated
//! file content (the panel's default mode), [`diff_view`] the unified-diff
//! mode, [`summary_view`] the branch change-summary pseudo-file,
//! [`media_view`] images/video, [`comment_thread`] inline review-comment
//! threads and the new-comment compose box, [`syntax`] syntax/diff annotation
//! helpers, [`span_utils`] generic `Span` manipulation, and [`search_box`] the
//! in-panel search input.

mod code_line;
mod comment_thread;
mod diff_line;
mod diff_view;
mod file_view;
mod media_view;
mod search_box;
mod span_utils;
mod summary_view;
mod syntax;

pub use file_view::render;

/// Shared definition of the inline-thread action row.
///
/// The renderer ([`comment_thread::build_inline_thread_lines`]) and the mouse
/// hit-testing in `event/mouse.rs` must agree on where each action sits; both
/// derive their layout from these constants so a label change cannot silently
/// break click targets.
pub(crate) mod thread_actions {
    pub const REPLY: &str = "\u{21a9} reply"; // ↩ reply
    pub const RESOLVE: &str = "\u{2713} resolve"; // ✓ resolve
    pub const UNRESOLVE: &str = "\u{21ba} unresolve"; // ↺ unresolve
    pub const DELETE: &str = "\u{2717} delete"; // ✗ delete
    pub const ASK_CLAUDE: &str = "\u{2728} ask claude"; // ✨ ask claude
    /// Columns of spacing between actions.
    pub const GAP: usize = 2;

    fn w(s: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(s)
    }

    /// Width the status (resolve/unresolve) slot is padded to, so the delete
    /// action starts at a stable column regardless of the current status.
    pub fn status_slot_width() -> usize {
        w(RESOLVE).max(w(UNRESOLVE))
    }

    /// Clicks left of this column (relative to the action-row content start)
    /// hit "reply".
    pub fn reply_end() -> usize {
        w(REPLY) + GAP
    }

    /// Clicks in `reply_end()..resolve_end()` hit "resolve"/"unresolve";
    /// clicks at or beyond it hit "delete" (or "ask claude" on the far right).
    pub fn resolve_end() -> usize {
        reply_end() + status_slot_width() + GAP
    }

    /// Display width of the right-aligned "ask claude" button, for hit-testing
    /// against the panel's right edge.
    pub fn ask_claude_width() -> usize {
        w(ASK_CLAUDE)
    }
}
