//! Configuration loading and persistence.
//!
//! Reads a TOML configuration file from `~/.config/conductor/config.toml` and
//! exposes strongly-typed settings for the rest of the application.
//!
//! Every field carries a serde default so the config file can be empty or
//! partially specified.

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
            if let Err(e) = std::fs::write(&config_path, &default_content) {
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
            self.general.repo = Some(expand_tilde(repo));
        }
        self.general.repos = self.general.repos.iter().map(|p| expand_tilde(p)).collect();
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
    /// Which LLM provider to use for smart worktree generation. Each provider stands
    /// alone; a failure surfaces to the user rather than falling back to another.
    /// `"gemini"` (default): the Gemini HTTP API.
    /// `"claude"`: the `claude -p` CLI.
    /// `"command"`: a user-supplied external command (see `command`).
    pub provider: String,
    /// External command to run when `provider = "command"`.
    ///
    /// argv form (`["ollama", "run", "llama3"]`); run directly without a shell.
    /// Conductor writes the prompt to the command's stdin and reads the completion
    /// from stdout — see the "External LLM Command Protocol" in `ai_caller.rs`.
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
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            explorer_width_pct: 24,
            viewer_width_pct: 38,
            terminal_split_pct: 80,
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
}

// ---------------------------------------------------------------------------
// Snapshot types
// ---------------------------------------------------------------------------

/// A point-in-time capture of all "live-reloadable" (appearance) fields.
///
/// Equality is used as an idempotency guard in `App::reload_appearance_config`:
/// when the snapshot matches the running state, no work is done, which naturally
/// absorbs the self-write loop from the in-app theme picker.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceSnapshot {
    pub ui_theme: Option<String>,
    pub viewer_theme: String,
    pub viewer_syntax_theme_file: Option<String>,
    pub viewer_tab_width: usize,
    // viewer.word_wrap is intentionally absent — the rendering path is not yet
    // implemented, so saving word_wrap should not trigger a "Config reloaded"
    // flash or any visual change. Re-add here when the render path is wired.
    pub diff_word_diff: bool,
    pub diff_default_view: DiffView,
    pub general_decoration: String,
    pub layout_explorer_width_pct: u16,
    pub layout_viewer_width_pct: u16,
    pub layout_terminal_split_pct: u16,
}

impl Config {
    /// Capture a snapshot of the appearance (live-reloadable) fields.
    pub fn appearance_snapshot(&self) -> AppearanceSnapshot {
        AppearanceSnapshot {
            ui_theme: self.ui.theme.clone(),
            viewer_theme: self.viewer.theme.clone(),
            viewer_syntax_theme_file: self.viewer.syntax_theme_file.clone(),
            viewer_tab_width: self.viewer.tab_width,
            diff_word_diff: self.diff.word_diff,
            diff_default_view: self.diff.default_view,
            general_decoration: self.general.decoration.clone(),
            layout_explorer_width_pct: self.layout.explorer_width_pct,
            layout_viewer_width_pct: self.layout.viewer_width_pct,
            layout_terminal_split_pct: self.layout.terminal_split_pct,
        }
    }

    /// Copy all live-reloadable appearance fields from `new` into `self`.
    ///
    /// Only the fields tracked by [`AppearanceSnapshot`] (plus `viewer.word_wrap`
    /// which is tracked in config but not yet in the snapshot) are updated;
    /// restart-required fields (shell, scrollback, API settings, keybinds, etc.)
    /// are intentionally left untouched. Called by `App::apply_appearance` before
    /// rebuilding derived state (syntect theme, diff, layout cache, etc.).
    pub fn adopt_appearance(&mut self, new: &Config) {
        self.ui.theme = new.ui.theme.clone();
        self.viewer.theme = new.viewer.theme.clone();
        self.viewer.syntax_theme_file = new.viewer.syntax_theme_file.clone();
        self.viewer.tab_width = new.viewer.tab_width;
        // word_wrap: copy into config so it persists, but not in AppearanceSnapshot
        // because the rendering path is not yet implemented.
        self.viewer.word_wrap = new.viewer.word_wrap;
        self.diff.word_diff = new.diff.word_diff;
        self.diff.default_view = new.diff.default_view;
        self.general.decoration = new.general.decoration.clone();
        self.layout = new.layout.clone();
    }
}

