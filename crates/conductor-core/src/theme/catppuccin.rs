//! Catppuccin パレット: Mocha(ダーク、アプリのデフォルト)と Latte(ライト)。

use super::Theme;
use ratatui::style::Color;

impl Theme {
    /// デフォルトテーマ — 公式の Catppuccin Mocha パレット。
    ///
    /// 公式パレット: https://catppuccin.com/palette
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
            comment_user_bg: Color::Rgb(40, 56, 58),
            reply_text: Color::Rgb(166, 173, 200), // Subtext0

            code_bg: Color::Rgb(17, 17, 27),    // Crust
            code_fg: Color::Rgb(245, 194, 231), // Pink
        }
    }

    /// Catppuccin Latte — Catppuccin の公式ライトバリアント。
    ///
    /// 公式パレット: https://catppuccin.com/palette
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
            selected_fg: Color::Rgb(239, 241, 245), // Base
            selected_bg_inactive: Color::Rgb(204, 208, 218), // Surface0
            selected_fg_inactive: Color::Rgb(76, 79, 105), // Text

            line_selected_bg: Color::Rgb(204, 208, 218), // Surface0
            line_selected_fg: Color::Rgb(76, 79, 105),   // Text

            gutter_selected_bg: Color::Rgb(30, 102, 245), // Blue
            gutter_selected_fg: Color::Rgb(239, 241, 245), // Base
            gutter_hover_fg: Color::Rgb(140, 143, 161),   // Overlay1
            gutter_hover_bg: Color::Rgb(224, 227, 236),   // Base と Surface0 の中間

            hint: Color::Rgb(156, 160, 176),           // Overlay0
            search_match_fg: Color::Rgb(223, 142, 29), // Yellow
            search_match_bg: Color::Rgb(252, 238, 190),
            search_current_fg: Color::Rgb(76, 79, 105), // Text

            waiting_primary: Color::Rgb(254, 100, 11), // Peach
            waiting_secondary: Color::Rgb(212, 82, 9),

            titlebar_bg: Color::Rgb(230, 233, 239), // Mantle
            dir_fg: Color::Rgb(156, 160, 176),      // Overlay0

            status_bg_success: Color::Rgb(210, 240, 200),
            status_bg_error: Color::Rgb(252, 212, 220),
            status_bg_warning: Color::Rgb(252, 232, 196),
            status_bg_info: Color::Rgb(208, 232, 252),

            comment_preview_bg: Color::Rgb(204, 208, 218), // Surface0
            comment_user_bg: Color::Rgb(196, 228, 220),
            reply_text: Color::Rgb(108, 111, 133), // Subtext0

            code_bg: Color::Rgb(230, 233, 239), // Mantle
            code_fg: Color::Rgb(234, 118, 203), // Pink
        }
    }
}
