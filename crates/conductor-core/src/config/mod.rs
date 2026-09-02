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
    ApiConfig, DiffConfig, GeneralConfig, LayoutConfig, TerminalConfig, UiConfig, ViewerConfig,
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
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match (s.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) => PathBuf::from(format!("{}{rest}", home.to_string_lossy())),
        _ => path.to_path_buf(),
    }
}