/// Return `true` when `new` differs from `old` in any restart-required field.
///
/// Restart-required fields are those NOT covered by `AppearanceSnapshot`:
/// `general.{repo, repos, worktree_dir, shell, main_branch, auto_resume,
/// auto_resume_main}`, `terminal.{active_scrollback, inactive_scrollback}`,
/// `rich.mode`, `api.*`, `updates.*`, `ccusage.*`, `review.*`, `keybinds`.
pub fn has_restart_changes(old: &Config, new: &Config) -> bool {
    old.general.shell != new.general.shell
        || old.general.repo != new.general.repo
        || old.general.repos != new.general.repos
        || old.general.worktree_dir != new.general.worktree_dir
        || old.general.main_branch != new.general.main_branch
        || old.general.auto_resume != new.general.auto_resume
        || old.general.auto_resume_main != new.general.auto_resume_main
        || old.terminal.inactive_scrollback != new.terminal.inactive_scrollback
        || old.terminal.active_scrollback != new.terminal.active_scrollback
        || old.rich.mode != new.rich.mode
        || old.api.model != new.api.model
        || old.api.provider != new.api.provider
        || old.api.command != new.api.command
        || old.api.command_timeout_secs != new.api.command_timeout_secs
        || old.updates.check_on_startup != new.updates.check_on_startup
        || old.updates.check_interval_secs != new.updates.check_interval_secs
        || old.ccusage.enabled != new.ccusage.enabled
        || old.ccusage.poll_interval_secs != new.ccusage.poll_interval_secs
        || old.review.prompt_template != new.review.prompt_template
        || old.review.prompt_action != new.review.prompt_action
        || old.keybinds != new.keybinds
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the canonical path to the configuration file.
pub fn config_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("conductor")
        .join("config.toml")
}

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

/// Detect the user's shell from `$SHELL`, falling back to `/bin/sh`.
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| String::from("/bin/sh"))
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with('~')
        && let Some(home) = dirs::home_dir()
    {
        return PathBuf::from(s.replacen('~', &home.to_string_lossy(), 1));
    }
    path.to_path_buf()
}

