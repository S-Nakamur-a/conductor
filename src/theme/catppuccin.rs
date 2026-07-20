//! Catppuccin palettes: Mocha (dark, the app default) and Latte (light).

use super::Theme;
use ratatui::style::Color;

impl Theme {
    /// Default theme — the official Catppuccin Mocha palette.
    ///
    /// Palette reference (https://catppuccin.com/palette): Base #1e1e2e,
    /// Mantle #181825, Surface0 #313244, Surface1 #45475a, Surface2 #585b70,
    /// Overlay0 #6c7086, Overlay1 #7f849c, Text #cdd6f4, Subtext0 #a6adc8,
    /// Mauve #cba6f7, Blue #89b4fa, Sky #89dceb, Green #a6e3a1, Red #f38ba8,
    /// Yellow #f9e2af, Peach #fab387.
    pub(super) fn catppuccin_mocha() -> Self {
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

    /// Catppuccin Latte — the official light variant of Catppuccin.
    ///
    /// Palette reference (https://catppuccin.com/palette): Base #eff1f5,
    /// Mantle #e6e9ef, Surface0 #ccd0da, Surface1 #bcc0cc, Surface2 #acb0be,
    /// Overlay0 #9ca0b0, Overlay1 #8c8fa1, Text #4c4f69, Subtext0 #6c6f85,
    /// Mauve #8839ef, Blue #1e66f5, Sky #04a5e5, Green #40a02b, Red #d20f39,
    /// Yellow #df8e1d, Peach #fe640b, Pink #ea76cb.
    pub(super) fn catppuccin_latte() -> Self {
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
}
