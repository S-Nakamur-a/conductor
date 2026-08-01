//! `config.toml` section structs.
//!
//! One struct per `[section]` in `config.toml`, each with its own `Default`
//! impl. Every field carries a serde default so the config file can be empty
//! or partially specified.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    /// Automatically resume Claude Code sessions from the previous run on startup.
    pub auto_resume: bool,
    /// Also auto-resume the session on the main worktree (only meaningful when
    /// `auto_resume` is `true`).  Defaults to `false` because sessions accumulate
    /// on the long-lived main worktree and reopening the latest one every launch
    /// is usually not desired.  Grabbed sessions are always resumed regardless of
    /// this setting.
    pub auto_resume_main: bool,
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
            auto_resume: true,
            auto_resume_main: false,
        }
    }
}

/// Detect the user's shell from `$SHELL`, falling back to `/bin/sh`.
pub(super) fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| String::from("/bin/sh"))
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    /// Natural language the walkthrough should be written in (e.g. "日本語",
    /// "English"). `None` leaves the choice to the model.
    pub walkthrough_language: Option<String>,
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

/// `[updates]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatesConfig {
    /// Check for new versions on startup.
    pub check_on_startup: bool,
    /// Minimum interval (seconds) between update checks (cache TTL).
    pub check_interval_secs: u64,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            check_interval_secs: 3600, // 1 hour
        }
    }
}

/// `[api]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Model ID for the Gemini API.
    pub model: String,
    /// Which LLM provider to use. Each provider stands alone; a failure surfaces to
    /// the user rather than falling back to another.
    /// `"gemini"` (default): the Gemini HTTP API.
    /// `"command"`: a user-supplied external command (see `command`).
    ///
    /// There is no built-in provider that runs the `claude` CLI: Conductor never
    /// spawns it. A Claude-backed setup is just `provider = "command"` with
    /// `command = ["claude", "-p", "{prompt}"]` — no wrapper script.
    pub provider: String,
    /// The AI tool to run when `provider = "command"`, in argv form, run
    /// directly without a shell.
    ///
    /// `{prompt}` and `{workdir}` are substituted into any argument
    /// (`["claude", "-p", "{prompt}"]`); with no `{prompt}` the
    /// prompt goes to stdin instead (`["ollama", "run", "llama3"]`). The
    /// completion is read from stdout. See the "External LLM Command Protocol"
    /// in `ai_caller.rs` — and note that per-task behaviour (tool use, output
    /// format) belongs to the feature's prompt, not to this command.
    pub command: Vec<String>,
    /// Wall-clock timeout (seconds) for the `command` provider. `0` disables it.
    pub command_timeout_secs: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            model: String::from("gemini-2.5-flash"),
            provider: String::from("gemini"),
            command: Vec::new(),
            command_timeout_secs: 60,
        }
    }
}

/// `[rich]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RichConfig {
    /// Rich mode activation: `"auto"` (detect terminal capabilities),
    /// `"off"` (never), or `"force"` (enable Tier A even without truecolor).
    pub mode: String,
}

impl Default for RichConfig {
    fn default() -> Self {
        Self {
            mode: String::from("auto"),
        }
    }
}

/// `[layout]` section — panel proportion overrides.
///
/// Values are percentages (0–100). The worktree column is always 0-width
/// (the worktree monitor lives in the top strip); the terminal column gets
/// whatever width remains after explorer and viewer. These proportions apply
/// only in the default (non-maximized) layout; maximizing a panel overrides
/// them as before.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    /// Explorer column width as a percentage of the total frame width.
    pub explorer_width_pct: u16,
    /// Viewer column width as a percentage of the total frame width.
    pub viewer_width_pct: u16,
    /// Claude Code area height as a percentage of the terminal column height.
    /// The shell area receives the remainder.
    pub terminal_split_pct: u16,
    /// File-tree height as a percentage of the Explorer column height; the
    /// changed-files list below receives the remainder.
    pub explorer_split_pct: u16,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            explorer_width_pct: 24,
            viewer_width_pct: 38,
            terminal_split_pct: 80,
            explorer_split_pct: 50,
        }
    }
}

/// `[ui]` section — UI appearance overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// UI color theme name. When `None`, falls back to `[viewer] theme` for
    /// backward compatibility. Light theme options: `catppuccin-latte`,
    /// `solarized-light`, `github-light`. Dark: see `Theme::all_names()`.
    pub theme: Option<String>,
    /// Apply a high-contrast transform to the active theme: brighten (dark
    /// themes) or deepen (light themes) the dim greys, body text, and accents
    /// for stronger legibility. Works with every theme, built-in or custom.
    pub high_contrast: bool,
}