/// Generate a default configuration file with all settings commented out.
pub fn generate_default_config() -> String {
    String::from(
        r#"# Conductor configuration file
# All fields are optional with sensible defaults.
# Appearance settings (theme, viewer, diff, decoration, layout) take effect
# immediately on file save — no restart required. All other settings need a restart.

[general]
# repo = "/path/to/default/repo"       # default repository to open on startup
# main_branch = "main"                  # main/trunk branch name (default: "main")
# shell = "/bin/zsh"                    # shell for PTY sessions (default: $SHELL)
# repos = ["/path/to/repo1", "/path/to/repo2"]  # additional repos for multi-repo support
# worktree_dir = "~/worktrees"          # custom worktree base directory
#                                       #   (default: <repo-parent>/<repo-name>-worktrees/)
# decoration = "aquarium"               # worktree panel decoration
#                                       #   aquarium | space | garden | city | none
# auto_resume = true                    # automatically resume Claude Code sessions on startup
# auto_resume_main = false              # also resume on the main worktree (grabbed sessions
#                                       #   are always resumed regardless of this setting)

[terminal]
# inactive_scrollback = 1000            # scrollback lines for background sessions
# active_scrollback = 10000             # scrollback lines for foreground session

[viewer]
# theme = "catppuccin-mocha"            # syntax highlighting theme
#                                       #   catppuccin-mocha | dracula | nord | solarized-dark
# syntax_theme_file = "~/.config/conductor/custom.tmTheme"  # custom .tmTheme file path
# tab_width = 2                         # spaces per tab stop
# word_wrap = false                     # soft-wrap long lines (未実装)

[diff]
# default_view = "unified"              # unified | side-by-side
# word_diff = true                      # highlight intra-line word changes

[review]
# レビュー機能はMCPプラグイン (conductor plugin) に移行済みです。
# 以下の設定は互換性のため残されていますが、通常は変更不要です。
# prompt_template = "以下のレビューコメントに対応してください。\n\n{comments}"
#                                       # template for review prompts ({comments} is replaced)
# prompt_action = "clipboard"           # clipboard | send_to_session

[keybinds]
# Key-bind overrides, in key→action form. Each entry maps a key chord to an
# action name. Your bindings are MERGED OVER the built-in defaults per-chord: a
# chord you bind here overrides the default for that exact chord, and every
# default you do not touch keeps working. To fully remove a default key (not
# just shadow it), set the chord to `false` — a tombstone, e.g.
# `"ctrl+q" = false`.
#
# [keybinds.keys] is the global layer (active everywhere). Each
# [keybinds.layers.<context>] table is a per-panel layer. Context names:
# worktree, explorer, explorer_diff_list, explorer_comment_list, viewer,
# viewer_diff_mode, terminal, overlay.
#
# Key grammar: modifiers ctrl/alt/shift/super joined with '+', then the key.
# A single char is verbatim and case-sensitive (e.g. "G" is Shift+g). Back-tab
# is "shift+tab". Named keys: enter, esc, tab, space, up/down/left/right, home,
# end, pageup, pagedown, delete, f1..f24.
#
# [keybinds.keys]
# "ctrl+q" = "quit"
# "ctrl+r" = false          # remove a default binding entirely
#
# [keybinds.layers.worktree]
# "j" = "navigate_down"
# "down" = "navigate_down"
# "w" = "create_worktree"

[ccusage]
# enabled = false                       # token usage display in the title bar (requires ccusage)
# poll_interval_secs = 120              # polling interval in seconds

[updates]
# check_on_startup = true               # check for new versions on startup
# check_interval_secs = 3600            # minimum interval between checks (default: 1h)

[rich]
# mode = "auto"                         # rich mode (gradient borders, pixel-quality images)
#                                       #   auto  - detect terminal capabilities (default)
#                                       #   off   - plain rendering everywhere
#                                       #   force - enable even when detection fails

[api]
# provider = "gemini"                   # "gemini" (Gemini API), "claude" (claude -p CLI), or "command" (external) — no fallback between them
# model = "gemini-2.5-flash"            # model for smart worktree generation (Gemini API)
# command = ["ollama", "run", "llama3"] # external command for provider = "command" (prompt on stdin, completion on stdout)
# command_timeout_secs = 60             # wall-clock timeout for the command provider (0 = no timeout)

[ui]
# theme = "catppuccin-mocha"            # UI color theme (overrides [viewer] theme when set)
#                                       #   dark:  catppuccin-mocha | dracula | nord | solarized-dark
#                                       #          tokyo-night | gruvbox | rose-pine | kanagawa
#                                       #   light: catppuccin-latte | solarized-light | github-light
#                                       # Light themes work best on terminals with a light/white background.
#                                       # When unset, conductor auto-detects a light background via OSC 11
#                                       # and switches to catppuccin-latte for that session (no file write).

[layout]
# explorer_width_pct = 24               # explorer column width % (default: 24)
# viewer_width_pct = 38                 # viewer column width % (default: 38)
#                                       # terminal column gets the remaining width
# terminal_split_pct = 80              # Claude Code area height % within terminal column (default: 80)
#                                       # shell area receives the remainder. These three values are the
#                                       # initial proportions; resize panels live, tmux-style, with
#                                       # Ctrl+Alt+Arrow (grows the focused panel toward the arrow) and
#                                       # conductor writes the new ratios back here automatically.
#                                       # These proportions apply in the default layout only;
#                                       # maximizing a panel (Ctrl+Alt+Z) overrides them temporarily.
"#,
    )
}

/// Persist a theme selection to `~/.config/conductor/config.toml`.
///
/// Uses text-based minimal editing to preserve existing comments and structure:
/// - If the `[ui]` section already has an uncommented `theme = ...` line, it is
///   replaced in place.
/// - If the `[ui]` section exists but has no uncommented `theme` line (e.g. only
///   comments), the line is inserted immediately after the `[ui]` header.
/// - If no `[ui]` section exists, `\n[ui]\ntheme = "..."\n` is appended.
pub fn persist_ui_theme(name: &str) -> Result<()> {
    let path = config_file_path();
    let contents = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        // Config file doesn't exist yet; generate defaults and proceed.
        generate_default_config()
    };
    let updated = upsert_ui_theme(&contents, name);
    std::fs::write(&path, updated)?;
    Ok(())
}

/// Return `true` when `line` is a bare `[section]` TOML header, possibly
/// followed by whitespace and/or an inline comment (e.g. `[ui]  # colors`).
/// Sub-sections like `[ui.fonts]` are deliberately excluded.
fn is_section_header(line: &str, section: &str) -> bool {
    let trimmed = line.trim();
    let bracket = format!("[{section}]");
    if !trimmed.starts_with(&bracket) {
        return false;
    }
    // After `[section]` only whitespace and/or a `#` comment may follow.
    let after = trimmed[bracket.len()..].trim_start();
    after.is_empty() || after.starts_with('#')
}

