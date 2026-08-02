//! syntect のシンタックスハイライトテーマ解決。

use super::ViewerConfig;

/// viewer 設定から syntect のシンタックスハイライトテーマを解決する。
///
/// viewer.syntax_theme_file が設定されていればそのファイルを直接読み込む
/// (失敗時は組み込みテーマにフォールバック)。未設定なら viewer.theme に
/// 最も近い組み込み syntect テーマを返す。
///
/// conductor の UI テーマ名から syntect テーマ名への対応は元々あった4つの
/// ダークテーマのみをカバーしており、それ以外は base16-mocha.dark に
/// フォールバックする。このヘルパーを切り出す前からあったずれであり、
/// 対応表を拡充するのはここでは対象外。
pub fn syntect_theme_for(
    viewer: &ViewerConfig,
    ts: &syntect::highlighting::ThemeSet,
) -> syntect::highlighting::Theme {
    // conductor の viewer テーマ名を対応する syntect キーにマップする。
    // ダークテーマは対応するダークな syntect テーマへ、ライトテーマは
    // 明るい UI でもコードブロックが読めるようライトな syntect 組み込みへ。
    let builtin_name = |theme: &str| -> &str {
        match theme {
            // ダークテーマ
            "catppuccin-mocha" => "base16-mocha.dark",
            "dracula" => "base16-eighties.dark",
            "nord" => "base16-ocean.dark",
            "solarized-dark" => "Solarized (dark)",
            // ライトテーマ — 明るい背景でも読めるようライトな syntect 組み込みへ。
            "catppuccin-latte" => "base16-ocean.light",
            "solarized-light" => "Solarized (light)",
            "github-light" => "InspiredGitHub",
            _ => "base16-mocha.dark",
        }
    };
    let fallback = || {
        let name = builtin_name(&viewer.theme);
        ts.themes
            .get(name)
            .cloned()
            .unwrap_or_else(|| ts.themes["base16-mocha.dark"].clone())
    };

    if let Some(ref path) = viewer.syntax_theme_file {
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
