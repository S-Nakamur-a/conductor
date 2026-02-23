//! Theme configuration for UI colors.
//!
//! Defines a set of named colors used throughout the UI, with support for
//! loading custom themes from the configuration.

use ratatui::style::Color;

/// A color theme for the application.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Theme {
    /// Background color for the main area.
    pub bg: Color,
    /// Foreground color for normal text.
    pub fg: Color,
    /// Accent color (used for highlights, selections).
    pub accent: Color,
    /// Color for borders and separators.
    pub border: Color,
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
}

#[allow(dead_code)]
impl Theme {
    /// Load a theme by name. Returns the built-in default if name is unrecognized.
    pub fn from_name(name: &str) -> Self {
        match name {
            "catppuccin-mocha" => Self::catppuccin_mocha(),
            "dracula" => Self::dracula(),
            "nord" => Self::nord(),
            "solarized-dark" => Self::solarized_dark(),
            _ => Self::default(),
        }
    }

    /// Default theme (similar to current hardcoded colors).
    fn catppuccin_mocha() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::White,
            accent: Color::Yellow,
            border: Color::Cyan,
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
        }
    }

    fn dracula() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Rgb(248, 248, 242),     // foreground
            accent: Color::Rgb(255, 121, 198), // pink
            border: Color::Rgb(98, 114, 164),  // comment
            muted: Color::Rgb(68, 71, 90),     // current line
            success: Color::Rgb(80, 250, 123), // green
            error: Color::Rgb(255, 85, 85),    // red
            warning: Color::Rgb(241, 250, 140), // yellow
            info: Color::Rgb(139, 233, 253),   // cyan
            diff_add: Color::Rgb(80, 250, 123),
            diff_add_bg: Color::Rgb(20, 60, 20),
            diff_del: Color::Rgb(255, 85, 85),
            diff_del_bg: Color::Rgb(60, 20, 20),
            diff_add_bg_emphasis: Color::Rgb(40, 100, 40),
            diff_del_bg_emphasis: Color::Rgb(100, 40, 40),
        }
    }

    fn nord() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Rgb(216, 222, 233),      // snow storm
            accent: Color::Rgb(136, 192, 208),  // frost
            border: Color::Rgb(76, 86, 106),    // polar night
            muted: Color::Rgb(59, 66, 82),      // polar night
            success: Color::Rgb(163, 190, 140), // aurora green
            error: Color::Rgb(191, 97, 106),    // aurora red
            warning: Color::Rgb(235, 203, 139), // aurora yellow
            info: Color::Rgb(129, 161, 193),    // frost
            diff_add: Color::Rgb(163, 190, 140),
            diff_add_bg: Color::Rgb(20, 40, 20),
            diff_del: Color::Rgb(191, 97, 106),
            diff_del_bg: Color::Rgb(40, 20, 20),
            diff_add_bg_emphasis: Color::Rgb(40, 70, 40),
            diff_del_bg_emphasis: Color::Rgb(70, 40, 40),
        }
    }

    fn solarized_dark() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Rgb(131, 148, 150),    // base0
            accent: Color::Rgb(181, 137, 0),  // yellow
            border: Color::Rgb(88, 110, 117), // base01
            muted: Color::Rgb(0, 43, 54),     // base03
            success: Color::Rgb(133, 153, 0), // green
            error: Color::Rgb(220, 50, 47),   // red
            warning: Color::Rgb(181, 137, 0), // yellow
            info: Color::Rgb(38, 139, 210),   // blue
            diff_add: Color::Rgb(133, 153, 0),
            diff_add_bg: Color::Rgb(15, 35, 15),
            diff_del: Color::Rgb(220, 50, 47),
            diff_del_bg: Color::Rgb(40, 15, 15),
            diff_add_bg_emphasis: Color::Rgb(30, 60, 30),
            diff_del_bg_emphasis: Color::Rgb(70, 30, 30),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}