/// Pure function: upsert `<key> = <value>` in the `[section]` table of a config
/// file string, preserving all comments and surrounding content.
///
/// - If the section has an uncommented `key = ...` line, it is replaced in place.
/// - If the section exists but has no uncommented `key` line (e.g. only the
///   commented default), the line is inserted right after the section header.
/// - If the section does not exist, `\n[section]\n<key> = <value>\n` is appended.
///
/// `value` must already be formatted as valid TOML (quote strings yourself).
/// Extracted as a testable helper so file I/O stays separable from the edit.
fn upsert_section_kv(contents: &str, section: &str, key: &str, value: &str) -> String {
    let kv_line = format!("{key} = {value}");
    let lines: Vec<&str> = contents.lines().collect();

    let sec_start = lines.iter().position(|l| is_section_header(l, section));

    if let Some(sec_idx) = sec_start {
        // End of section = next bare `[...]` header (not a comment).
        let sec_end = lines[sec_idx + 1..]
            .iter()
            .position(|l| {
                let t = l.trim();
                t.starts_with('[') && !t.starts_with('#')
            })
            .map(|i| sec_idx + 1 + i)
            .unwrap_or(lines.len());

        // Look for an existing uncommented `key = ...` line within the section.
        let key_eq = format!("{key} =");
        let key_eq_tight = format!("{key}=");
        let existing_idx = lines[sec_idx + 1..sec_end]
            .iter()
            .position(|l| {
                let t = l.trim();
                !t.starts_with('#') && (t.starts_with(&key_eq) || t.starts_with(&key_eq_tight))
            })
            .map(|i| sec_idx + 1 + i);

        let mut result_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        if let Some(idx) = existing_idx {
            result_lines[idx] = kv_line;
        } else {
            // No uncommented key line in the section — insert right after the header.
            result_lines.insert(sec_idx + 1, kv_line);
        }

        let mut result = result_lines.join("\n");
        if contents.ends_with('\n') {
            result.push('\n');
        }
        result
    } else {
        // No such section at all — append one.
        let mut result = contents.to_string();
        if !result.ends_with('\n') && !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("\n[{section}]\n{kv_line}\n"));
        result
    }
}

/// Pure function: upsert `theme = "<name>"` in the `[ui]` section. Thin wrapper
/// over [`upsert_section_kv`] kept for call-site clarity and its dedicated tests.
fn upsert_ui_theme(contents: &str, name: &str) -> String {
    upsert_section_kv(contents, "ui", "theme", &format!("\"{name}\""))
}

