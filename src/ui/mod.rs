//! UI module — organises all TUI rendering.
//!
//! Each sub-module corresponds to one panel in the unified layout.

use crate::app::{App, Focus};
use crate::theme::Theme;

pub mod common;
pub mod decoration;
pub mod worktree_panel;
pub mod explorer_panel;
pub mod viewer_panel;
pub mod terminal_claude;
pub mod terminal_shell;

// Overlay renderers (used from main.rs render_ui overlays).
pub mod dashboard;
pub mod grep_search;
pub mod review;

/// Shared read-only context extracted from `App` for UI rendering.
///
/// Provides common fields that almost every render function needs,
/// without requiring a reference to the full `App` struct.
#[allow(dead_code)]
pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub focus: Focus,
    pub expanded_panel: Option<Focus>,
    pub ui_tick: u64,
}

#[allow(dead_code)]
impl<'a> RenderContext<'a> {
    pub fn from_app(app: &'a App) -> Self {
        Self {
            theme: &app.theme,
            focus: app.focus,
            expanded_panel: app.expanded_panel,
            ui_tick: app.ui_tick,
        }
    }

    /// Whether the given panel is currently focused.
    pub fn is_focused(&self, panel: Focus) -> bool {
        self.focus == panel
    }

    /// Whether the given panel is currently expanded to fill the screen.
    pub fn is_expanded(&self, panel: Focus) -> bool {
        self.expanded_panel == Some(panel)
    }
}
