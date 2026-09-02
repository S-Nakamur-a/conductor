//! UI カラーテーマ。
//!
//! 名前付きカラーの集合 [Theme] と 11 の組み込みパレット。パレットは 1 テーマ 1 ファイルで、
//! それぞれが Theme のコンストラクタを持つ。色の演算 (明暗、補色、高コントラスト化) は
//! color_ops にある。

use ratatui::style::Color;

mod catppuccin;
mod color_ops;
mod dracula;
mod github;
mod gruvbox;
mod hsl;
mod kanagawa;
mod nord;
mod rose_pine;
mod solarized;
mod tokyo_night;

#[cfg(test)]
mod tests;

/// アプリケーションのカラーテーマ。
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    /// 端末背景の自動判定とテーマピッカーの明暗タグが読む。高コントラスト化の向きも決める。
    pub light: bool,

    pub fg: Color,
    pub accent: Color,
    /// 枠線や区切りに使う面の色。文字色には使わない: 7 テーマで背景に近く、
    /// solarized-dark では背景そのもの。文字を薄くするなら hint。
    pub muted: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub info: Color,

    pub diff_add: Color,
    pub diff_add_bg: Color,
    pub diff_del: Color,
    pub diff_del_bg: Color,
    pub diff_add_bg_emphasis: Color,
    pub diff_del_bg_emphasis: Color,
    pub diff_section_header: Color,

    pub border_focused: Color,
    pub border_unfocused: Color,
    pub border_secondary: Color,

    pub selected_bg: Color,
    pub selected_fg: Color,
    pub selected_bg_inactive: Color,
    pub selected_fg_inactive: Color,

    pub line_selected_bg: Color,
    pub line_selected_fg: Color,

    pub gutter_selected_bg: Color,
    pub gutter_selected_fg: Color,
    pub gutter_hover_fg: Color,
    pub gutter_hover_bg: Color,

    pub hint: Color,
    pub search_match_fg: Color,
    pub search_match_bg: Color,
    pub search_current_fg: Color,

    pub waiting_primary: Color,
    pub waiting_secondary: Color,

    pub titlebar_bg: Color,
    pub dir_fg: Color,

    pub status_bg_success: Color,
    pub status_bg_error: Color,
    pub status_bg_warning: Color,
    pub status_bg_info: Color,

    /// comment_user_bg と必ず違う色味にする。署名を読まずに誰が書いたかを
    /// 判別できるのはこの差だけ。
    pub comment_preview_bg: Color,
    pub comment_user_bg: Color,
    pub reply_text: Color,

    /// 基調色より一段暗くする。背後に何があっても凹んで見せるため。
    pub code_bg: Color,
    /// 見出しやアクセントと別系統の色にして、コード参照だと一目で分かるようにする。
    pub code_fg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}

type Builtin = (&'static str, fn() -> Theme);

/// 表示順。ダークが先、ライトが後。
const BUILTIN: &[Builtin] = &[
    ("catppuccin-mocha", Theme::catppuccin_mocha),
    ("dracula", Theme::dracula),
    ("nord", Theme::nord),
    ("solarized-dark", Theme::solarized_dark),
    ("tokyo-night", Theme::tokyo_night),
    ("gruvbox", Theme::gruvbox),
    ("rose-pine", Theme::rose_pine),
    ("kanagawa", Theme::kanagawa),
    ("catppuccin-latte", Theme::catppuccin_latte),
    ("solarized-light", Theme::solarized_light),
    ("github-light", Theme::github_light),
];

const BUILTIN_NAMES: [&str; BUILTIN.len()] = {
    let mut names = [""; BUILTIN.len()];
    let mut i = 0;
    while i < BUILTIN.len() {
        names[i] = BUILTIN[i].0;
        i += 1;
    }
    names
};

impl Theme {
    /// 名前で組み込みテーマを引く。未知の名前は既定のテーマになる。
    pub fn from_name(name: &str) -> Self {
        BUILTIN
            .iter()
            .find(|(builtin, _)| *builtin == name)
            .map_or_else(Self::default, |(_, build)| build())
    }

    /// 全組み込みテーマ名を表示順で返す。
    pub fn all_names() -> &'static [&'static str] {
        &BUILTIN_NAMES
    }
}
