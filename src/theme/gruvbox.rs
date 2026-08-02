//! Gruvbox パレット。

use super::Theme;
use ratatui::style::Color;

impl Theme {
    pub(super) fn gruvbox() -> Self {
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
            code_fg: Color::Rgb(211, 134, 155), // 紫がかったピンク

        }
    }
}
