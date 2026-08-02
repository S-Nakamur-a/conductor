//! syntect_theme_for のダーク/ライトテーマ対応と、未知テーマ・theme ファイル
//! 不在時のフォールバック挙動のテスト。

use super::*;

/// ヘルパー: syntect テーマの背景輝度を推定する(0〜255の範囲)。
fn theme_bg_luma(theme: &syntect::highlighting::Theme) -> f32 {
    theme
        .settings
        .background
        .map(|c| 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32)
        .unwrap_or(0.0)
}

/// syntect_theme_for: dark UI テーマは暗い syntect テーマを返すこと。
#[test]
fn syntect_theme_for_dark_themes_return_dark_syntect() {
    let ts = syntect::highlighting::ThemeSet::load_defaults();
    let viewer_with = |theme: &str| ViewerConfig {
        theme: theme.to_string(),
        syntax_theme_file: None,
        ..Default::default()
    };

    for name in &["catppuccin-mocha", "dracula", "nord", "solarized-dark"] {
        let theme = syntect_theme_for(&viewer_with(name), &ts);
        assert!(
            theme_bg_luma(&theme) < 128.0,
            "dark conductor theme '{name}' must map to a dark syntect theme (luma={:.0})",
            theme_bg_luma(&theme)
        );
    }
}

/// syntect_theme_for: ライトテーマがライト系 syntect テーマを返すこと。
#[test]
fn syntect_theme_for_light_themes_use_light_syntect() {
    let ts = syntect::highlighting::ThemeSet::load_defaults();
    let viewer_with = |theme: &str| ViewerConfig {
        theme: theme.to_string(),
        syntax_theme_file: None,
        ..Default::default()
    };

    // ライト UI テーマはライトな syntect 組み込みテーマにマップされること。
    for name in &["catppuccin-latte", "solarized-light", "github-light"] {
        let theme = syntect_theme_for(&viewer_with(name), &ts);
        assert!(
            theme_bg_luma(&theme) >= 128.0,
            "light conductor theme '{name}' must map to a light syntect theme (luma={:.0})",
            theme_bg_luma(&theme)
        );
    }
}

/// syntect_theme_for: 未知テーマ名はパニックせず mocha フォールバック。
#[test]
fn syntect_theme_for_unknown_falls_back_without_panic() {
    let ts = syntect::highlighting::ThemeSet::load_defaults();
    let viewer = ViewerConfig {
        theme: String::from("nonexistent-theme-xyz"),
        syntax_theme_file: None,
        ..Default::default()
    };
    let _ = syntect_theme_for(&viewer, &ts); // パニックしないこと
}

/// syntect_theme_for: 存在しないパスの syntax_theme_file はパニックしないこと。
#[test]
fn syntect_theme_for_missing_theme_file_falls_back_without_panic() {
    let ts = syntect::highlighting::ThemeSet::load_defaults();
    let viewer = ViewerConfig {
        theme: String::from("catppuccin-mocha"),
        syntax_theme_file: Some(String::from("/nonexistent/path/theme.tmTheme")),
        ..Default::default()
    };
    let _ = syntect_theme_for(&viewer, &ts); // パニックしないこと
}
