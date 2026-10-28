//! ~/.config/conductor/config.toml の読み込みと書き戻し。
//!
//! 全フィールドが serde の既定値を持つので、ファイルは空でも部分指定でもよい。
//! 知らないセクションや鍵は無視する。過去の版が書いていた設定が残っていても落ちない。

mod persist;
mod sections;
mod snapshot;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use persist::{
    DEFAULT_CONFIG, config_file_path, persist_layout_proportions, persist_ui_high_contrast,
    persist_ui_icons, persist_ui_theme,
};
pub use sections::{
    ApiConfig, ClaudeAccount, ClaudeConfig, DiffConfig, GeneralConfig, LayoutConfig, ReviewConfig,
    TerminalConfig, UiConfig, UpdatesConfig, ViewerConfig,
};
pub use snapshot::{AppearanceSnapshot, has_restart_changes};

/// config.toml の [section] 構成をそのまま写した設定全体。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub terminal: TerminalConfig,
    pub viewer: ViewerConfig,
    pub diff: DiffConfig,
    /// [keybinds] は生のテーブルのまま持ち、スキーマを知る keymap::KeyMap に渡す。
    pub keybinds: toml::Table,
    pub api: ApiConfig,
    pub ui: UiConfig,
    pub layout: LayoutConfig,
    pub updates: UpdatesConfig,
    pub review: ReviewConfig,
    pub claude: ClaudeConfig,
}

impl Config {
    /// 有効な UI テーマ名。[ui] theme が無ければ [ui] 導入前の [viewer] theme に落ちる。
    ///
    /// UI の配色もシンタックスハイライトも必ずここを通す。片方が viewer.theme を直接
    /// 読んでいたせいで、テーマピッカーで ui.theme を切り替えてもコードの配色だけが
    /// 取り残されたことがある。
    pub fn theme_name(&self) -> &str {
        self.ui.theme.as_deref().unwrap_or(&self.viewer.theme)
    }

    /// ~/.config/conductor/config.toml を読む。無ければ既定のファイルを生成して既定値を返す。
    pub fn load() -> Result<Self> {
        Self::load_from(&config_file_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            if let Err(e) = persist::write_default(path) {
                log::warn!("failed to write default config: {e}");
            } else {
                log::info!("generated default config at {}", path.display());
            }
            return Ok(Self::default());
        }
        let mut config: Config = toml::from_str(&std::fs::read_to_string(path)?)?;
        config.expand_paths();
        Ok(config)
    }

    fn expand_paths(&mut self) {
        let general = &mut self.general;
        general.repo = general.repo.as_deref().map(expand_tilde);
        general.repos = general.repos.iter().map(|p| expand_tilde(p)).collect();
        general.worktree_dir = general.worktree_dir.as_deref().map(expand_tilde);
        self.viewer.syntax_theme_file = self
            .viewer
            .syntax_theme_file
            .as_deref()
            .map(|p| expand_tilde(Path::new(p)).to_string_lossy().into_owned());
        let review = &mut self.review;
        review.auto_review_repos = review
            .auto_review_repos
            .iter()
            .map(|p| expand_tilde(p))
            .collect();
        for account in &mut self.claude.accounts {
            account.config_dir = account.config_dir.as_deref().map(expand_tilde);
        }
    }

    /// 読む Claude Code の config dir。accounts の順に、最後に窓自身の CLAUDE_CONFIG_DIR (無ければ
    /// ~/.claude) を足す。窓の env で起こした Claude の状態も見えるように。同じ実体は先に出た綴りだけ残す。
    pub fn claude_dirs(&self) -> Vec<PathBuf> {
        claude_dirs_from(
            &self.claude.accounts,
            crate::cc_hook::statusline::claude_config_dir(),
            dirs::home_dir(),
        )
    }

    /// その config dir の下の sessions/ から読んだセッションのアカウント。綴りは [Self::claude_dirs] のもの。
    pub fn account_of(&self, sessions_dir: &Path) -> Option<&ClaudeAccount> {
        let home = dirs::home_dir();
        self.claude.accounts.iter().find(|account| {
            account_dir(account, home.as_deref())
                .is_some_and(|dir| dir.join("sessions") == sessions_dir)
        })
    }

    /// 起動時に知らせる accounts の書き損じ。重なりは先に書いた方が使われる。
    pub fn claude_account_problems(&self) -> Vec<String> {
        let home = dirs::home_dir();
        let mut problems = Vec::new();
        let mut names = std::collections::HashSet::new();
        let mut dirs = std::collections::HashSet::new();
        for account in &self.claude.accounts {
            if !names.insert(account.name.as_str()) {
                problems.push(format!(
                    "Claude account \"{}\" is listed twice",
                    account.name
                ));
            }
            if account.color.is_some() && account.color().is_none() {
                problems.push(format!(
                    "Claude account \"{}\" has an unknown color",
                    account.name
                ));
            }
            if let Some(dir) = account_dir(account, home.as_deref())
                && !dirs.insert(canonical(&dir))
            {
                problems.push(format!(
                    "Claude account \"{}\" uses the same config dir as an earlier one",
                    account.name
                ));
            }
        }
        problems
    }
}

fn account_dir(account: &ClaudeAccount, home: Option<&Path>) -> Option<PathBuf> {
    account
        .config_dir
        .clone()
        .or_else(|| home.map(|home| home.join(".claude")))
}

fn claude_dirs_from(
    accounts: &[ClaudeAccount],
    window: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    accounts
        .iter()
        .filter_map(|account| account_dir(account, home.as_deref()))
        .chain(window)
        .filter(|dir| seen.insert(canonical(dir)))
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match (s.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) => PathBuf::from(format!("{}{rest}", home.to_string_lossy())),
        _ => path.to_path_buf(),
    }
}
