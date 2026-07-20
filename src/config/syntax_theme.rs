//! Syntect syntax-highlighting theme resolution.

use super::ViewerConfig;

/// Resolve the syntect syntax-highlighting theme for a given viewer config.
///
/// When `viewer.syntax_theme_file` is set, the file is loaded directly
/// (falling back to a built-in theme on error). Otherwise, the built-in
/// syntect theme that best matches `viewer.theme` is returned.
///
/// The mapping from conductor UI theme names to syntect names covers the four
/// original dark themes; all other names fall back to `base16-mocha.dark`
/// (the same drift that existed before this helper was extracted — expanding
/// the mapping table is out of scope here).
pub fn syntect_theme_for(
    viewer: &ViewerConfig,
    ts: &syntect::highlighting::ThemeSet,
) -> syntect::highlighting::Theme {
    // Map the conductor viewer theme name to the corresponding syntect key.
    // Dark themes map to matching dark syntect themes; light themes map to
    // light syntect built-ins so code blocks remain readable on a light UI.
    let builtin_name = |theme: &str| -> &str {
        match theme {
            // Dark themes
            "catppuccin-mocha" => "base16-mocha.dark",
            "dracula" => "base16-eighties.dark",
            "nord" => "base16-ocean.dark",
            "solarized-dark" => "Solarized (dark)",
            // Light themes — map to light syntect built-ins to preserve
            // readability on a light background.
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
