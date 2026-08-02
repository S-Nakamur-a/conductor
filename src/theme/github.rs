//! GitHub Light パレット。

use super::Theme;
use ratatui::style::Color;

impl Theme {
    /// GitHub Light — GitHub の Web UI のカラーシステムを参考にしたテーマ。
    ///
    /// パレット参照(https://primer.style/primitives/colors): bg #ffffff,
    /// fg #24292f, blue #0969da, green #1a7f37, red #cf222e,
    /// amber #9a6700, border #d0d7de, neutral #6e7781.
    pub(super) fn github_light() -> Self {
        Self {
            name: "github-light",
            light: true,
            fg: Color::Rgb(36, 41, 47),         // fg.default
            accent: Color::Rgb(9, 105, 218),    // accent.fg
            muted: Color::Rgb(208, 215, 222),   // border.default — 白背景でも視認できる区切り線
            success: Color::Rgb(26, 127, 55),   // success.fg
            error: Color::Rgb(207, 34, 46),     // danger.fg
            warning: Color::Rgb(154, 103, 0),   // attention.fg (amber)
            info: Color::Rgb(9, 105, 218),      // accent.fg

            diff_add: Color::Rgb(26, 127, 55),
            diff_add_bg: Color::Rgb(230, 255, 237),  // GitHub の追加行背景
            diff_del: Color::Rgb(207, 34, 46),
            diff_del_bg: Color::Rgb(255, 235, 233),  // GitHub の削除行背景
            diff_add_bg_emphasis: Color::Rgb(204, 255, 220), // より強調した追加
            diff_del_bg_emphasis: Color::Rgb(255, 193, 186), // より強調した削除
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

            hint: Color::Rgb(110, 119, 129),            // fg.muted
            search_match_fg: Color::Rgb(154, 103, 0),   // attention.fg
            search_match_bg: Color::Rgb(255, 248, 197), // attention.subtle
            search_current_fg: Color::Rgb(36, 41, 47),  // fg.default

            waiting_primary: Color::Rgb(225, 111, 36),  // オレンジ
            waiting_secondary: Color::Rgb(184, 92, 30),

            titlebar_bg: Color::Rgb(246, 248, 250), // canvas.subtle
            dir_fg: Color::Rgb(110, 119, 129),      // fg.muted

            status_bg_success: Color::Rgb(218, 251, 225),
            status_bg_error: Color::Rgb(255, 228, 225),
            status_bg_warning: Color::Rgb(255, 248, 197),
            status_bg_info: Color::Rgb(221, 244, 255),

            comment_preview_bg: Color::Rgb(246, 248, 250), // canvas.subtle
            comment_user_bg: Color::Rgb(232, 245, 241),    // ティールがかった淡色
            reply_text: Color::Rgb(110, 119, 129),         // fg.muted

            code_bg: Color::Rgb(246, 248, 250),  // canvas.subtle — GitHub のインラインコード背景
            code_fg: Color::Rgb(149, 56, 0),     // インラインコード用の褐色がかった赤

        }
    }
}
