//! Dracula palette.

use super::Theme;
use ratatui::style::Color;

impl Theme {
    pub(super) fn dracula() -> Self {
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
}