/// Persist the runtime panel proportions to the `[layout]` section of
/// `~/.config/conductor/config.toml`, preserving comments and structure.
///
/// Called after a tmux-style pane resize so the chosen ratios survive restarts.
/// The three values are the explorer/viewer column width percentages and the
/// Claude-area height percentage within the terminal column. Writing this file
/// trips the config watcher, but `reload_appearance_config` no-ops because the
/// running config already holds these values (the appearance snapshot matches).
pub fn persist_layout_proportions(
    explorer_width_pct: u16,
    viewer_width_pct: u16,
    terminal_split_pct: u16,
) -> Result<()> {
    let path = config_file_path();
    let contents = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        generate_default_config()
    };
    let updated = upsert_section_kv(
        &contents,
        "layout",
        "explorer_width_pct",
        &explorer_width_pct.to_string(),
    );
    let updated = upsert_section_kv(&updated, "layout", "viewer_width_pct", &viewer_width_pct.to_string());
    let updated = upsert_section_kv(
        &updated,
        "layout",
        "terminal_split_pct",
        &terminal_split_pct.to_string(),
    );
    std::fs::write(&path, updated)?;
    Ok(())
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
        assert!(!cfg2.ccusage.enabled);
        assert_eq!(cfg2.ccusage.poll_interval_secs, 120);
        assert!(cfg2.updates.check_on_startup);
        assert_eq!(cfg2.updates.check_interval_secs, 3600);
    }

    #[test]
    fn empty_toml_gives_defaults() {
        let cfg: Config = toml::from_str("").expect("empty toml");
        assert_eq!(cfg.general.main_branch, "main");
        assert_eq!(cfg.diff.default_view, DiffView::Unified);
    }

    #[test]
    fn diff_view_serde() {
        let cfg: DiffConfig = toml::from_str(r#"default_view = "side-by-side""#).expect("parse");
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
        let cfg: CcusageConfig = toml::from_str(
            r#"enabled = true
poll_interval_secs = 60"#,
        )
        .expect("parse");
        assert!(cfg.enabled);
        assert_eq!(cfg.poll_interval_secs, 60);
    }

    #[test]
    fn updates_config_parse() {
        let cfg: UpdatesConfig = toml::from_str(
            r#"check_on_startup = false
check_interval_secs = 3600"#,
        )
        .expect("parse");
        assert!(!cfg.check_on_startup);
        assert_eq!(cfg.check_interval_secs, 3600);
    }

    #[test]
    fn keybinds_parse() {
        // The [keybinds] section is captured as a raw table (key→action schema)
        // and handed to keymap::KeyMap, which owns parsing.
        let toml_str = r#"
[keybinds.keys]
"ctrl+q" = "quit"

[keybinds.layers.worktree]
"j" = "navigate_down"
"w" = "create_worktree"
"#;
        let cfg: Config = toml::from_str(toml_str).expect("parse config");
        let keys = cfg.keybinds.get("keys").and_then(|v| v.as_table()).unwrap();
        assert_eq!(keys.get("ctrl+q").and_then(|v| v.as_str()), Some("quit"));

        let worktree = cfg
            .keybinds
            .get("layers")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("worktree"))
            .and_then(|v| v.as_table())
            .unwrap();
        assert_eq!(
            worktree.get("j").and_then(|v| v.as_str()),
            Some("navigate_down")
        );
    }

    #[test]
    fn generated_default_config_is_valid_toml() {
        let content = generate_default_config();
        let cfg: Config = toml::from_str(&content).expect("generated config must be valid TOML");
        // All values should match defaults since everything is commented out.
        assert_eq!(cfg.general.main_branch, "main");
        assert_eq!(cfg.terminal.inactive_scrollback, 1000);
        assert_eq!(cfg.viewer.tab_width, 2);
        assert!(cfg.updates.check_on_startup);
    }

    #[test]
    fn ui_config_default_has_no_theme() {
        let cfg = Config::default();
        assert!(cfg.ui.theme.is_none());
    }

    #[test]
    fn ui_config_round_trips_through_toml() {
        let toml_str = r#"[ui]
theme = "catppuccin-latte"
"#;
        let cfg: Config = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.ui.theme.as_deref(), Some("catppuccin-latte"));

        // Serialize and deserialize again.
        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let cfg2: Config = toml::from_str(&serialized).expect("round-trip");
        assert_eq!(cfg2.ui.theme.as_deref(), Some("catppuccin-latte"));
    }

    #[test]
    fn upsert_ui_theme_appends_when_no_ui_section() {
        let contents = "[general]\nmain_branch = \"main\"\n";
        let result = upsert_ui_theme(contents, "nord");
        assert!(result.contains("[ui]"));
        assert!(result.contains("theme = \"nord\""));
        // Original content preserved.
        assert!(result.contains("[general]"));
    }

    #[test]
    fn upsert_section_kv_inserts_layout_value_over_commented_default() {
        // The generated config ships layout keys commented out; a resize must
        // insert a live value after the header while keeping the comment.
        let contents = "[layout]\n# explorer_width_pct = 24    # default\n";
        let result = upsert_section_kv(contents, "layout", "explorer_width_pct", "30");
        assert!(result.contains("explorer_width_pct = 30"));
        assert!(result.contains("# explorer_width_pct = 24"));
    }

    #[test]
    fn upsert_section_kv_replaces_existing_layout_value() {
        let contents = "[layout]\nexplorer_width_pct = 24\nviewer_width_pct = 38\n";
        let result = upsert_section_kv(contents, "layout", "viewer_width_pct", "42");
        assert_eq!(result, "[layout]\nexplorer_width_pct = 24\nviewer_width_pct = 42\n");
    }

    #[test]
    fn upsert_section_kv_chains_for_all_three_layout_keys() {
        // Mirrors persist_layout_proportions: three sequential upserts land in
        // the same [layout] table without clobbering each other.
        let contents = "[layout]\n# explorer_width_pct = 24\n# viewer_width_pct = 38\n# terminal_split_pct = 80\n\n[ui]\ntheme = \"nord\"\n";
        let r = upsert_section_kv(contents, "layout", "explorer_width_pct", "30");
        let r = upsert_section_kv(&r, "layout", "viewer_width_pct", "40");
        let r = upsert_section_kv(&r, "layout", "terminal_split_pct", "65");
        assert!(r.contains("explorer_width_pct = 30"));
        assert!(r.contains("viewer_width_pct = 40"));
        assert!(r.contains("terminal_split_pct = 65"));
        // Adjacent section is untouched.
        assert!(r.contains("[ui]"));
        assert!(r.contains("theme = \"nord\""));
        // Round-trips as valid TOML reflecting the new values.
        let cfg: Config = toml::from_str(&r).expect("layout edits stay valid TOML");
        assert_eq!(cfg.layout.explorer_width_pct, 30);
        assert_eq!(cfg.layout.viewer_width_pct, 40);
        assert_eq!(cfg.layout.terminal_split_pct, 65);
    }

    #[test]
    fn upsert_ui_theme_replaces_existing_theme_line() {
        let contents = "[ui]\ntheme = \"dracula\"\n";
        let result = upsert_ui_theme(contents, "github-light");
        assert_eq!(
            result,
            "[ui]\ntheme = \"github-light\"\n",
            "existing theme line must be replaced in place"
        );
    }

    #[test]
    fn upsert_ui_theme_inserts_after_ui_header_when_only_comments() {
        let contents = "[ui]\n# theme = \"catppuccin-mocha\"\n";
        let result = upsert_ui_theme(contents, "catppuccin-latte");
        // Should have the new line inserted after [ui], before the comment.
        assert!(result.contains("theme = \"catppuccin-latte\""));
        // Comment must be preserved.
        assert!(result.contains("# theme = \"catppuccin-mocha\""));
    }

    #[test]
    fn upsert_ui_theme_preserves_other_sections_after_ui() {
        let contents = "[viewer]\ntheme = \"dracula\"\n\n[ui]\n# theme placeholder\n\n[general]\n";
        let result = upsert_ui_theme(contents, "nord");
        assert!(result.contains("theme = \"nord\""));
        assert!(result.contains("[viewer]"));
        assert!(result.contains("[general]"));
    }

    #[test]
    fn upsert_ui_theme_trailing_newline_preserved() {
        let with_newline = "[ui]\ntheme = \"dracula\"\n";
        let without_newline = "[ui]\ntheme = \"dracula\"";
        assert!(upsert_ui_theme(with_newline, "nord").ends_with('\n'));
        assert!(!upsert_ui_theme(without_newline, "nord").ends_with('\n'));
    }

    // inline-comment [ui] header detection.

    #[test]
    fn upsert_ui_theme_handles_inline_comment_on_ui_header() {
        // `[ui]  # color settings` must be recognised as the [ui] section.
        let contents = "[general]\n\n[ui]  # color settings\ntheme = \"dracula\"\n";
        let result = upsert_ui_theme(contents, "nord");
        assert_eq!(
            result.matches("[ui]").count(),
            1,
            "must not append a duplicate [ui] section"
        );
        assert!(result.contains("theme = \"nord\""));
    }

    #[test]
    fn upsert_ui_theme_does_not_match_ui_subsection() {
        // `[ui.colors]` is NOT the `[ui]` section; a new `[ui]` block must be appended.
        let contents = "[ui.colors]\nfoo = \"bar\"\n";
        let result = upsert_ui_theme(contents, "nord");
        assert!(
            result.contains("[ui]\n"),
            "a new [ui] section should be appended, not matched"
        );
        // The subsection must still be present.
        assert!(result.contains("[ui.colors]"));
    }

    #[test]
    fn is_section_header_cases() {
        assert!(super::is_section_header("[ui]", "ui"));
        assert!(super::is_section_header("[ui]  ", "ui"));
        assert!(super::is_section_header("[ui]  # comment", "ui"));
        assert!(super::is_section_header("  [ui]", "ui"));
        assert!(!super::is_section_header("[ui.sub]", "ui"));
        assert!(!super::is_section_header("[ui.colors]", "ui"));
        assert!(!super::is_section_header("[viewer]", "ui"));
        // Generic over the section name.
        assert!(super::is_section_header("[layout]", "layout"));
        assert!(!super::is_section_header("[layout]", "ui"));
    }

    #[test]
    fn layout_config_defaults() {
        let cfg = LayoutConfig::default();
        assert_eq!(cfg.explorer_width_pct, 24);
        assert_eq!(cfg.viewer_width_pct, 38);
        assert_eq!(cfg.terminal_split_pct, 80);
    }

    #[test]
    fn layout_config_round_trips_through_toml() {
        let toml_str = r#"[layout]
explorer_width_pct = 30
viewer_width_pct = 40
terminal_split_pct = 75
"#;
        let cfg: Config = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.layout.explorer_width_pct, 30);
        assert_eq!(cfg.layout.viewer_width_pct, 40);
        assert_eq!(cfg.layout.terminal_split_pct, 75);

        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let cfg2: Config = toml::from_str(&serialized).expect("round-trip");
        assert_eq!(cfg2.layout.explorer_width_pct, 30);
        assert_eq!(cfg2.layout.viewer_width_pct, 40);
        assert_eq!(cfg2.layout.terminal_split_pct, 75);
    }

    #[test]
    fn layout_config_empty_toml_gives_defaults() {
        let cfg: Config = toml::from_str("").expect("empty toml");
        assert_eq!(cfg.layout.explorer_width_pct, 24);
        assert_eq!(cfg.layout.viewer_width_pct, 38);
        assert_eq!(cfg.layout.terminal_split_pct, 80);
    }

    #[test]
    fn appearance_snapshot_includes_layout() {
        let mut cfg = Config::default();
        cfg.layout.explorer_width_pct = 30;
        let snap = cfg.appearance_snapshot();
        assert_eq!(snap.layout_explorer_width_pct, 30);
        assert_eq!(snap.layout_viewer_width_pct, 38);
        assert_eq!(snap.layout_terminal_split_pct, 80);
    }

    // ── adopt_appearance / appearance_snapshot invariants / has_restart_changes ──

    /// AC4 往復不変条件: adopt_appearance 後に snapshot が new と一致すること。
    /// AppearanceSnapshot に足してadopt_appearance のコピーに足し忘れた場合を検出する。
    #[test]
    fn adopt_appearance_round_trip_invariant() {
        let mut cur = Config::default();
        let mut new = Config::default();
        // Change every live field to a non-default value.
        new.ui.theme = Some(String::from("dracula"));
        new.viewer.theme = String::from("dracula");
        new.viewer.syntax_theme_file = Some(String::from("/tmp/custom.tmTheme"));
        new.viewer.tab_width = 4;        // default is 2
        new.viewer.word_wrap = true;     // default is false
        new.diff.word_diff = false;      // default is true
        new.diff.default_view = DiffView::SideBySide; // default is Unified
        new.general.decoration = String::from("space");
        new.layout.explorer_width_pct = 30;
        new.layout.viewer_width_pct = 42;
        new.layout.terminal_split_pct = 70;

        cur.adopt_appearance(&new);

        assert_eq!(
            cur.appearance_snapshot(),
            new.appearance_snapshot(),
            "adopt_appearance must copy all snapshot-tracked live fields"
        );
    }

    /// snapshot 等価: 同一 config は等価。
    #[test]
    fn appearance_snapshot_equal_for_identical_configs() {
        let cfg = Config::default();
        assert_eq!(cfg.appearance_snapshot(), cfg.appearance_snapshot());
    }

    /// snapshot 不等価: 各 live フィールドを 1 つ変えると != になること。
    #[test]
    fn appearance_snapshot_detects_each_live_field_change() {
        let base = Config::default();

        let mut c = base.clone();
        c.ui.theme = Some(String::from("dracula"));
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "ui.theme");

        let mut c = base.clone();
        c.viewer.theme = String::from("nord");
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "viewer.theme");

        let mut c = base.clone();
        c.viewer.syntax_theme_file = Some(String::from("/custom.tmTheme"));
        assert_ne!(
            c.appearance_snapshot(),
            base.appearance_snapshot(),
            "viewer.syntax_theme_file"
        );

        let mut c = base.clone();
        c.viewer.tab_width = 4; // default is 2
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "viewer.tab_width");

        let mut c = base.clone();
        c.diff.word_diff = false; // default is true
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "diff.word_diff");

        let mut c = base.clone();
        c.diff.default_view = DiffView::SideBySide; // default is Unified
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "diff.default_view");

        let mut c = base.clone();
        c.general.decoration = String::from("space");
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "general.decoration");

        let mut c = base.clone();
        c.layout.explorer_width_pct = 30;
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "layout.explorer_width_pct");

        let mut c = base.clone();
        c.layout.viewer_width_pct = 42;
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "layout.viewer_width_pct");

        let mut c = base.clone();
        c.layout.terminal_split_pct = 70;
        assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "layout.terminal_split_pct");
    }

    /// has_restart_changes: live フィールドのみ変えたら false。
    #[test]
    fn has_restart_changes_false_for_live_only_diff() {
        let old = Config::default();
        let mut new = Config::default();
        new.ui.theme = Some(String::from("dracula"));
        new.viewer.theme = String::from("nord");
        new.viewer.tab_width = 4;        // default is 2
        new.diff.word_diff = false;      // default is true
        new.general.decoration = String::from("space");
        new.layout.explorer_width_pct = 30;
        assert!(!has_restart_changes(&old, &new));
    }

    /// has_restart_changes: 各 restart フィールドを 1 つ変えたら true。
    #[test]
    fn has_restart_changes_true_for_each_restart_field() {
        let base = Config::default();

        let mut c = base.clone();
        c.general.shell = String::from("/bin/fish");
        assert!(has_restart_changes(&base, &c), "general.shell");

        let mut c = base.clone();
        c.general.main_branch = String::from("master");
        assert!(has_restart_changes(&base, &c), "general.main_branch");

        let mut c = base.clone();
        c.general.repo = Some(PathBuf::from("/other/repo"));
        assert!(has_restart_changes(&base, &c), "general.repo");

        let mut c = base.clone();
        c.general.auto_resume = false; // default is true
        assert!(has_restart_changes(&base, &c), "general.auto_resume");

        let mut c = base.clone();
        c.general.auto_resume_main = true;
        assert!(has_restart_changes(&base, &c), "general.auto_resume_main");

        let mut c = base.clone();
        c.terminal.active_scrollback = 99999;
        assert!(has_restart_changes(&base, &c), "terminal.active_scrollback");

        let mut c = base.clone();
        c.api.provider = String::from("claude"); // default is "gemini"
        assert!(has_restart_changes(&base, &c), "api.provider");

        let mut c = base.clone();
        c.ccusage.enabled = true;
        assert!(has_restart_changes(&base, &c), "ccusage.enabled");
    }

    /// Partition test: すべてのフィールドが live か restart のどちらかに必ず属する。
    /// フィールドを 1 つ変えた new で snapshot != か has_restart_changes が必ず true になること。
    #[test]
    fn every_field_is_either_live_or_restart() {
        let base = Config::default();

        // general
        {
            let mut c = base.clone();
            c.general.repo = Some(PathBuf::from("/p"));
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.repo");
        }
        {
            let mut c = base.clone();
            c.general.main_branch = String::from("master");
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.main_branch");
        }
        {
            let mut c = base.clone();
            c.general.shell = String::from("/bin/fish");
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.shell");
        }
        {
            let mut c = base.clone();
            c.general.repos = vec![PathBuf::from("/p")];
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.repos");
        }
        {
            let mut c = base.clone();
            c.general.worktree_dir = Some(PathBuf::from("/wt"));
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.worktree_dir");
        }
        {
            let mut c = base.clone();
            c.general.decoration = String::from("space");
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.decoration");
        }
        {
            let mut c = base.clone();
            c.general.auto_resume = false; // default is true
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.auto_resume");
        }
        {
            let mut c = base.clone();
            c.general.auto_resume_main = true;
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.auto_resume_main");
        }
        // terminal
        {
            let mut c = base.clone();
            c.terminal.active_scrollback = 9999;
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "terminal.active_scrollback");
        }
        // viewer (live)
        {
            let mut c = base.clone();
            c.viewer.theme = String::from("nord");
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "viewer.theme");
        }
        {
            let mut c = base.clone();
            c.viewer.tab_width = 4; // default is 2
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "viewer.tab_width");
        }
        // diff (live)
        {
            let mut c = base.clone();
            c.diff.word_diff = false; // default is true
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "diff.word_diff");
        }
        // api
        {
            let mut c = base.clone();
            c.api.provider = String::from("claude"); // default is "gemini"
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "api.provider");
        }
        // ccusage
        {
            let mut c = base.clone();
            c.ccusage.enabled = true;
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "ccusage.enabled");
        }
        // layout (live)
        {
            let mut c = base.clone();
            c.layout.explorer_width_pct = 30;
            assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "layout.explorer_width_pct");
        }
    }

    /// Helper: estimate background luminance of a syntect theme (0–255 range).
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

        // Light UI themes must map to light syntect built-ins.
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
        let _ = syntect_theme_for(&viewer, &ts); // must not panic
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
        let _ = syntect_theme_for(&viewer, &ts); // must not panic
    }
}
