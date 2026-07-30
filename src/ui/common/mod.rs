//! Shared UI components used across multiple panels.
//!
//! Provides reusable widgets such as PTY output rendering, session tab bars,
//! and the status bar. Split by responsibility: [`pty`] (vt100 → ratatui
//! rendering and its cache), [`color`] (badge/contrast color math), and one
//! file per top-level bar/widget ([`title_bar`], [`notification_bar`],
//! [`status_bar`], [`worktree_label`]).

mod color;
pub mod list_row;
mod notification_bar;
mod pty;
mod status_bar;
mod title_bar;
mod worktree_label;

#[cfg(test)]
mod tests;

pub use pty::{PtyRenderCache, build_pty_lines, render_pty_cached};
pub use status_bar::render_status_bar;
pub(crate) use status_bar::representative_chord;
pub use title_bar::render_title_bar;
// Currently unwired — see `notification_bar`'s doc comment. The re-export is
// kept for parity with the pre-split module surface.
#[allow(unused_imports)]
pub use notification_bar::render_notification_bar;
pub use worktree_label::render_worktree_label;

/// Braille spinner frames for in-progress (async) operations.
const BRAILLE_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The current spinner frame for the given UI tick. Advances roughly every
/// four frames so the animation reads as a steady spin. Shared by every panel
/// that shows an async-operation spinner so they stay in sync.
pub fn spinner_frame(ui_tick: u64) -> &'static str {
    BRAILLE_SPINNER[(ui_tick as usize / 4) % BRAILLE_SPINNER.len()]
}
