//! Nord palette.

use super::Theme;
use ratatui::style::Color;

impl Theme {
    pub(super) fn nord() -> Self {
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
}
