//! GitHub Light palette.

use super::Theme;
use ratatui::style::Color;

impl Theme {
    /// GitHub Light — inspired by GitHub's web UI color system.
    ///
    /// Palette reference (https://primer.style/primitives/colors): bg #ffffff,
    /// fg #24292f, blue #0969da, green #1a7f37, red #cf222e,
    /// amber #9a6700, border #d0d7de, neutral #6e7781.
    pub(super) fn github_light() -> Self {
        Self {
            name: "github-light",
            light: true,
            fg: Color::Rgb(36, 41, 47),         // fg.default
            accent: Color::Rgb(9, 105, 218),    // accent.fg
            muted: Color::Rgb(208, 215, 222),   // border.default — visible separator on white
            success: Color::Rgb(26, 127, 55),   // success.fg
            error: Color::Rgb(207, 34, 46),     // danger.fg
            warning: Color::Rgb(154, 103, 0),   // attention.fg (amber)
            info: Color::Rgb(9, 105, 218),      // accent.fg

            diff_add: Color::Rgb(26, 127, 55),
            diff_add_bg: Color::Rgb(230, 255, 237),  // GitHub addition bg
            diff_del: Color::Rgb(207, 34, 46),
            diff_del_bg: Color::Rgb(255, 235, 233),  // GitHub deletion bg
            diff_add_bg_emphasis: Color::Rgb(204, 255, 220), // stronger addition
            diff_del_bg_emphasis: Color::Rgb(255, 193, 186), // stronger deletion
            diff_section_header: Color::Rgb(110, 119, 129),  // fg.muted

            border_focused: Color::Rgb(9, 105, 218),    // accent.fg
            border_unfocused: Color::Rgb(208, 215, 222), // border.default
            border_secondary: Color::Rgb(175, 184, 193), // border.muted

            selected_bg: Color::Rgb(9, 105, 218),    // accent.fg
            selected_fg: Color::Rgb(255, 255, 255),  // white
            selected_bg_inactive: Color::Rgb(234, 238, 242), // neutral.subtle
            selected_fg_inactive: Color::Rgb(36, 41, 47),    // fg.default

            line_selected_bg: Color::Rgb(234, 238, 242), // neutral.subtle
            line_selected_fg: Color::Rgb(36, 41, 47),    // fg.default

            gutter_selected_bg: Color::Rgb(9, 105, 218),  // accent.fg
            gutter_selected_fg: Color::Rgb(255, 255, 255), // white
            gutter_hover_fg: Color::Rgb(110, 119, 129),   // fg.muted
            gutter_hover_bg: Color::Rgb(246, 248, 250),   // canvas.subtle
            gutter_pending_bg: Color::Rgb(182, 212, 251), // accent.subtle darker
            line_pending_bg: Color::Rgb(240, 245, 255),   // very light blue

            hint: Color::Rgb(110, 119, 129),            // fg.muted
            search_match_fg: Color::Rgb(154, 103, 0),   // attention.fg
            search_match_bg: Color::Rgb(255, 248, 197), // attention.subtle
            search_current_fg: Color::Rgb(36, 41, 47),  // fg.default

            waiting_primary: Color::Rgb(225, 111, 36),  // orange
            waiting_secondary: Color::Rgb(184, 92, 30),

            titlebar_bg: Color::Rgb(246, 248, 250), // canvas.subtle
            dir_fg: Color::Rgb(110, 119, 129),      // fg.muted

            status_bg_success: Color::Rgb(218, 251, 225),
            status_bg_error: Color::Rgb(255, 228, 225),
            status_bg_warning: Color::Rgb(255, 248, 197),
            status_bg_info: Color::Rgb(221, 244, 255),

            comment_preview_bg: Color::Rgb(246, 248, 250), // canvas.subtle
            comment_user_bg: Color::Rgb(232, 245, 241),    // teal-tinted light
            reply_text: Color::Rgb(110, 119, 129),         // fg.muted

            code_bg: Color::Rgb(246, 248, 250),  // canvas.subtle — GitHub inline code bg
            code_fg: Color::Rgb(149, 56, 0),     // brown-red for inline code

            panel_focused_bg: Color::Rgb(240, 243, 246), // between white and canvas.subtle
        }
    }
}
