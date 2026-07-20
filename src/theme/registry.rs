//! Theme-name resolution: maps the config `theme` string to a built-in
//! palette constructor, and lists all built-in names for the theme picker.

use super::Theme;

impl Theme {
    /// Load a theme by name. Returns the built-in default if name is unrecognized.
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

    /// All built-in theme names in display order: dark themes first, then light.
    /// Used by the theme-picker UI and OSC11 auto-detection switch.
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
