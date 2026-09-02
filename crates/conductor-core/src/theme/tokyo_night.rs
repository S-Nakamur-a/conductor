//! Tokyo Night パレット。

use super::Theme;
use ratatui::style::Color;

impl Theme {
    pub(super) fn tokyo_night() -> Self {
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
            code_fg: Color::Rgb(247, 140, 180),
        }
    }
}
