//! syntect のシンタックスハイライトテーマ解決。

use super::Config;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

/// 有効な UI テーマ名に対応する two-face 組み込みテーマを返す。
///
/// 対応表は theme::Theme::all_names() の全エントリを網羅する。ここに漏れが
/// あるとそのテーマだけシンタックスハイライトが別テーマの配色のまま残り、
/// 「テーマを切り替えてもコードの色が変わらない」ように見えてしまうので、
/// 網羅は tests_syntax_theme.rs のテストで固定してある。
///
/// syntect 同梱の base16-* 系ではなく two-face (bat 由来) を使うのは、
/// 色を付けるスコープの数が段違いに多いため。base16-mocha.dark はスコープ
/// 規則が 47 件しかなく、TypeScript ではトークンの約半分が無着色の
/// デフォルト前景のまま残る。Catppuccin Mocha は 185 件で、同じファイルの
/// 約 9 割に色が付く。
fn embedded_theme_for(theme: &str) -> EmbeddedThemeName {
    match theme {
        // ダークテーマ — 同名の組み込みがあるものはそのまま対応させる。
        "catppuccin-mocha" => EmbeddedThemeName::CatppuccinMocha,
        "dracula" => EmbeddedThemeName::Dracula,
        "nord" => EmbeddedThemeName::Nord,
        "solarized-dark" => EmbeddedThemeName::SolarizedDark,
        "gruvbox" => EmbeddedThemeName::GruvboxDark,
        // 同名の組み込みが無いダークテーマは、色相と明度が最も近いもので代用する。
        // 背景色は使わない (ハイライト結果の背景は Reset に潰している) ので、
        // 前景アクセントの傾向だけで選んでいる。
        // tokyo-night は青紫寄りのダーク。
        "tokyo-night" => EmbeddedThemeName::CatppuccinMacchiato,
        // rose-pine は彩度を落としたパステル。
        "rose-pine" => EmbeddedThemeName::CatppuccinFrappe,
        // kanagawa は暖色寄りで、前景 #dcd7ba が gruvbox の #ebdbb2 に近い。
        "kanagawa" => EmbeddedThemeName::GruvboxDark,
        // ライトテーマ — 明るい背景でもコントラストが出るライト系を割り当てる。
        "catppuccin-latte" => EmbeddedThemeName::CatppuccinLatte,
        "solarized-light" => EmbeddedThemeName::SolarizedLight,
        "github-light" => EmbeddedThemeName::InspiredGithub,
        // 未知の名前。theme::Theme::from_name も既定テーマに落ちるので合わせる。
        _ => EmbeddedThemeName::CatppuccinMocha,
    }
}

/// 設定から syntect のシンタックスハイライトテーマを解決する。
///
/// viewer.syntax_theme_file が設定されていればそのファイルを直接読み込む
/// (失敗時は組み込みテーマにフォールバック)。未設定なら有効な UI テーマ名
/// (ui.theme、無ければ後方互換で viewer.theme) に対応する組み込みテーマを返す。
///
/// 参照するテーマ名を Config::theme_name() に一本化しているのが要点。以前は
/// viewer.theme だけを見ていたので、テーマピッカーが書き込む ui.theme とずれ、
/// UI の配色だけが変わってコードの配色が取り残されていた。
pub fn syntect_theme_for(
    cfg: &Config,
    themes: &EmbeddedLazyThemeSet,
) -> syntect::highlighting::Theme {
    let fallback = || themes.get(embedded_theme_for(cfg.theme_name())).clone();

    if let Some(ref path) = cfg.viewer.syntax_theme_file {
        match syntect::highlighting::ThemeSet::get_theme(path) {
            Ok(theme) => theme,
            Err(e) => {
                log::warn!(
                    "failed to load syntax theme file {path}: {e}; falling back to built-in theme"
                );
                fallback()
            }
        }
    } else {
        fallback()
    }
}

/// syntect テーマの解決結果を一意に決める入力の指紋。
///
/// ハイライト結果のキャッシュを、テーマが実際に変わったときだけ捨てるために使う。
/// 同じ文字列なら同じテーマが解決されるので、キャッシュを持ち越してよい。
pub fn syntax_theme_id(cfg: &Config) -> String {
    match cfg.viewer.syntax_theme_file {
        // ファイル指定時はパスだけで決まる (テーマ名は結果に影響しない)。
        Some(ref path) => format!("file:{path}"),
        None => format!("builtin:{}", cfg.theme_name()),
    }
}
