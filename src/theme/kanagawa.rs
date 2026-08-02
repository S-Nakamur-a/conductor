//! Kanagawa パレット。

use super::Theme;
use ratatui::style::Color;

impl Theme {
    pub(super) fn kanagawa() -> Self {
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
            code_fg: Color::Rgb(210, 126, 153), // 桜色のピンク

        }
    }
}
