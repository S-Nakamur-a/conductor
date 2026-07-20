//! Theme configuration for UI colors.
//!
//! Defines a set of named colors used throughout the UI, with support for
//! loading custom themes from the configuration.
//!
//! Split by responsibility: this file holds the `Theme` struct itself and its
//! `Default` impl; `registry` resolves theme names to built-in palettes;
//! `color_ops` holds the generic color-math methods (darken/lighten/
//! complement/lerp/high_contrast); `hsl` holds the private RGB↔HSL
//! conversion used by `complement`; and one file per built-in palette
//! (`catppuccin`, `dracula`, `nord`, `solarized`, `tokyo_night`, `gruvbox`,
//! `rose_pine`, `kanagawa`, `github`) holds that theme's constructor(s) as an
//! `impl Theme` block.

use ratatui::style::Color;

mod catppuccin;
mod color_ops;
mod dracula;
mod github;
mod gruvbox;
mod hsl;
mod kanagawa;
mod nord;
mod registry;
mod rose_pine;
mod solarized;
mod tokyo_night;

#[cfg(test)]
mod tests;

/// A color theme for the application.
#[derive(Debug, Clone)]
pub struct Theme {
    // ── Meta ─────────────────────────────────────────────────────────
    /// The canonical name of this theme (matches the key used in `from_name`
    /// and returned by `all_names`). Primarily used to detect registration
    /// drift between `from_name` and `all_names` at test time.
    #[allow(dead_code)]
    pub name: &'static str,
    /// Whether this is a light (true) or dark (false) background theme.
    /// Used by OSC 11 auto-detection and by the theme-picker light/dark tag.
    pub light: bool,

    // ── Core ─────────────────────────────────────────────────────────
    /// Foreground color for normal text.
    pub fg: Color,
    /// Accent color (used for highlights, selections).
    pub accent: Color,
    /// Color for muted/dimmed text.
    pub muted: Color,
    /// Color for success indicators.
    pub success: Color,
    /// Color for error/danger indicators.
    pub error: Color,
    /// Color for warning indicators.
    pub warning: Color,
    /// Color for informational text.
    pub info: Color,

    // ── Diff ─────────────────────────────────────────────────────────
    /// Color for added/inserted lines in diffs.
    pub diff_add: Color,
    /// Background color for added lines.
    pub diff_add_bg: Color,
    /// Color for deleted/removed lines in diffs.
    pub diff_del: Color,
    /// Background color for deleted lines.
    pub diff_del_bg: Color,
    /// Brighter background for emphasized (word-level) additions.
    pub diff_add_bg_emphasis: Color,
    /// Brighter background for emphasized (word-level) deletions.
    pub diff_del_bg_emphasis: Color,
    /// Color for diff hunk section headers (function names, etc.) — brighter than muted.
    pub diff_section_header: Color,

    // ── Border ───────────────────────────────────────────────────────
    /// Border color when panel is focused.
    pub border_focused: Color,
    /// Border color when panel is unfocused.
    pub border_unfocused: Color,
    /// Secondary border color (separator between sub-areas).
    pub border_secondary: Color,

    // ── Selection ────────────────────────────────────────────────────
    /// Background for the currently selected item (active panel).
    pub selected_bg: Color,
    /// Foreground for the currently selected item (active panel).
    pub selected_fg: Color,
    /// Background for the currently selected item (inactive panel).
    pub selected_bg_inactive: Color,
    /// Foreground for the currently selected item (inactive panel).
    pub selected_fg_inactive: Color,

    // ── Line selection (viewer) ──────────────────────────────────────
    /// Background for selected lines in the viewer.
    pub line_selected_bg: Color,
    /// Foreground for selected lines in the viewer.
    pub line_selected_fg: Color,

    // ── Gutter ───────────────────────────────────────────────────────
    /// Background for gutter of selected lines.
    pub gutter_selected_bg: Color,
    /// Foreground for gutter of selected lines.
    pub gutter_selected_fg: Color,
    /// Foreground for gutter line numbers on hover (slightly brighter than muted).
    pub gutter_hover_fg: Color,
    /// Background for gutter line numbers on hover (subtle highlight to indicate clickability).
    pub gutter_hover_bg: Color,
    /// Background for gutter of pending range lines (dimmer than selected).
    pub gutter_pending_bg: Color,
    /// Background for pending range lines in the viewer (dimmer than selected).
    pub line_pending_bg: Color,

    // ── Text ─────────────────────────────────────────────────────────
    /// Color for hint / muted helper text.
    pub hint: Color,
    /// Foreground for non-current search matches.
    pub search_match_fg: Color,
    /// Background for the current search match.
    pub search_match_bg: Color,
    /// Foreground for the current search match.
    pub search_current_fg: Color,

    // ── Waiting / pulse ──────────────────────────────────────────────
    /// Primary waiting indicator color (bright orange).
    pub waiting_primary: Color,
    /// Secondary waiting indicator color (dimmer orange).
    pub waiting_secondary: Color,

    // ── Title bar ────────────────────────────────────────────────────
    /// Title bar background color.
    pub titlebar_bg: Color,
    /// Directory path text color in the title bar.
    pub dir_fg: Color,

    // ── Status bar backgrounds ───────────────────────────────────────
    /// Flash background for success status messages.
    pub status_bg_success: Color,
    /// Flash background for error status messages.
    pub status_bg_error: Color,
    /// Flash background for warning status messages.
    pub status_bg_warning: Color,
    /// Flash background for info status messages.
    pub status_bg_info: Color,

    // ── Comment overlays ─────────────────────────────────────────────
    /// Background for comment preview popups — also the inline-thread surface
    /// for **Claude**-authored comments and replies (the neutral default).
    pub comment_preview_bg: Color,
    /// Inline-thread surface for **user**-authored comments and replies. A
    /// distinct tint from `comment_preview_bg` so "who wrote this" reads at a
    /// glance without parsing the byline.
    pub comment_user_bg: Color,
    /// Text color for reply content.
    pub reply_text: Color,

    // ── Markdown ─────────────────────────────────────────────────────
    /// Background for rendered code — fenced blocks and inline `code` — in
    /// Markdown (change summary, comment bodies). A shaded "card" a step darker
    /// than each theme's base so code reads as inset, GitHub-style, regardless
    /// of the surface drawn behind it.
    pub code_bg: Color,
    /// Foreground for inline `code` chips. A soft pink, distinct from the
    /// heading/accent colours, so code references read as code at a glance.
    pub code_fg: Color,

    // ── Panel surface ────────────────────────────────────────────────
    /// Subtle background for the focused list panel (worktree / explorer).
    /// Retained in the struct for theme compatibility; the layout.rs surface
    /// fill was removed in the transparency sweep so the terminal background
    /// shows through instead.
    #[allow(dead_code)]
    pub panel_focused_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}
