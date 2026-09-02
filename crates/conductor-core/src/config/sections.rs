//! config.toml のセクションごとの構造体。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::diff_state::DiffView;
use crate::icons::IconSet;

/// [general] セクション。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// 起動時に開くリポジトリ。
    pub repo: Option<PathBuf>,
    pub main_branch: String,
    /// PTY で起動するシェル。既定は $SHELL、無ければ /bin/sh。
    pub shell: String,
    /// リポジトリ切り替えの候補。
    pub repos: Vec<PathBuf>,
    /// worktree の置き場。None なら <repo-parent>/<repo-name>-worktrees/。
    pub worktree_dir: Option<PathBuf>,
    /// 起動時に前回の Claude Code セッションを再開する。
    pub auto_resume: bool,
    /// main worktree でも再開する。main は寿命が長くセッションが積み重なるので既定は
    /// off。grab したセッションはこの設定に関わらず再開する。
    pub auto_resume_main: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            repo: None,
            main_branch: String::from("main"),
            shell: std::env::var("SHELL").unwrap_or_else(|_| String::from("/bin/sh")),
            repos: Vec::new(),
            worktree_dir: None,
            auto_resume: true,
            auto_resume_main: false,
        }
    }
}

/// [terminal] セクション。値はスクロールバックの行数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub inactive_scrollback: usize,
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

/// [viewer] セクション。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerConfig {
    /// [ui] theme 導入前のテーマ名。[Config::theme_name](super::Config::theme_name) が読む。
    pub theme: String,
    /// シンタックスハイライトに使う .tmTheme ファイル。
    pub syntax_theme_file: Option<String>,
    pub tab_width: usize,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            theme: String::from("catppuccin-mocha"),
            syntax_theme_file: None,
            tab_width: 2,
        }
    }
}

/// [diff] セクション。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffConfig {
    pub default_view: DiffView,
    /// 行内の単語単位の変更を強調する。
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

/// [api] セクション。conductor 自身のプロンプト (worktree 名の提案など) に答える AI。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// "gemini" か "command"。互いにフォールバックせず、失敗はそのまま表面化する。
    pub provider: String,
    /// provider = "gemini" のモデル ID。
    pub model: String,
    /// provider = "command" で起動する argv。シェルは介さない。{prompt} と {workdir} を
    /// 置換し、{prompt} が無ければプロンプトは stdin に渡す。
    pub command: Vec<String>,
    /// command の実時間タイムアウト。0 で無効。
    pub command_timeout_secs: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            provider: String::from("gemini"),
            model: String::from("gemini-2.5-flash"),
            command: Vec::new(),
            command_timeout_secs: 60,
        }
    }
}

/// [layout] セクション。値はパーセント。terminal 列は explorer と viewer の残り幅を取る。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    pub explorer_width_pct: u16,
    pub viewer_width_pct: u16,
    /// terminal 列のうち Claude Code 領域の高さ。残りが shell。
    pub terminal_split_pct: u16,
    /// explorer 列のうちファイルツリーの高さ。残りが変更ファイル一覧。
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

/// [ui] セクション。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// UI カラーテーマ名。None なら [viewer] theme に落ちる。
    pub theme: Option<String>,
    pub high_contrast: bool,
    /// None なら初回起動時に端末から判定し、その結果をファイルへ書き戻す
    /// ([persist_ui_icons](super::persist_ui_icons))。
    pub icons: Option<IconSet>,
}

impl UiConfig {
    /// 描画に使う文字セット。未設定で自動判定も効かなければどの端末でも読める方。
    pub fn icon_set(&self) -> IconSet {
        self.icons.unwrap_or(IconSet::Unicode)
    }
}
