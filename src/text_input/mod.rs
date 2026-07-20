//! Reusable text input buffer with cursor movement and editing.
//!
//! Provides a `TextInput` struct that supports cursor navigation,
//! insertion at the cursor position, forward/backward deletion,
//! word-level movement, and clipboard paste.
//!
//! Split into: `construct` (creation/state accessors), `edit` (insertion and
//! deletion), `movement` (cursor navigation), `key_handler` (the unified key
//! event handler), and `boundary` (private char-boundary helpers shared by
//! `edit`/`movement`). The `impl TextInput` blocks in each submodule merge
//! automatically, so `crate::text_input::TextInput` needs no re-export.

mod boundary;
mod construct;
mod edit;
mod key_handler;
mod movement;
#[cfg(test)]
mod tests;

use std::fmt;
use std::ops::Deref;

/// A text input buffer with cursor position tracking.
///
/// Supports single-line and multi-line modes, cursor movement,
/// insertion/deletion at cursor position, and word-level navigation.
#[derive(Clone, Debug)]
pub struct TextInput {
    buffer: String,
    /// Cursor position as a byte offset into `buffer`.
    cursor: usize,
    /// Whether this input supports multi-line editing.
    multiline: bool,
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TextInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.buffer)
    }
}

impl Deref for TextInput {
    type Target = str;
    fn deref(&self) -> &str {
        &self.buffer
    }
}
