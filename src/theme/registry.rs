//! テーマ名の解決。設定ファイルの theme 文字列を組み込みパレットのコンストラクタに
//! マッピングし、テーマピッカー用に全組み込み名を一覧できるようにする。

use super::Theme;

impl Theme {
    /// 名前でテーマを読み込む。未知の名前なら組み込みのデフォルトを返す。
    pub fn from_name(name: &str) -> Self {
        match name {
            "catppuccin-mocha" => Self::catppuccin_mocha(),
            "dracula" => Self::dracula(),
            "nord" => Self::nord(),
            "solarized-dark" => Self::solarized_dark(),
            "tokyo-night" => Self::tokyo_night(),
            "gruvbox" => Self::gruvbox(),
            "rose-pine" => Self::rose_pine(),
            "kanagawa" => Self::kanagawa(),
            "catppuccin-latte" => Self::catppuccin_latte(),
            "solarized-light" => Self::solarized_light(),
            "github-light" => Self::github_light(),
            _ => Self::default(),
        }
    }

    /// 表示順に並んだ全組み込みテーマ名(先にダークテーマ、次にライトテーマ)。
    /// テーマピッカー UI と OSC11 自動判定の切り替えで使う。
    pub fn all_names() -> &'static [&'static str] {
        &[
            "catppuccin-mocha",
            "dracula",
            "nord",
            "solarized-dark",
            "tokyo-night",
            "gruvbox",
            "rose-pine",
            "kanagawa",
            "catppuccin-latte",
            "solarized-light",
            "github-light",
        ]
    }
}
