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

/// Pure function: upsert `theme = "<name>"` in the `[ui]` section of a
/// config file string, preserving all comments and surrounding content.
///
/// Extracted as a testable helper so file I/O in `persist_ui_theme` can be
/// tested independently. Called transitively via `set_theme`.
/// Return `true` when `line` is a `[ui]` TOML section header, possibly followed
/// by whitespace and/or an inline comment (e.g. `[ui]  # colors`).
/// Sub-sections like `[ui.fonts]` are deliberately excluded.
fn is_ui_section_header(line: &str) -> bool {
    let trimmed = line.trim();
    // Must start with exactly `[ui]` (not `[ui.something]`).
    if !trimmed.starts_with("[ui]") {
        return false;
    }
    // After `[ui]` only whitespace and/or a `#` comment may follow.
    let after = trimmed["[ui]".len()..].trim_start();
    after.is_empty() || after.starts_with('#')
}

fn upsert_ui_theme(contents: &str, name: &str) -> String {
    let theme_line = format!("theme = \"{name}\"");
    let lines: Vec<&str> = contents.lines().collect();

    // Locate the [ui] section header.
    // Match `[ui]` with optional trailing whitespace/comment, e.g. `[ui]  # colors`,
    // but NOT sub-sections like `[ui.colors]`.
    let ui_start = lines.iter().position(|l| is_ui_section_header(l));

    if let Some(ui_idx) = ui_start {
        // End of section = next bare `[...]` header (not a comment).
        let ui_end = lines[ui_idx + 1..]
            .iter()
            .position(|l| {
                let t = l.trim();
                t.starts_with('[') && !t.starts_with('#')
            })
            .map(|i| ui_idx + 1 + i)
            .unwrap_or(lines.len());

        // Look for an existing uncommented `theme = ...` key within the section.
        let existing_theme_idx = lines[ui_idx + 1..ui_end]
            .iter()
            .position(|l| {
                let t = l.trim();
                !t.starts_with('#') && (t.starts_with("theme =") || t.starts_with("theme="))
            })
            .map(|i| ui_idx + 1 + i);

        let mut result_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        if let Some(idx) = existing_theme_idx {
            result_lines[idx] = theme_line;
        } else {
            // No uncommented theme line in the section — insert right after [ui].
            result_lines.insert(ui_idx + 1, theme_line);
        }

        let mut result = result_lines.join("\n");
        if contents.ends_with('\n') {
            result.push('\n');
        }
        result
    } else {
        // No [ui] section at all — append one.
        let mut result = contents.to_string();
        if !result.ends_with('\n') && !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("\n[ui]\n{theme_line}\n"));
        result
    }
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
    fn is_ui_section_header_cases() {
        assert!(super::is_ui_section_header("[ui]"));
        assert!(super::is_ui_section_header("[ui]  "));
        assert!(super::is_ui_section_header("[ui]  # comment"));
        assert!(super::is_ui_section_header("  [ui]"));
        assert!(!super::is_ui_section_header("[ui.sub]"));
        assert!(!super::is_ui_section_header("[ui.colors]"));
        assert!(!super::is_ui_section_header("[viewer]"));
    }
}
