//! Configuration loading and persistence.
//!
//! Reads a TOML configuration file from `~/.config/conductor/config.toml` and
//! exposes strongly-typed settings for the rest of the application.
//!
//! Every field carries a serde default so the config file can be empty or
//! partially specified.

mod persist;
mod sections;
mod snapshot;
mod syntax_theme;

#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_persist;
#[cfg(test)]
mod tests_snapshot;
#[cfg(test)]
mod tests_syntax_theme;

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use persist::{
    config_file_path, generate_default_config, persist_layout_proportions,
    persist_ui_high_contrast, persist_ui_theme,
};
// PromptAction and AppearanceSnapshot are not referenced by name anywhere else
// in the crate yet (call sites use type inference / field access instead),
// but they are part of this module's public surface, so keep re-exporting
// them under `crate::config::*` rather than letting the split hide them.
#[allow(unused_imports)]
pub use sections::{
    ApiConfig, CcusageConfig, DiffConfig, DiffView, GeneralConfig, LayoutConfig, PromptAction,
    ReviewConfig, RichConfig, TerminalConfig, UiConfig, UpdatesConfig, ViewerConfig,
};
#[allow(unused_imports)]
pub use snapshot::{AppearanceSnapshot, has_restart_changes};
pub use syntax_theme::syntect_theme_for;

use persist::write_atomic;

// ---------------------------------------------------------------------------
// Top-level Config
// ---------------------------------------------------------------------------

/// Application-level configuration.
///
/// Mirrors the `[section]` layout of `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    /// `[general]` -- repository path, main branch, shell.
    pub general: GeneralConfig,
    /// `[terminal]` -- scrollback limits.
    pub terminal: TerminalConfig,
    /// `[viewer]` -- syntax theme, tab width, word wrap.
    pub viewer: ViewerConfig,
    /// `[diff]` -- diff presentation options.
    pub diff: DiffConfig,
    /// `[review]` -- code-review prompt settings.
    pub review: ReviewConfig,
    /// `[keybinds]` -- optional user key-bind overrides, in keymap-config's
    /// key→action TOML schema (`[keybinds.keys]`, `[keybinds.layers.<name>]`).
    /// Kept as a raw table and handed to `keymap::KeyMap`, which owns the schema.
    pub keybinds: toml::Table,
    /// `[ccusage]` -- Claude Code token usage display.
    pub ccusage: CcusageConfig,
    /// `[updates]` -- startup version check settings.
    pub updates: UpdatesConfig,
    /// `[api]` -- Gemini API settings.
    pub api: ApiConfig,
    /// `[rich]` -- rich mode (terminal graphics) settings.
    pub rich: RichConfig,
    /// `[ui]` -- UI appearance settings (theme, etc.).
    #[serde(default)]
    pub ui: UiConfig,
    /// `[layout]` -- panel proportion overrides.
    #[serde(default)]
    pub layout: LayoutConfig,
}

impl Config {
    /// Load configuration from `~/.config/conductor/config.toml`.
    ///
    /// Falls back to `Config::default()` when the file does not exist.
    pub fn load() -> Result<Self> {
        let config_path = config_file_path();

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            let mut config: Config = toml::from_str(&contents)?;
            config.expand_paths();
            Ok(config)
        } else {
            // Generate a default config file with comments.
            if let Some(parent) = config_path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                log::warn!("failed to create config directory: {e}");
            }
            let default_content = generate_default_config();
            if let Err(e) = write_atomic(&config_path, &default_content) {
                log::warn!("failed to write default config: {e}");
            } else {
                log::info!("generated default config at {}", config_path.display());
            }
            Ok(Config::default())
        }
    }

    /// Expand tilde (`~`) prefixes in path-valued fields.
    fn expand_paths(&mut self) {
        if let Some(ref repo) = self.general.repo {
            self.general.repo = Some(persist::expand_tilde(repo));
        }
        self.general.repos = self
            .general
            .repos
            .iter()
            .map(|p| persist::expand_tilde(p))
            .collect();
        if let Some(ref wt_dir) = self.general.worktree_dir {
            self.general.worktree_dir = Some(persist::expand_tilde(wt_dir));
        }
        if let Some(ref path) = self.viewer.syntax_theme_file {
            let expanded = persist::expand_tilde(&PathBuf::from(path));
            self.viewer.syntax_theme_file = Some(expanded.to_string_lossy().into_owned());
        }
    }
}
