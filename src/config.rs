//! Configuration loading and persistence.
//!
//! Reads a TOML configuration file from `~/.config/conductor/config.toml` and
//! exposes strongly-typed settings for the rest of the application.
//!
//! Every field carries a serde default so the config file can be empty or
//! partially specified.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

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
    /// `[keybinds]` -- optional user key-bind overrides.
    pub keybinds: KeybindsConfig,
    /// `[notification]` -- OS notification settings.
    pub notification: NotificationConfig,
    /// `[ccusage]` -- Claude Code token usage display.
    pub ccusage: CcusageConfig,
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
            Ok(Config::default())
        }
    }

    /// Expand tilde (`~`) prefixes in path-valued fields.
    fn expand_paths(&mut self) {
        if let Some(ref repo) = self.general.repo {
            self.general.repo = Some(expand_tilde(repo));
        }
        self.general.repos = self
            .general
            .repos
            .iter()
            .map(|p| expand_tilde(p))
            .collect();
        if let Some(ref wt_dir) = self.general.worktree_dir {
            self.general.worktree_dir = Some(expand_tilde(wt_dir));
        }
        if let Some(ref path) = self.viewer.syntax_theme_file {
            let expanded = expand_tilde(&PathBuf::from(path));
            self.viewer.syntax_theme_file = Some(expanded.to_string_lossy().into_owned());
        }
    }
}

// ---------------------------------------------------------------------------
// Section structs
// ---------------------------------------------------------------------------

/// `[general]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Path to the default repository to open on startup.
    pub repo: Option<PathBuf>,
    /// Name of the main/trunk branch (e.g. `"main"` or `"master"`).
    pub main_branch: String,
    /// Shell executable used for PTY sessions.
    pub shell: String,
    /// List of additional repository paths for multi-repo support.
    pub repos: Vec<PathBuf>,
    /// Custom base directory for worktrees.
    /// When `None`, defaults to `<repo-parent>/<repo-name>-worktrees/`.
    pub worktree_dir: Option<PathBuf>,
    /// Decoration mode for the worktree panel:
    /// "aquarium" (default), "space", "garden", "city", "none".
    pub decoration: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            repo: None,
            main_branch: String::from("main"),
            shell: default_shell(),
            repos: Vec::new(),
            worktree_dir: None,
            decoration: String::from("aquarium"),
        }
    }
}

/// `[terminal]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Scrollback lines kept for inactive (background) sessions.
    pub inactive_scrollback: usize,
    /// Scrollback lines kept for the active (foreground) session.
    pub active_scrollback: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            inactive_scrollback: 1000,
            active_scrollback: 10000,
        }
    }
}

/// `[viewer]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerConfig {
    /// Syntax-highlighting theme name.
    pub theme: String,
    /// Path to a custom `.tmTheme` file for syntax highlighting.
    pub syntax_theme_file: Option<String>,
    /// Number of spaces per tab stop.
    pub tab_width: usize,
    /// Whether to soft-wrap long lines.
    pub word_wrap: bool,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            theme: String::from("catppuccin-mocha"),
            syntax_theme_file: None,
            tab_width: 2,
            word_wrap: false,
        }
    }
}

/// `[diff]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffConfig {
    /// Whether to show a unified or side-by-side diff.
    pub default_view: DiffView,
    /// Whether to highlight intra-line word changes.
    pub word_diff: bool,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            default_view: DiffView::Unified,
            word_diff: true,
        }
    }
}

/// Supported diff presentation styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffView {
    Unified,
    SideBySide,
}

/// `[review]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    /// Template string used to format review prompts.
    ///
    /// The placeholder `{comments}` is replaced with the actual review
    /// comments at runtime.
    pub prompt_template: String,
    /// What to do with the rendered prompt.
    pub prompt_action: PromptAction,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            prompt_template: default_prompt_template(),
            prompt_action: PromptAction::Clipboard,
        }
    }
}

/// Action taken with a rendered review prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAction {
    Clipboard,
    SendToSession,
}

/// `[keybinds]` section.
///
/// Stores arbitrary `key = "action"` pairs that can override the default
/// key bindings at runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindsConfig {
    /// Map from key chord string to action name.
    #[serde(flatten)]
    pub overrides: HashMap<String, String>,
}

/// `[notification]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    /// Send OS notification when Claude Code is waiting for input.
    pub cc_waiting: bool,
}

/// `[ccusage]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CcusageConfig {
    /// Enable Claude Code token usage display in the title bar.
    pub enabled: bool,
    /// Polling interval in seconds.
    pub poll_interval_secs: u64,
}

impl Default for CcusageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval_secs: 120,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the canonical path to the configuration file.
fn config_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("conductor")
        .join("config.toml")
}

/// Detect the user's shell from `$SHELL`, falling back to `/bin/sh`.
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| String::from("/bin/sh"))
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            return PathBuf::from(s.replacen('~', &home.to_string_lossy(), 1));
        }
    }
    path.to_path_buf()
}

/// Default review prompt template (Japanese).
fn default_prompt_template() -> String {
    String::from(
        "\
以下のレビューコメントに対応してください。

{comments}",
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips_through_toml() {
        let cfg = Config::default();
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let cfg2: Config = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(cfg2.general.main_branch, "main");
        assert_eq!(cfg2.general.decoration, "aquarium");
        assert_eq!(cfg2.terminal.inactive_scrollback, 1000);
        assert_eq!(cfg2.terminal.active_scrollback, 10000);
        assert_eq!(cfg2.viewer.theme, "catppuccin-mocha");
        assert_eq!(cfg2.viewer.tab_width, 2);
        assert!(!cfg2.viewer.word_wrap);
        assert_eq!(cfg2.diff.default_view, DiffView::Unified);
        assert!(cfg2.diff.word_diff);
        assert_eq!(cfg2.review.prompt_action, PromptAction::Clipboard);
        assert!(!cfg2.notification.cc_waiting);
        assert!(!cfg2.ccusage.enabled);
        assert_eq!(cfg2.ccusage.poll_interval_secs, 120);
    }

    #[test]
    fn empty_toml_gives_defaults() {
        let cfg: Config = toml::from_str("").expect("empty toml");
        assert_eq!(cfg.general.main_branch, "main");
        assert_eq!(cfg.diff.default_view, DiffView::Unified);
    }

    #[test]
    fn diff_view_serde() {
        let cfg: DiffConfig =
            toml::from_str(r#"default_view = "side-by-side""#).expect("parse");
        assert_eq!(cfg.default_view, DiffView::SideBySide);
    }

    #[test]
    fn prompt_action_serde() {
        let cfg: ReviewConfig =
            toml::from_str(r#"prompt_action = "send_to_session""#).expect("parse");
        assert_eq!(cfg.prompt_action, PromptAction::SendToSession);
    }

    #[test]
    fn tilde_expansion() {
        let p = PathBuf::from("~/dev/project");
        let expanded = expand_tilde(&p);
        assert!(!expanded.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn ccusage_config_parse() {
        let cfg: CcusageConfig =
            toml::from_str(r#"enabled = true
poll_interval_secs = 60"#)
                .expect("parse");
        assert!(cfg.enabled);
        assert_eq!(cfg.poll_interval_secs, 60);
    }

    #[test]
    fn keybinds_parse() {
        let toml_str = r#"
quit = "q"
refresh = "r"
"#;
        let kb: KeybindsConfig = toml::from_str(toml_str).expect("parse keybinds");
        assert_eq!(kb.overrides.get("quit").unwrap(), "q");
        assert_eq!(kb.overrides.get("refresh").unwrap(), "r");
    }
}
