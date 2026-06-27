//! Theme configuration for UI colors.
//!
//! Defines a set of named colors used throughout the UI, with support for
//! loading custom themes from the configuration.

use ratatui::style::Color;

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

impl Theme {
    /// Load a theme by name. Returns the built-in default if name is unrecognized.
    pub fn from_name(name: &str) -> Self {
        match name {
            "catppuccin-mocha" => Self::catppuccin_mocha(),
            "dracula" => Self::dracula(),
            "nord" => Self::nord(),
            "solarized-dark" => Self::solarized_dark(),
            "tokyo-night" => Self::tokyo_night(),
            "gruvbox" => Self::gruvbox(),
            "rose-pine" => Self::rose_pine(),
            "kanagawa" => Self::kanagawa(),
            "catppuccin-latte" => Self::catppuccin_latte(),
            "solarized-light" => Self::solarized_light(),
            "github-light" => Self::github_light(),
            _ => Self::default(),
        }
    }

    /// All built-in theme names in display order: dark themes first, then light.
    /// Used by the theme-picker UI and OSC11 auto-detection switch.
    pub fn all_names() -> &'static [&'static str] {
        &[
            "catppuccin-mocha",
            "dracula",
            "nord",
            "solarized-dark",
            "tokyo-night",
            "gruvbox",
            "rose-pine",
            "kanagawa",
            "catppuccin-latte",
            "solarized-light",
            "github-light",
        ]
    }

    /// Darken an RGB color by the given factor (0.0 = black, 1.0 = unchanged).
    /// Non-RGB colors are returned unchanged.
    pub fn darken(color: Color, factor: f64) -> Color {
        match color {
            Color::Rgb(r, g, b) => Color::Rgb(
                (r as f64 * factor) as u8,
                (g as f64 * factor) as u8,
                (b as f64 * factor) as u8,
            ),
            other => other,
        }
    }

    // ── Built-in themes ──────────────────────────────────────────────

    /// Default theme — the official Catppuccin Mocha palette.
    ///
    /// Palette reference (https://catppuccin.com/palette): Base #1e1e2e,
    /// Mantle #181825, Surface0 #313244, Surface1 #45475a, Surface2 #585b70,
    /// Overlay0 #6c7086, Overlay1 #7f849c, Text #cdd6f4, Subtext0 #a6adc8,
    /// Mauve #cba6f7, Blue #89b4fa, Sky #89dceb, Green #a6e3a1, Red #f38ba8,
    /// Yellow #f9e2af, Peach #fab387.
    fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin-mocha",
            light: false,
            fg: Color::Rgb(205, 214, 244),      // Text
            accent: Color::Rgb(203, 166, 247),  // Mauve
            muted: Color::Rgb(69, 71, 90),      // Surface1
            success: Color::Rgb(166, 227, 161), // Green
            error: Color::Rgb(243, 139, 168),   // Red
            warning: Color::Rgb(249, 226, 175), // Yellow
            info: Color::Rgb(137, 220, 235),    // Sky

            diff_add: Color::Rgb(166, 227, 161),
            diff_add_bg: Color::Rgb(31, 46, 38),
            diff_del: Color::Rgb(243, 139, 168),
            diff_del_bg: Color::Rgb(49, 33, 42),
            diff_add_bg_emphasis: Color::Rgb(45, 72, 52),
            diff_del_bg_emphasis: Color::Rgb(78, 44, 56),
            diff_section_header: Color::Rgb(127, 132, 156), // Overlay1

            border_focused: Color::Rgb(203, 166, 247), // Mauve
            border_unfocused: Color::Rgb(69, 71, 90),  // Surface1
            border_secondary: Color::Rgb(88, 91, 112), // Surface2

            selected_bg: Color::Rgb(203, 166, 247), // Mauve
            selected_fg: Color::Rgb(30, 30, 46),    // Base
            selected_bg_inactive: Color::Rgb(69, 71, 90), // Surface1
            selected_fg_inactive: Color::Rgb(205, 214, 244), // Text

            line_selected_bg: Color::Rgb(49, 50, 68), // Surface0
            line_selected_fg: Color::Rgb(205, 214, 244), // Text

            gutter_selected_bg: Color::Rgb(137, 180, 250), // Blue
            gutter_selected_fg: Color::Rgb(30, 30, 46),    // Base
            gutter_hover_fg: Color::Rgb(127, 132, 156),    // Overlay1
            gutter_hover_bg: Color::Rgb(40, 41, 56),
            gutter_pending_bg: Color::Rgb(54, 64, 98),
            line_pending_bg: Color::Rgb(40, 41, 58),

            hint: Color::Rgb(108, 112, 134),            // Overlay0
            search_match_fg: Color::Rgb(249, 226, 175), // Yellow
            search_match_bg: Color::Rgb(249, 226, 175), // Yellow
            search_current_fg: Color::Rgb(30, 30, 46),  // Base

            waiting_primary: Color::Rgb(250, 179, 135), // Peach
            waiting_secondary: Color::Rgb(200, 140, 100),

            titlebar_bg: Color::Rgb(24, 24, 37), // Mantle
            dir_fg: Color::Rgb(108, 112, 134),   // Overlay0

            status_bg_success: Color::Rgb(25, 40, 30),
            status_bg_error: Color::Rgb(48, 28, 36),
            status_bg_warning: Color::Rgb(46, 40, 26),
            status_bg_info: Color::Rgb(24, 36, 48),

            comment_preview_bg: Color::Rgb(49, 50, 68), // Surface0
            comment_user_bg: Color::Rgb(40, 56, 58),    // teal-leaning surface
            reply_text: Color::Rgb(166, 173, 200),      // Subtext0

            code_bg: Color::Rgb(17, 17, 27), // Crust
            code_fg: Color::Rgb(245, 194, 231), // Pink

            panel_focused_bg: Color::Rgb(40, 41, 58), // between Base and Surface0
        }
    }

    fn dracula() -> Self {
        Self {
            name: "dracula",
            light: false,
            fg: Color::Rgb(248, 248, 242),
            accent: Color::Rgb(255, 121, 198),
            muted: Color::Rgb(68, 71, 90),
            success: Color::Rgb(80, 250, 123),
            error: Color::Rgb(255, 85, 85),
            warning: Color::Rgb(241, 250, 140),
            info: Color::Rgb(139, 233, 253),

            diff_add: Color::Rgb(80, 250, 123),
            diff_add_bg: Color::Rgb(20, 60, 20),
            diff_del: Color::Rgb(255, 85, 85),
            diff_del_bg: Color::Rgb(60, 20, 20),
            diff_add_bg_emphasis: Color::Rgb(40, 100, 40),
            diff_del_bg_emphasis: Color::Rgb(100, 40, 40),
            diff_section_header: Color::Rgb(98, 114, 164),

            border_focused: Color::Rgb(255, 121, 198),
            border_unfocused: Color::Rgb(68, 71, 90),
            border_secondary: Color::Rgb(98, 114, 164),

            selected_bg: Color::Rgb(255, 121, 198),
            selected_fg: Color::Rgb(40, 42, 54),
            selected_bg_inactive: Color::Rgb(68, 71, 90),
            selected_fg_inactive: Color::Rgb(248, 248, 242),

            line_selected_bg: Color::Rgb(68, 71, 90),
            line_selected_fg: Color::Rgb(248, 248, 242),

            gutter_selected_bg: Color::Rgb(98, 114, 164),
            gutter_selected_fg: Color::Rgb(40, 42, 54),
            gutter_hover_fg: Color::Rgb(98, 114, 164),
            gutter_hover_bg: Color::Rgb(55, 58, 75),
            gutter_pending_bg: Color::Rgb(60, 65, 100),
            line_pending_bg: Color::Rgb(50, 52, 68),

            hint: Color::Rgb(98, 114, 164),
            search_match_fg: Color::Rgb(241, 250, 140),
            search_match_bg: Color::Rgb(241, 250, 140),
            search_current_fg: Color::Rgb(40, 42, 54),

            waiting_primary: Color::Rgb(255, 184, 108),
            waiting_secondary: Color::Rgb(200, 140, 80),

            titlebar_bg: Color::Rgb(40, 42, 54),
            dir_fg: Color::Rgb(98, 114, 164),

            status_bg_success: Color::Rgb(20, 50, 20),
            status_bg_error: Color::Rgb(60, 15, 15),
            status_bg_warning: Color::Rgb(50, 40, 10),
            status_bg_info: Color::Rgb(15, 30, 50),

            comment_preview_bg: Color::Rgb(52, 54, 76),
            comment_user_bg: Color::Rgb(44, 64, 60),
            reply_text: Color::Rgb(189, 147, 249),

            code_bg: Color::Rgb(33, 34, 44),
            code_fg: Color::Rgb(255, 159, 212), // soft pink

            panel_focused_bg: Color::Rgb(50, 52, 66),
        }
    }

    fn nord() -> Self {
        Self {
            name: "nord",
            light: false,
            fg: Color::Rgb(216, 222, 233),
            accent: Color::Rgb(136, 192, 208),
            muted: Color::Rgb(59, 66, 82),
            success: Color::Rgb(163, 190, 140),
            error: Color::Rgb(191, 97, 106),
            warning: Color::Rgb(235, 203, 139),
            info: Color::Rgb(129, 161, 193),

            diff_add: Color::Rgb(163, 190, 140),
            diff_add_bg: Color::Rgb(20, 40, 20),
            diff_del: Color::Rgb(191, 97, 106),
            diff_del_bg: Color::Rgb(40, 20, 20),
            diff_add_bg_emphasis: Color::Rgb(40, 70, 40),
            diff_del_bg_emphasis: Color::Rgb(70, 40, 40),
            diff_section_header: Color::Rgb(76, 86, 106),

            border_focused: Color::Rgb(136, 192, 208),
            border_unfocused: Color::Rgb(59, 66, 82),
            border_secondary: Color::Rgb(76, 86, 106),

            selected_bg: Color::Rgb(136, 192, 208),
            selected_fg: Color::Rgb(46, 52, 64),
            selected_bg_inactive: Color::Rgb(59, 66, 82),
            selected_fg_inactive: Color::Rgb(216, 222, 233),

            line_selected_bg: Color::Rgb(59, 66, 82),
            line_selected_fg: Color::Rgb(216, 222, 233),

            gutter_selected_bg: Color::Rgb(129, 161, 193),
            gutter_selected_fg: Color::Rgb(46, 52, 64),
            gutter_hover_fg: Color::Rgb(76, 86, 106),
            gutter_hover_bg: Color::Rgb(50, 56, 70),
            gutter_pending_bg: Color::Rgb(70, 85, 110),
            line_pending_bg: Color::Rgb(50, 58, 72),

            hint: Color::Rgb(76, 86, 106),
            search_match_fg: Color::Rgb(235, 203, 139),
            search_match_bg: Color::Rgb(235, 203, 139),
            search_current_fg: Color::Rgb(46, 52, 64),

            waiting_primary: Color::Rgb(208, 135, 112),
            waiting_secondary: Color::Rgb(170, 100, 80),

            titlebar_bg: Color::Rgb(46, 52, 64),
            dir_fg: Color::Rgb(76, 86, 106),

            status_bg_success: Color::Rgb(20, 40, 20),
            status_bg_error: Color::Rgb(45, 20, 22),
            status_bg_warning: Color::Rgb(45, 38, 15),
            status_bg_info: Color::Rgb(15, 30, 45),

            comment_preview_bg: Color::Rgb(56, 62, 82),
            // nord15 (aurora purple): distinct from `info` (129,161,193), which
            // styles reply *authors* — identical colours made them merge.
            comment_user_bg: Color::Rgb(48, 66, 68),
            reply_text: Color::Rgb(180, 142, 173),

            code_bg: Color::Rgb(40, 45, 56),
            code_fg: Color::Rgb(212, 150, 180), // muted pink

            panel_focused_bg: Color::Rgb(56, 62, 78),
        }
    }

    fn solarized_dark() -> Self {
        Self {
            name: "solarized-dark",
            light: false,
            fg: Color::Rgb(131, 148, 150),
            accent: Color::Rgb(181, 137, 0),
            muted: Color::Rgb(0, 43, 54),
            success: Color::Rgb(133, 153, 0),
            error: Color::Rgb(220, 50, 47),
            warning: Color::Rgb(181, 137, 0),
            info: Color::Rgb(38, 139, 210),

            diff_add: Color::Rgb(133, 153, 0),
            diff_add_bg: Color::Rgb(15, 35, 15),
            diff_del: Color::Rgb(220, 50, 47),
            diff_del_bg: Color::Rgb(40, 15, 15),
            diff_add_bg_emphasis: Color::Rgb(30, 60, 30),
            diff_del_bg_emphasis: Color::Rgb(70, 30, 30),
            diff_section_header: Color::Rgb(88, 110, 117),

            border_focused: Color::Rgb(181, 137, 0),
            border_unfocused: Color::Rgb(0, 43, 54),
            border_secondary: Color::Rgb(88, 110, 117),

            selected_bg: Color::Rgb(181, 137, 0),
            selected_fg: Color::Rgb(0, 43, 54),
            selected_bg_inactive: Color::Rgb(7, 54, 66),
            selected_fg_inactive: Color::Rgb(131, 148, 150),

            line_selected_bg: Color::Rgb(7, 54, 66),
            line_selected_fg: Color::Rgb(131, 148, 150),

            gutter_selected_bg: Color::Rgb(38, 139, 210),
            gutter_selected_fg: Color::Rgb(0, 43, 54),
            gutter_hover_fg: Color::Rgb(88, 110, 117),
            gutter_hover_bg: Color::Rgb(10, 55, 68),
            gutter_pending_bg: Color::Rgb(15, 75, 115),
            line_pending_bg: Color::Rgb(3, 48, 60),

            hint: Color::Rgb(88, 110, 117),
            search_match_fg: Color::Rgb(181, 137, 0),
            search_match_bg: Color::Rgb(181, 137, 0),
            search_current_fg: Color::Rgb(0, 43, 54),

            waiting_primary: Color::Rgb(203, 75, 22),
            waiting_secondary: Color::Rgb(160, 60, 18),

            titlebar_bg: Color::Rgb(7, 54, 66),
            dir_fg: Color::Rgb(88, 110, 117),

            status_bg_success: Color::Rgb(10, 35, 10),
            status_bg_error: Color::Rgb(45, 10, 10),
            status_bg_warning: Color::Rgb(40, 30, 5),
            status_bg_info: Color::Rgb(5, 25, 45),

            comment_preview_bg: Color::Rgb(15, 64, 84),
            comment_user_bg: Color::Rgb(15, 74, 64),
            reply_text: Color::Rgb(108, 113, 196),

            code_bg: Color::Rgb(0, 33, 42), // base03, a step under base
            code_fg: Color::Rgb(211, 54, 130), // magenta

            panel_focused_bg: Color::Rgb(8, 52, 64),
        }
    }

    fn tokyo_night() -> Self {
        Self {
            name: "tokyo-night",
            light: false,
            fg: Color::Rgb(192, 202, 245),
            accent: Color::Rgb(122, 162, 247),
            muted: Color::Rgb(59, 66, 97),
            success: Color::Rgb(158, 206, 106),
            error: Color::Rgb(247, 118, 142),
            warning: Color::Rgb(224, 175, 104),
            info: Color::Rgb(125, 207, 255),

            diff_add: Color::Rgb(158, 206, 106),
            diff_add_bg: Color::Rgb(15, 40, 15),
            diff_del: Color::Rgb(247, 118, 142),
            diff_del_bg: Color::Rgb(45, 15, 20),
            diff_add_bg_emphasis: Color::Rgb(30, 70, 30),
            diff_del_bg_emphasis: Color::Rgb(80, 30, 35),
            diff_section_header: Color::Rgb(86, 95, 137),

            border_focused: Color::Rgb(122, 162, 247),
            border_unfocused: Color::Rgb(59, 66, 97),
            border_secondary: Color::Rgb(65, 72, 104),

            selected_bg: Color::Rgb(122, 162, 247),
            selected_fg: Color::Rgb(26, 27, 38),
            selected_bg_inactive: Color::Rgb(59, 66, 97),
            selected_fg_inactive: Color::Rgb(192, 202, 245),

            line_selected_bg: Color::Rgb(41, 46, 66),
            line_selected_fg: Color::Rgb(192, 202, 245),

            gutter_selected_bg: Color::Rgb(122, 162, 247),
            gutter_selected_fg: Color::Rgb(26, 27, 38),
            gutter_hover_fg: Color::Rgb(86, 95, 137),
            gutter_hover_bg: Color::Rgb(36, 40, 58),
            gutter_pending_bg: Color::Rgb(55, 72, 130),
            line_pending_bg: Color::Rgb(35, 38, 55),

            hint: Color::Rgb(65, 72, 104),
            search_match_fg: Color::Rgb(224, 175, 104),
            search_match_bg: Color::Rgb(224, 175, 104),
            search_current_fg: Color::Rgb(26, 27, 38),

            waiting_primary: Color::Rgb(255, 158, 100),
            waiting_secondary: Color::Rgb(200, 120, 70),

            titlebar_bg: Color::Rgb(26, 27, 38),
            dir_fg: Color::Rgb(65, 72, 104),

            status_bg_success: Color::Rgb(15, 35, 15),
            status_bg_error: Color::Rgb(45, 12, 18),
            status_bg_warning: Color::Rgb(45, 35, 12),
            status_bg_info: Color::Rgb(12, 25, 50),

            comment_preview_bg: Color::Rgb(42, 44, 66),
            comment_user_bg: Color::Rgb(36, 56, 58),
            reply_text: Color::Rgb(125, 207, 255),

            code_bg: Color::Rgb(22, 22, 32),
            code_fg: Color::Rgb(247, 140, 180), // pink

            panel_focused_bg: Color::Rgb(36, 38, 52),
        }
    }

    fn gruvbox() -> Self {
        Self {
            name: "gruvbox",
            light: false,
            fg: Color::Rgb(235, 219, 178),
            accent: Color::Rgb(250, 189, 47),
            muted: Color::Rgb(60, 56, 54),
            success: Color::Rgb(184, 187, 38),
            error: Color::Rgb(251, 73, 52),
            warning: Color::Rgb(250, 189, 47),
            info: Color::Rgb(131, 165, 152),

            diff_add: Color::Rgb(184, 187, 38),
            diff_add_bg: Color::Rgb(20, 35, 8),
            diff_del: Color::Rgb(251, 73, 52),
            diff_del_bg: Color::Rgb(45, 12, 8),
            diff_add_bg_emphasis: Color::Rgb(40, 65, 15),
            diff_del_bg_emphasis: Color::Rgb(80, 25, 15),
            diff_section_header: Color::Rgb(102, 92, 84),

            border_focused: Color::Rgb(250, 189, 47),
            border_unfocused: Color::Rgb(60, 56, 54),
            border_secondary: Color::Rgb(102, 92, 84),

            selected_bg: Color::Rgb(250, 189, 47),
            selected_fg: Color::Rgb(40, 40, 40),
            selected_bg_inactive: Color::Rgb(60, 56, 54),
            selected_fg_inactive: Color::Rgb(235, 219, 178),

            line_selected_bg: Color::Rgb(60, 56, 54),
            line_selected_fg: Color::Rgb(235, 219, 178),

            gutter_selected_bg: Color::Rgb(131, 165, 152),
            gutter_selected_fg: Color::Rgb(40, 40, 40),
            gutter_hover_fg: Color::Rgb(102, 92, 84),
            gutter_hover_bg: Color::Rgb(55, 52, 50),
            gutter_pending_bg: Color::Rgb(75, 95, 88),
            line_pending_bg: Color::Rgb(50, 48, 46),

            hint: Color::Rgb(102, 92, 84),
            search_match_fg: Color::Rgb(250, 189, 47),
            search_match_bg: Color::Rgb(250, 189, 47),
            search_current_fg: Color::Rgb(40, 40, 40),

            waiting_primary: Color::Rgb(254, 128, 25),
            waiting_secondary: Color::Rgb(200, 100, 20),

            titlebar_bg: Color::Rgb(50, 48, 47),
            dir_fg: Color::Rgb(102, 92, 84),

            status_bg_success: Color::Rgb(18, 32, 8),
            status_bg_error: Color::Rgb(50, 12, 8),
            status_bg_warning: Color::Rgb(50, 38, 8),
            status_bg_info: Color::Rgb(15, 30, 30),

            comment_preview_bg: Color::Rgb(62, 58, 68),
            comment_user_bg: Color::Rgb(52, 64, 54),
            reply_text: Color::Rgb(131, 165, 152),

            code_bg: Color::Rgb(29, 32, 33), // bg0_h
            code_fg: Color::Rgb(211, 134, 155), // purple-pink

            panel_focused_bg: Color::Rgb(58, 55, 52),
        }
    }

    fn rose_pine() -> Self {
        Self {
            name: "rose-pine",
            light: false,
            fg: Color::Rgb(224, 222, 244),
            accent: Color::Rgb(235, 188, 186),
            muted: Color::Rgb(57, 53, 82),
            success: Color::Rgb(156, 207, 216),
            error: Color::Rgb(235, 111, 146),
            warning: Color::Rgb(246, 193, 119),
            info: Color::Rgb(196, 167, 231),

            diff_add: Color::Rgb(156, 207, 216),
            diff_add_bg: Color::Rgb(15, 35, 38),
            diff_del: Color::Rgb(235, 111, 146),
            diff_del_bg: Color::Rgb(45, 15, 25),
            diff_add_bg_emphasis: Color::Rgb(28, 60, 65),
            diff_del_bg_emphasis: Color::Rgb(75, 25, 40),
            diff_section_header: Color::Rgb(110, 106, 134),

            border_focused: Color::Rgb(235, 188, 186),
            border_unfocused: Color::Rgb(57, 53, 82),
            border_secondary: Color::Rgb(110, 106, 134),

            selected_bg: Color::Rgb(235, 188, 186),
            selected_fg: Color::Rgb(25, 23, 36),
            selected_bg_inactive: Color::Rgb(57, 53, 82),
            selected_fg_inactive: Color::Rgb(224, 222, 244),

            line_selected_bg: Color::Rgb(57, 53, 82),
            line_selected_fg: Color::Rgb(224, 222, 244),

            gutter_selected_bg: Color::Rgb(196, 167, 231),
            gutter_selected_fg: Color::Rgb(25, 23, 36),
            gutter_hover_fg: Color::Rgb(110, 106, 134),
            gutter_hover_bg: Color::Rgb(45, 42, 65),
            gutter_pending_bg: Color::Rgb(110, 90, 145),
            line_pending_bg: Color::Rgb(45, 42, 65),

            hint: Color::Rgb(110, 106, 134),
            search_match_fg: Color::Rgb(246, 193, 119),
            search_match_bg: Color::Rgb(246, 193, 119),
            search_current_fg: Color::Rgb(25, 23, 36),

            waiting_primary: Color::Rgb(234, 154, 151),
            waiting_secondary: Color::Rgb(190, 120, 118),

            titlebar_bg: Color::Rgb(25, 23, 36),
            dir_fg: Color::Rgb(110, 106, 134),

            status_bg_success: Color::Rgb(15, 35, 38),
            status_bg_error: Color::Rgb(45, 12, 22),
            status_bg_warning: Color::Rgb(45, 35, 15),
            status_bg_info: Color::Rgb(20, 18, 40),

            comment_preview_bg: Color::Rgb(48, 44, 70),
            comment_user_bg: Color::Rgb(40, 60, 62),
            reply_text: Color::Rgb(196, 167, 231),

            code_bg: Color::Rgb(20, 18, 30),
            code_fg: Color::Rgb(235, 159, 188), // rose pink

            panel_focused_bg: Color::Rgb(38, 35, 52),
        }
    }

    fn kanagawa() -> Self {
        Self {
            name: "kanagawa",
            light: false,
            fg: Color::Rgb(220, 215, 186),
            accent: Color::Rgb(127, 180, 202),
            muted: Color::Rgb(54, 54, 70),
            success: Color::Rgb(152, 187, 108),
            error: Color::Rgb(195, 64, 67),
            warning: Color::Rgb(226, 194, 95),
            info: Color::Rgb(127, 180, 202),

            diff_add: Color::Rgb(152, 187, 108),
            diff_add_bg: Color::Rgb(18, 35, 12),
            diff_del: Color::Rgb(195, 64, 67),
            diff_del_bg: Color::Rgb(40, 12, 12),
            diff_add_bg_emphasis: Color::Rgb(35, 60, 22),
            diff_del_bg_emphasis: Color::Rgb(72, 22, 22),
            diff_section_header: Color::Rgb(84, 84, 109),

            border_focused: Color::Rgb(127, 180, 202),
            border_unfocused: Color::Rgb(54, 54, 70),
            border_secondary: Color::Rgb(84, 84, 109),

            selected_bg: Color::Rgb(127, 180, 202),
            selected_fg: Color::Rgb(22, 22, 29),
            selected_bg_inactive: Color::Rgb(54, 54, 70),
            selected_fg_inactive: Color::Rgb(220, 215, 186),

            line_selected_bg: Color::Rgb(54, 54, 70),
            line_selected_fg: Color::Rgb(220, 215, 186),

            gutter_selected_bg: Color::Rgb(127, 180, 202),
            gutter_selected_fg: Color::Rgb(22, 22, 29),
            gutter_hover_fg: Color::Rgb(84, 84, 109),
            gutter_hover_bg: Color::Rgb(38, 38, 52),
            gutter_pending_bg: Color::Rgb(65, 100, 120),
            line_pending_bg: Color::Rgb(40, 40, 55),

            hint: Color::Rgb(84, 84, 109),
            search_match_fg: Color::Rgb(226, 194, 95),
            search_match_bg: Color::Rgb(226, 194, 95),
            search_current_fg: Color::Rgb(22, 22, 29),

            waiting_primary: Color::Rgb(255, 160, 102),
            waiting_secondary: Color::Rgb(200, 120, 75),

            titlebar_bg: Color::Rgb(22, 22, 29),
            dir_fg: Color::Rgb(84, 84, 109),

            status_bg_success: Color::Rgb(15, 32, 10),
            status_bg_error: Color::Rgb(42, 10, 10),
            status_bg_warning: Color::Rgb(42, 35, 10),
            status_bg_info: Color::Rgb(12, 28, 40),

            comment_preview_bg: Color::Rgb(42, 42, 62),
            comment_user_bg: Color::Rgb(38, 56, 56),
            reply_text: Color::Rgb(127, 180, 202),

            code_bg: Color::Rgb(18, 18, 24), // sumiInk0
            code_fg: Color::Rgb(210, 126, 153), // sakura pink

            panel_focused_bg: Color::Rgb(34, 34, 46),
        }
    }

    /// Catppuccin Latte — the official light variant of Catppuccin.
    ///
    /// Palette reference (https://catppuccin.com/palette): Base #eff1f5,
    /// Mantle #e6e9ef, Surface0 #ccd0da, Surface1 #bcc0cc, Surface2 #acb0be,
    /// Overlay0 #9ca0b0, Overlay1 #8c8fa1, Text #4c4f69, Subtext0 #6c6f85,
    /// Mauve #8839ef, Blue #1e66f5, Sky #04a5e5, Green #40a02b, Red #d20f39,
    /// Yellow #df8e1d, Peach #fe640b, Pink #ea76cb.
    fn catppuccin_latte() -> Self {
        Self {
            name: "catppuccin-latte",
            light: true,
            fg: Color::Rgb(76, 79, 105),       // Text
            accent: Color::Rgb(136, 57, 239),  // Mauve
            muted: Color::Rgb(188, 192, 204),  // Surface1
            success: Color::Rgb(64, 160, 43),  // Green
            error: Color::Rgb(210, 15, 57),    // Red
            warning: Color::Rgb(223, 142, 29), // Yellow
            info: Color::Rgb(4, 165, 229),     // Sky

            diff_add: Color::Rgb(64, 160, 43),
            diff_add_bg: Color::Rgb(220, 245, 210),
            diff_del: Color::Rgb(210, 15, 57),
            diff_del_bg: Color::Rgb(252, 220, 228),
            diff_add_bg_emphasis: Color::Rgb(196, 236, 182),
            diff_del_bg_emphasis: Color::Rgb(248, 196, 210),
            diff_section_header: Color::Rgb(140, 143, 161), // Overlay1

            border_focused: Color::Rgb(136, 57, 239), // Mauve
            border_unfocused: Color::Rgb(188, 192, 204), // Surface1
            border_secondary: Color::Rgb(172, 176, 190), // Surface2

            selected_bg: Color::Rgb(136, 57, 239),  // Mauve
            selected_fg: Color::Rgb(239, 241, 245), // Base (near-white)
            selected_bg_inactive: Color::Rgb(204, 208, 218), // Surface0
            selected_fg_inactive: Color::Rgb(76, 79, 105),   // Text

            line_selected_bg: Color::Rgb(204, 208, 218), // Surface0
            line_selected_fg: Color::Rgb(76, 79, 105),   // Text

            gutter_selected_bg: Color::Rgb(30, 102, 245), // Blue
            gutter_selected_fg: Color::Rgb(239, 241, 245), // Base
            gutter_hover_fg: Color::Rgb(140, 143, 161),   // Overlay1
            gutter_hover_bg: Color::Rgb(224, 227, 236),   // between Base and Surface0
            gutter_pending_bg: Color::Rgb(189, 211, 252), // light blue
            line_pending_bg: Color::Rgb(236, 240, 252),   // very light blue

            hint: Color::Rgb(156, 160, 176),            // Overlay0
            search_match_fg: Color::Rgb(223, 142, 29),  // Yellow
            search_match_bg: Color::Rgb(252, 238, 190), // light yellow
            search_current_fg: Color::Rgb(76, 79, 105), // Text

            waiting_primary: Color::Rgb(254, 100, 11),  // Peach
            waiting_secondary: Color::Rgb(212, 82, 9),

            titlebar_bg: Color::Rgb(230, 233, 239), // Mantle
            dir_fg: Color::Rgb(156, 160, 176),      // Overlay0

            status_bg_success: Color::Rgb(210, 240, 200),
            status_bg_error: Color::Rgb(252, 212, 220),
            status_bg_warning: Color::Rgb(252, 232, 196),
            status_bg_info: Color::Rgb(208, 232, 252),

            comment_preview_bg: Color::Rgb(204, 208, 218), // Surface0
            comment_user_bg: Color::Rgb(196, 228, 220),    // teal-tinted light
            reply_text: Color::Rgb(108, 111, 133),         // Subtext0

            code_bg: Color::Rgb(230, 233, 239),  // Mantle
            code_fg: Color::Rgb(234, 118, 203),  // Pink

            panel_focused_bg: Color::Rgb(220, 223, 232), // between Base and Surface0
        }
    }

    /// Solarized Light — Ethan Schoonover's light variant.
    ///
    /// Palette reference (https://ethanschoonover.com/solarized/): base3 #fdf6e3,
    /// base2 #eee8d5, base1 #93a1a1, base0 #839496, base00 #657b83,
    /// base01 #586e75, yellow #b58900, orange #cb4b16, red #dc322f,
    /// magenta #d33682, violet #6c71c4, blue #268bd2, cyan #2aa198, green #859900.
    fn solarized_light() -> Self {
        Self {
            name: "solarized-light",
            light: true,
            fg: Color::Rgb(101, 123, 131),     // base00 — body text
            accent: Color::Rgb(38, 139, 210),  // blue
            muted: Color::Rgb(147, 161, 161),  // base1 — medium gray for separators
            success: Color::Rgb(133, 153, 0),  // green
            error: Color::Rgb(220, 50, 47),    // red
            warning: Color::Rgb(181, 137, 0),  // yellow
            info: Color::Rgb(42, 161, 152),    // cyan

            diff_add: Color::Rgb(133, 153, 0),
            diff_add_bg: Color::Rgb(232, 244, 210),
            diff_del: Color::Rgb(220, 50, 47),
            diff_del_bg: Color::Rgb(252, 228, 224),
            diff_add_bg_emphasis: Color::Rgb(212, 236, 184),
            diff_del_bg_emphasis: Color::Rgb(248, 206, 200),
            diff_section_header: Color::Rgb(147, 161, 161), // base1

            border_focused: Color::Rgb(38, 139, 210),  // blue
            border_unfocused: Color::Rgb(147, 161, 161), // base1
            border_secondary: Color::Rgb(131, 148, 150), // base0

            selected_bg: Color::Rgb(38, 139, 210),   // blue
            selected_fg: Color::Rgb(253, 246, 227),  // base3 (lightest)
            selected_bg_inactive: Color::Rgb(238, 232, 213), // base2
            selected_fg_inactive: Color::Rgb(101, 123, 131), // base00

            line_selected_bg: Color::Rgb(238, 232, 213), // base2
            line_selected_fg: Color::Rgb(101, 123, 131), // base00

            gutter_selected_bg: Color::Rgb(42, 161, 152), // cyan
            gutter_selected_fg: Color::Rgb(253, 246, 227), // base3
            gutter_hover_fg: Color::Rgb(131, 148, 150),   // base0
            gutter_hover_bg: Color::Rgb(245, 238, 218),   // between base3 and base2
            gutter_pending_bg: Color::Rgb(196, 224, 244), // light blue
            line_pending_bg: Color::Rgb(240, 248, 253),   // very light blue

            hint: Color::Rgb(131, 148, 150),             // base0
            search_match_fg: Color::Rgb(181, 137, 0),    // yellow
            search_match_bg: Color::Rgb(253, 240, 184),  // light yellow
            search_current_fg: Color::Rgb(253, 246, 227), // base3

            waiting_primary: Color::Rgb(203, 75, 22),   // orange
            waiting_secondary: Color::Rgb(160, 60, 18),

            titlebar_bg: Color::Rgb(238, 232, 213), // base2
            dir_fg: Color::Rgb(131, 148, 150),      // base0

            status_bg_success: Color::Rgb(220, 240, 192),
            status_bg_error: Color::Rgb(252, 224, 220),
            status_bg_warning: Color::Rgb(252, 232, 192),
            status_bg_info: Color::Rgb(208, 232, 248),

            comment_preview_bg: Color::Rgb(238, 232, 213), // base2
            comment_user_bg: Color::Rgb(216, 236, 232),    // teal-tinted light
            reply_text: Color::Rgb(88, 110, 117),          // base01

            code_bg: Color::Rgb(228, 222, 200),  // slightly darker than base2
            code_fg: Color::Rgb(211, 54, 130),   // magenta

            panel_focused_bg: Color::Rgb(232, 226, 208), // between base3 and base2
        }
    }

    /// GitHub Light — inspired by GitHub's web UI color system.
    ///
    /// Palette reference (https://primer.style/primitives/colors): bg #ffffff,
    /// fg #24292f, blue #0969da, green #1a7f37, red #cf222e,
    /// amber #9a6700, border #d0d7de, neutral #6e7781.
    fn github_light() -> Self {
        Self {
            name: "github-light",
            light: true,
            fg: Color::Rgb(36, 41, 47),         // fg.default
            accent: Color::Rgb(9, 105, 218),    // accent.fg
            muted: Color::Rgb(208, 215, 222),   // border.default — visible separator on white
            success: Color::Rgb(26, 127, 55),   // success.fg
            error: Color::Rgb(207, 34, 46),     // danger.fg
            warning: Color::Rgb(154, 103, 0),   // attention.fg (amber)
            info: Color::Rgb(9, 105, 218),      // accent.fg

            diff_add: Color::Rgb(26, 127, 55),
            diff_add_bg: Color::Rgb(230, 255, 237),  // GitHub addition bg
            diff_del: Color::Rgb(207, 34, 46),
            diff_del_bg: Color::Rgb(255, 235, 233),  // GitHub deletion bg
            diff_add_bg_emphasis: Color::Rgb(204, 255, 220), // stronger addition
            diff_del_bg_emphasis: Color::Rgb(255, 193, 186), // stronger deletion
            diff_section_header: Color::Rgb(110, 119, 129),  // fg.muted

            border_focused: Color::Rgb(9, 105, 218),    // accent.fg
            border_unfocused: Color::Rgb(208, 215, 222), // border.default
            border_secondary: Color::Rgb(175, 184, 193), // border.muted

            selected_bg: Color::Rgb(9, 105, 218),    // accent.fg
            selected_fg: Color::Rgb(255, 255, 255),  // white
            selected_bg_inactive: Color::Rgb(234, 238, 242), // neutral.subtle
            selected_fg_inactive: Color::Rgb(36, 41, 47),    // fg.default

            line_selected_bg: Color::Rgb(234, 238, 242), // neutral.subtle
            line_selected_fg: Color::Rgb(36, 41, 47),    // fg.default

            gutter_selected_bg: Color::Rgb(9, 105, 218),  // accent.fg
            gutter_selected_fg: Color::Rgb(255, 255, 255), // white
            gutter_hover_fg: Color::Rgb(110, 119, 129),   // fg.muted
            gutter_hover_bg: Color::Rgb(246, 248, 250),   // canvas.subtle
            gutter_pending_bg: Color::Rgb(182, 212, 251), // accent.subtle darker
            line_pending_bg: Color::Rgb(240, 245, 255),   // very light blue

            hint: Color::Rgb(110, 119, 129),            // fg.muted
            search_match_fg: Color::Rgb(154, 103, 0),   // attention.fg
            search_match_bg: Color::Rgb(255, 248, 197), // attention.subtle
            search_current_fg: Color::Rgb(36, 41, 47),  // fg.default

            waiting_primary: Color::Rgb(225, 111, 36),  // orange
            waiting_secondary: Color::Rgb(184, 92, 30),

            titlebar_bg: Color::Rgb(246, 248, 250), // canvas.subtle
            dir_fg: Color::Rgb(110, 119, 129),      // fg.muted

            status_bg_success: Color::Rgb(218, 251, 225),
            status_bg_error: Color::Rgb(255, 228, 225),
            status_bg_warning: Color::Rgb(255, 248, 197),
            status_bg_info: Color::Rgb(221, 244, 255),

            comment_preview_bg: Color::Rgb(246, 248, 250), // canvas.subtle
            comment_user_bg: Color::Rgb(232, 245, 241),    // teal-tinted light
            reply_text: Color::Rgb(110, 119, 129),         // fg.muted

            code_bg: Color::Rgb(246, 248, 250),  // canvas.subtle — GitHub inline code bg
            code_fg: Color::Rgb(149, 56, 0),     // brown-red for inline code

            panel_focused_bg: Color::Rgb(240, 243, 246), // between white and canvas.subtle
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_light_themes_have_light_true() {
        assert!(Theme::from_name("catppuccin-latte").light);
        assert!(Theme::from_name("solarized-light").light);
        assert!(Theme::from_name("github-light").light);
    }

    #[test]
    fn from_name_dark_themes_have_light_false() {
        assert!(!Theme::from_name("catppuccin-mocha").light);
        assert!(!Theme::from_name("dracula").light);
        assert!(!Theme::from_name("nord").light);
        assert!(!Theme::from_name("solarized-dark").light);
        assert!(!Theme::from_name("tokyo-night").light);
        assert!(!Theme::from_name("gruvbox").light);
        assert!(!Theme::from_name("rose-pine").light);
        assert!(!Theme::from_name("kanagawa").light);
    }

    #[test]
    fn all_names_contains_all_eleven_themes() {
        let names = Theme::all_names();
        assert_eq!(names.len(), 11);
        assert!(names.contains(&"catppuccin-mocha"));
        assert!(names.contains(&"dracula"));
        assert!(names.contains(&"nord"));
        assert!(names.contains(&"solarized-dark"));
        assert!(names.contains(&"tokyo-night"));
        assert!(names.contains(&"gruvbox"));
        assert!(names.contains(&"rose-pine"));
        assert!(names.contains(&"kanagawa"));
        assert!(names.contains(&"catppuccin-latte"));
        assert!(names.contains(&"solarized-light"));
        assert!(names.contains(&"github-light"));
    }

    #[test]
    fn all_names_dark_before_light() {
        let names = Theme::all_names();
        let last_dark = names
            .iter()
            .rposition(|n| !Theme::from_name(n).light)
            .expect("at least one dark theme");
        let first_light = names
            .iter()
            .position(|n| Theme::from_name(n).light)
            .expect("at least one light theme");
        assert!(last_dark < first_light, "dark themes must precede light themes");
    }

    #[test]
    fn unknown_name_falls_back_to_default() {
        // Unknown names return the default (catppuccin-mocha), which is dark.
        let theme = Theme::from_name("does-not-exist");
        assert!(!theme.light);
    }

    /// Every name in `all_names()` must round-trip through `from_name` and
    /// return the same canonical `name` field. A mismatch means a theme was
    /// registered in one list but omitted or renamed in the other.
    #[test]
    fn all_names_round_trip_through_from_name() {
        for &n in Theme::all_names() {
            let theme = Theme::from_name(n);
            assert_eq!(
                theme.name, n,
                "Theme::from_name(\"{n}\").name == \"{}\", expected \"{n}\" — \
                 check that from_name has a match arm for this theme",
                theme.name
            );
        }
    }
}
