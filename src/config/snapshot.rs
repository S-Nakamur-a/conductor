//! Appearance snapshot and restart-required change detection.
//!
//! [`AppearanceSnapshot`] captures the subset of [`Config`](super::Config)
//! fields that can be live-reloaded without restarting Conductor.
//! [`has_restart_changes`] identifies everything else.

use super::{Config, DiffView};

/// A point-in-time capture of all "live-reloadable" (appearance) fields.
///
/// Equality is used as an idempotency guard in `App::reload_appearance_config`:
/// when the snapshot matches the running state, no work is done, which naturally
/// absorbs the self-write loop from the in-app theme picker.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceSnapshot {
    pub ui_theme: Option<String>,
    pub ui_high_contrast: bool,
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
    pub layout_explorer_split_pct: u16,
}

impl Config {
    /// Capture a snapshot of the appearance (live-reloadable) fields.
    pub fn appearance_snapshot(&self) -> AppearanceSnapshot {
        AppearanceSnapshot {
            ui_theme: self.ui.theme.clone(),
            ui_high_contrast: self.ui.high_contrast,
            viewer_theme: self.viewer.theme.clone(),
            viewer_syntax_theme_file: self.viewer.syntax_theme_file.clone(),
            viewer_tab_width: self.viewer.tab_width,
            diff_word_diff: self.diff.word_diff,
            diff_default_view: self.diff.default_view,
            general_decoration: self.general.decoration.clone(),
            layout_explorer_width_pct: self.layout.explorer_width_pct,
            layout_viewer_width_pct: self.layout.viewer_width_pct,
            layout_terminal_split_pct: self.layout.terminal_split_pct,
            layout_explorer_split_pct: self.layout.explorer_split_pct,
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
        self.ui.high_contrast = new.ui.high_contrast;
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
