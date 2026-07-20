//! Rosé Pine palette.

use super::Theme;
use ratatui::style::Color;

impl Theme {
    pub(super) fn rose_pine() -> Self {
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
}
