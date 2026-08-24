//! Solarized パレット: Ethan Schoonover の Solarized の Dark/Light バリアント。

use super::Theme;
use ratatui::style::Color;

impl Theme {
    pub(super) fn solarized_dark() -> Self {
        Self {
            name: "solarized-dark",
            light: false,
            fg: Color::Rgb(131, 148, 150),
            accent: Color::Rgb(181, 137, 0),
            muted: Color::Rgb(0, 43, 54),
            success: Color::Rgb(133, 153, 0),
            error: Color::Rgb(220, 50, 47),
            warning: Color::Rgb(181, 137, 0),
            info: Color::Rgb(38, 139, 210),

            diff_add: Color::Rgb(133, 153, 0),
            diff_add_bg: Color::Rgb(15, 35, 15),
            diff_del: Color::Rgb(220, 50, 47),
            diff_del_bg: Color::Rgb(40, 15, 15),
            diff_add_bg_emphasis: Color::Rgb(30, 60, 30),
            diff_del_bg_emphasis: Color::Rgb(70, 30, 30),
            diff_section_header: Color::Rgb(88, 110, 117),

            border_focused: Color::Rgb(181, 137, 0),
            border_unfocused: Color::Rgb(0, 43, 54),
            border_secondary: Color::Rgb(88, 110, 117),

            selected_bg: Color::Rgb(181, 137, 0),
            selected_fg: Color::Rgb(0, 43, 54),
            selected_bg_inactive: Color::Rgb(7, 54, 66),
            selected_fg_inactive: Color::Rgb(131, 148, 150),

            line_selected_bg: Color::Rgb(7, 54, 66),
            line_selected_fg: Color::Rgb(131, 148, 150),

            gutter_selected_bg: Color::Rgb(38, 139, 210),
            gutter_selected_fg: Color::Rgb(0, 43, 54),
            gutter_hover_fg: Color::Rgb(88, 110, 117),
            gutter_hover_bg: Color::Rgb(10, 55, 68),

            hint: Color::Rgb(88, 110, 117),
            search_match_fg: Color::Rgb(181, 137, 0),
            search_match_bg: Color::Rgb(181, 137, 0),
            search_current_fg: Color::Rgb(0, 43, 54),

            waiting_primary: Color::Rgb(203, 75, 22),
            waiting_secondary: Color::Rgb(160, 60, 18),

            titlebar_bg: Color::Rgb(7, 54, 66),
            dir_fg: Color::Rgb(88, 110, 117),

            status_bg_success: Color::Rgb(10, 35, 10),
            status_bg_error: Color::Rgb(45, 10, 10),
            status_bg_warning: Color::Rgb(40, 30, 5),
            status_bg_info: Color::Rgb(5, 25, 45),

            comment_preview_bg: Color::Rgb(15, 64, 84),
            comment_user_bg: Color::Rgb(15, 74, 64),
            reply_text: Color::Rgb(108, 113, 196),

            code_bg: Color::Rgb(0, 33, 42), // base03、base より一段暗い
            code_fg: Color::Rgb(211, 54, 130), // magenta
        }
    }

    /// Solarized Light — Ethan Schoonover のライトバリアント。
    ///
    /// パレット参照(https://ethanschoonover.com/solarized/): base3 #fdf6e3,
    /// base2 #eee8d5, base1 #93a1a1, base0 #839496, base00 #657b83,
    /// base01 #586e75, yellow #b58900, orange #cb4b16, red #dc322f,
    /// magenta #d33682, violet #6c71c4, blue #268bd2, cyan #2aa198, green #859900.
    pub(super) fn solarized_light() -> Self {
        Self {
            name: "solarized-light",
            light: true,
            fg: Color::Rgb(101, 123, 131),    // base00 — 本文テキスト
            accent: Color::Rgb(38, 139, 210), // blue
            muted: Color::Rgb(147, 161, 161), // base1 — 区切り線用の中間グレー
            success: Color::Rgb(133, 153, 0), // green
            error: Color::Rgb(220, 50, 47),   // red
            warning: Color::Rgb(181, 137, 0), // yellow
            info: Color::Rgb(42, 161, 152),   // cyan

            diff_add: Color::Rgb(133, 153, 0),
            diff_add_bg: Color::Rgb(232, 244, 210),
            diff_del: Color::Rgb(220, 50, 47),
            diff_del_bg: Color::Rgb(252, 228, 224),
            diff_add_bg_emphasis: Color::Rgb(212, 236, 184),
            diff_del_bg_emphasis: Color::Rgb(248, 206, 200),
            diff_section_header: Color::Rgb(147, 161, 161), // base1

            border_focused: Color::Rgb(38, 139, 210), // blue
            border_unfocused: Color::Rgb(147, 161, 161), // base1
            border_secondary: Color::Rgb(131, 148, 150), // base0

            selected_bg: Color::Rgb(38, 139, 210),  // blue
            selected_fg: Color::Rgb(253, 246, 227), // base3(最も明るい)
            selected_bg_inactive: Color::Rgb(238, 232, 213), // base2
            selected_fg_inactive: Color::Rgb(101, 123, 131), // base00

            line_selected_bg: Color::Rgb(238, 232, 213), // base2
            line_selected_fg: Color::Rgb(101, 123, 131), // base00

            gutter_selected_bg: Color::Rgb(42, 161, 152), // cyan
            gutter_selected_fg: Color::Rgb(253, 246, 227), // base3
            gutter_hover_fg: Color::Rgb(131, 148, 150),   // base0
            gutter_hover_bg: Color::Rgb(245, 238, 218),   // base3 と base2 の中間

            hint: Color::Rgb(131, 148, 150),              // base0
            search_match_fg: Color::Rgb(181, 137, 0),     // yellow
            search_match_bg: Color::Rgb(253, 240, 184),   // 明るい黄色
            search_current_fg: Color::Rgb(253, 246, 227), // base3

            waiting_primary: Color::Rgb(203, 75, 22), // orange
            waiting_secondary: Color::Rgb(160, 60, 18),

            titlebar_bg: Color::Rgb(238, 232, 213), // base2
            dir_fg: Color::Rgb(131, 148, 150),      // base0

            status_bg_success: Color::Rgb(220, 240, 192),
            status_bg_error: Color::Rgb(252, 224, 220),
            status_bg_warning: Color::Rgb(252, 232, 192),
            status_bg_info: Color::Rgb(208, 232, 248),

            comment_preview_bg: Color::Rgb(238, 232, 213), // base2
            comment_user_bg: Color::Rgb(216, 236, 232),    // teal がかった明るい色
            reply_text: Color::Rgb(88, 110, 117),          // base01

            code_bg: Color::Rgb(228, 222, 200), // base2 よりわずかに暗い
            code_fg: Color::Rgb(211, 54, 130),  // magenta
        }
    }
}
