//! 設定の読み込みと永続化。
//!
//! ~/.config/conductor/config.toml から TOML 設定ファイルを読み込み、
//! アプリケーションの他部分に型付けされた設定を公開する。
//!
//! すべてのフィールドが serde のデフォルト値を持つため、設定ファイルは
//! 空でも一部だけの指定でもよい。

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
// AppearanceSnapshot はまだクレート内のどこからも名前で参照されていない
// (呼び出し側は型推論やフィールドアクセスを使っている)が、このモジュールの
// 公開インタフェースの一部なので、分割によって隠れてしまわないよう
// crate::config::* 配下で re-export し続ける。
#[allow(unused_imports)]
pub use sections::{
    ApiConfig, CcusageConfig, DiffConfig, DiffView, GeneralConfig, LayoutConfig, ReviewConfig,
    TerminalConfig, UiConfig, UpdatesConfig, ViewerConfig,
};
#[allow(unused_imports)]
pub use snapshot::{AppearanceSnapshot, has_restart_changes};
pub use syntax_theme::{syntax_theme_id, syntect_theme_for};

use persist::write_atomic;

// トップレベルの Config

/// アプリケーション全体の設定。
///
/// config.toml の [section] 構成をそのまま反映している。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    /// [general] -- リポジトリパス、メインブランチ、シェル。
    pub general: GeneralConfig,
    /// [terminal] -- スクロールバックの上限。
    pub terminal: TerminalConfig,
    /// [viewer] -- シンタックステーマ、タブ幅、折り返し。
    pub viewer: ViewerConfig,
    /// [diff] -- diff 表示のオプション。
    pub diff: DiffConfig,
    /// [review] -- コードレビュー用プロンプトの設定。
    pub review: ReviewConfig,
    /// [keybinds] -- ユーザによるキーバインド上書き(任意)。keymap-config の
    /// key→action TOML スキーマ([keybinds.keys], [keybinds.layers.<name>])。
    /// 生のテーブルとして保持し、スキーマを所有する keymap::KeyMap に渡す。
    pub keybinds: toml::Table,
    /// [ccusage] -- Claude Code のトークン使用量表示。
    pub ccusage: CcusageConfig,
    /// [updates] -- 起動時バージョンチェックの設定。
    pub updates: UpdatesConfig,
    /// [api] -- Gemini API の設定。
    pub api: ApiConfig,
    /// [ui] -- UI 外観の設定(テーマなど)。
    #[serde(default)]
    pub ui: UiConfig,
    /// [layout] -- パネル比率の上書き。
    #[serde(default)]
    pub layout: LayoutConfig,
}

impl Config {
    /// 有効な UI テーマ名。
    ///
    /// [ui] theme が優先される。存在しない場合は、[ui] セクション導入前の
    /// config との後方互換性のために [viewer] theme が使われる。
    ///
    /// UI の配色もシンタックスハイライトのテーマも、必ずここを通して名前を
    /// 決める。片方が viewer.theme を直接読んでいたせいで、テーマピッカーで
    /// ui.theme を切り替えてもコードの配色だけが取り残されていた。
    pub fn theme_name(&self) -> &str {
        self.ui.theme.as_deref().unwrap_or(&self.viewer.theme)
    }

    /// ~/.config/conductor/config.toml から設定を読み込む。
    ///
    /// ファイルが存在しない場合は Config::default() にフォールバックする。
    pub fn load() -> Result<Self> {
        let config_path = config_file_path();

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            let mut config: Config = toml::from_str(&contents)?;
            config.expand_paths();
            Ok(config)
        } else {
            // コメント付きのデフォルト設定ファイルを生成する。
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

    /// パス値を持つフィールドの先頭にあるチルダ(~)を展開する。
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
