//! UI module — organises all TUI rendering.
//!
//! Each sub-module corresponds to one panel in the unified layout.

pub mod common;
pub mod decoration;
pub mod worktree_panel;
pub mod explorer_panel;
pub mod viewer_panel;
pub mod terminal_claude;
pub mod terminal_shell;

// Overlay renderers (used from main.rs render_ui overlays).
pub mod dashboard;
pub mod review;
