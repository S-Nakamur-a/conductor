//! Theme configuration for UI colors.
//!
//! Defines a set of named colors used throughout the UI, with support for
//! loading custom themes from the configuration.

use ratatui::style::Color;

/// A color theme for the application.
#[derive(Debug, Clone)]
pub struct Theme {
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
    /// Background for comment preview popups.
    pub comment_preview_bg: Color,
    /// Text color for reply content.
    pub reply_text: Color,
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
            _ => Self::default(),
        }
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

    /// Default theme — matches the original hardcoded colors exactly.
    fn catppuccin_mocha() -> Self {
        Self {
            fg: Color::White,
            accent: Color::Yellow,
            muted: Color::DarkGray,
            success: Color::Green,
            error: Color::Red,
            warning: Color::Yellow,
            info: Color::Cyan,

            diff_add: Color::Green,
            diff_add_bg: Color::Rgb(0, 40, 0),
            diff_del: Color::Red,
            diff_del_bg: Color::Rgb(40, 0, 0),
            diff_add_bg_emphasis: Color::Rgb(0, 80, 0),
            diff_del_bg_emphasis: Color::Rgb(80, 0, 0),

            border_focused: Color::Yellow,
            border_unfocused: Color::DarkGray,
            border_secondary: Color::White,

            selected_bg: Color::Yellow,
            selected_fg: Color::Black,
            selected_bg_inactive: Color::DarkGray,
            selected_fg_inactive: Color::Black,

            line_selected_bg: Color::DarkGray,
            line_selected_fg: Color::White,

            gutter_selected_bg: Color::LightBlue,
            gutter_selected_fg: Color::Black,
            gutter_hover_fg: Color::Gray,
            gutter_pending_bg: Color::Rgb(50, 70, 90),
            line_pending_bg: Color::Rgb(40, 40, 50),

            hint: Color::Gray,
            search_match_fg: Color::Yellow,
            search_match_bg: Color::Yellow,
            search_current_fg: Color::Black,

            waiting_primary: Color::Rgb(255, 165, 0),
            waiting_secondary: Color::Rgb(200, 120, 0),

            titlebar_bg: Color::DarkGray,
            dir_fg: Color::Gray,

            status_bg_success: Color::Rgb(0, 30, 0),
            status_bg_error: Color::Rgb(40, 0, 0),
            status_bg_warning: Color::Rgb(40, 30, 0),
            status_bg_info: Color::Rgb(0, 20, 40),

            comment_preview_bg: Color::Rgb(30, 30, 50),
            reply_text: Color::Rgb(180, 180, 200),
        }
    }

    fn dracula() -> Self {
        Self {
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

            comment_preview_bg: Color::Rgb(40, 42, 60),
            reply_text: Color::Rgb(189, 147, 249),
        }
    }

    fn nord() -> Self {
        Self {
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

            comment_preview_bg: Color::Rgb(46, 52, 70),
            reply_text: Color::Rgb(129, 161, 193),
        }
    }

    fn solarized_dark() -> Self {
        Self {
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

            comment_preview_bg: Color::Rgb(7, 54, 72),
            reply_text: Color::Rgb(108, 113, 196),
        }
    }

    fn tokyo_night() -> Self {
        Self {
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

            comment_preview_bg: Color::Rgb(30, 32, 50),
            reply_text: Color::Rgb(125, 207, 255),
        }
    }

    fn gruvbox() -> Self {
        Self {
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

            comment_preview_bg: Color::Rgb(50, 48, 55),
            reply_text: Color::Rgb(131, 165, 152),
        }
    }

    fn rose_pine() -> Self {
        Self {
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

            comment_preview_bg: Color::Rgb(35, 33, 55),
            reply_text: Color::Rgb(196, 167, 231),
        }
    }

    fn kanagawa() -> Self {
        Self {
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

            comment_preview_bg: Color::Rgb(30, 30, 45),
            reply_text: Color::Rgb(127, 180, 202),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}
