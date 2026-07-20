//! Runtime theme switching and live config-file reload for appearance
//! settings (theme, syntax highlighting, diff view mode, layout ratios).

use syntect::highlighting::ThemeSet;

use super::App;
use crate::config;

impl App {
    /// Switch the active UI theme at runtime.
    ///
    /// When `persist` is `true`, the selection is written to the config file
    /// (`~/.config/conductor/config.toml`) so it survives restarts. A write
    /// failure is non-fatal: it is logged and surfaced as a warning flash.
    pub fn set_theme(&mut self, name: &str, persist: bool) {
        self.theme = super::build_theme(name, self.high_contrast);
        self.theme_name = name.to_string();
        self.config.ui.theme = Some(name.to_string());
        if persist
            && let Err(e) = crate::config::persist_ui_theme(name)
        {
            log::warn!("failed to persist theme '{name}': {e}");
            self.set_status(
                format!("Theme saved in session but could not write config: {e}"),
                super::StatusLevel::Warning,
            );
        }
    }

    // ── Live config reload ─────────────────────────────────────────

    /// Apply appearance (live-reloadable) fields from `new` to the running app.
    ///
    /// Only the fields classified as LIVE are copied; restart-required fields
    /// (shell, scrollback limits, API settings, etc.) are intentionally left
    /// untouched so that `refresh_diff`, which reads `config.general.main_branch`
    /// on every call, never sees a stale or transitional value.
    ///
    /// ## LIVE fields applied here
    /// - `ui.theme` / `viewer.theme` → theme + theme_name + syntect rebuild
    /// - `viewer.syntax_theme_file`  → syntect rebuild (same path as theme)
    /// - `viewer.tab_width`          → config copy + refresh_viewer + refresh_diff
    /// - `diff.word_diff`            → config copy + refresh_diff
    /// - `diff.default_view`         → diff_state.view_mode + refresh_diff
    /// - `general.decoration`        → config copy (drawn directly each frame)
    /// - `layout.*`                  → config copy; LayoutCache auto-invalidates
    ///
    /// `viewer.word_wrap` is copied into config via `adopt_appearance` but is not
    /// in `AppearanceSnapshot` and has no rendering effect until the render path
    /// is implemented.
    pub fn apply_appearance(&mut self, new: &config::Config) {
        // ── UI / syntax theme ──────────────────────────────────────
        let new_theme_name = super::resolve_theme_name(new);
        let new_high_contrast = new.ui.high_contrast;
        if new_theme_name != self.theme_name || new_high_contrast != self.high_contrast {
            self.theme = super::build_theme(&new_theme_name, new_high_contrast);
            self.theme_name = new_theme_name;
            self.high_contrast = new_high_contrast;
        }

        // Rebuild syntect theme when either the viewer theme or the custom
        // theme file changes (the two are bundled into a single re-construction
        // so there is never a half-updated state).
        let ts = ThemeSet::load_defaults();
        self.syntect_theme = config::syntect_theme_for(&new.viewer, &ts);

        // Clear the Markdown cache so code blocks inside review comments pick
        // up the new syntect theme. The cache fingerprints the UI colour palette
        // only; a syntax-only change would otherwise leave stale highlighted spans.
        self.markdown_cache.clear();

        // Force a full rebuild of the reflow transcript on the next render so
        // that Markdown spans pick up the new theme colours and syntect palette.
        // Setting last_width=0 makes build_lines run on the next frame regardless
        // of whether the panel width changed.
        self.reflow.last_width = 0;
        self.reflow.cache.clear();

        // ── Diff view mode ──────────────────────────────────────────
        // Apply view_mode directly. `diff_state.view_mode` is written only in
        // `DiffState::new` and here — there is no runtime interactive toggle —
        // so overwriting it is safe.
        self.diff_state.view_mode = crate::diff_state::DiffViewMode::from(new.diff.default_view);

        // Copy all live config fields (no-op for restart-required fields).
        // LayoutCache keyed on layout proportions detects changes automatically
        // and recomputes on the next frame; no explicit invalidation needed.
        self.config.adopt_appearance(new);

        // The Claude/Shell split is a runtime field seeded from config; resync it
        // so an external edit to layout.terminal_split_pct takes effect live. Our
        // own resize-driven writes never reach here — they leave the appearance
        // snapshot unchanged, so reload_appearance_config short-circuits first.
        self.terminal_split_pct = self
            .config
            .layout
            .terminal_split_pct
            .clamp(Self::TERMINAL_SPLIT_MIN, Self::TERMINAL_SPLIT_MAX);

        // Refresh the viewer file tree + diff to pick up tab_width / word_diff.
        // refresh_viewer calls rehighlight_viewer unconditionally, so the new
        // syntect theme is applied to the open file as part of this call.
        self.refresh_viewer();
        self.refresh_diff();

        // Trigger a full redraw.
        self.dirty.mark_all();
    }

    /// Reload the config file and apply any appearance changes.
    ///
    /// 1. Guards against the config file being absent (e.g., a remove event from
    ///    a delete-then-write atomic save): skips loading to avoid `Config::load()`
    ///    writing a default file and clobbering the user's in-progress edits.
    /// 2. Loads `~/.config/conductor/config.toml`; on parse error, flashes an
    ///    error message and returns without modifying the running config.
    /// 3. Computes whether appearance fields and/or restart-required fields changed.
    ///    True no-op (neither changed) → returns silently, which is also the guard
    ///    that absorbs the self-write loop from the in-app theme picker.
    /// 4. If restart-required fields changed, flashes a warning.
    /// 5. If appearance fields changed, calls `apply_appearance` and (when no
    ///    restart warning was issued) flashes an info confirmation.
    pub fn reload_appearance_config(&mut self) {
        // Guard: skip if the file was just deleted (remove event from an atomic
        // editor save). Config::load() on a missing file would write defaults and
        // return Config::default(), clobbering the user's work.
        if !config::config_file_path().exists() {
            return;
        }

        let new = match config::Config::load() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("config reload: failed to parse config file: {e}");
                self.set_status(
                    format!("Config error — kept previous settings: {e}"),
                    super::StatusLevel::Error,
                );
                return;
            }
        };

        let appearance_changed = new.appearance_snapshot() != self.config.appearance_snapshot();
        let restart_changed = config::has_restart_changes(&self.config, &new);

        // True no-op: nothing changed. This absorbs the FS event from the in-app
        // theme picker (ui.theme is appearance-only, so both flags are false when
        // the picker persists a theme that the running config already reflects).
        if !appearance_changed && !restart_changed {
            return;
        }

        if restart_changed {
            self.set_status(
                String::from("Config updated — some changes require a restart to take effect"),
                super::StatusLevel::Warning,
            );
        }

        if appearance_changed {
            self.apply_appearance(&new);
            if !restart_changed {
                self.set_status_info(String::from("Config reloaded"));
            }
        }
    }
}
