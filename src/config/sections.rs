//! config.toml のセクションごとの構造体。
//!
//! config.toml の [section] ごとに1つの構造体があり、それぞれが独自の
//! Default impl を持つ。config ファイルが空でも部分的な指定でも成立する
//! よう、全フィールドに serde の default を付けている。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::icons::IconSet;

/// [general] セクション。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// 起動時に開くデフォルトリポジトリのパス。
    pub repo: Option<PathBuf>,
    /// main/trunk ブランチの名前(例: "main" や "master")。
    pub main_branch: String,
    /// PTY セッションで使うシェルの実行ファイル。
    pub shell: String,
    /// マルチリポジトリ対応のための、追加リポジトリパスの一覧。
    pub repos: Vec<PathBuf>,
    /// worktree 用のカスタムベースディレクトリ。
    /// None の場合は <repo-parent>/<repo-name>-worktrees/ がデフォルトになる。
    pub worktree_dir: Option<PathBuf>,
    /// 起動時に前回実行の Claude Code セッションを自動的に再開する。
    pub auto_resume: bool,
    /// main worktree でもセッションを自動再開する(auto_resume が true の
    /// ときのみ意味を持つ)。デフォルトは false — main worktree は寿命が
    /// 長くセッションが積み重なるので、起動のたびに最新のものを開き直すのは
    /// 通常望まれないため。grab したセッションはこの設定に関わらず常に
    /// 再開される。
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
            auto_resume: true,
            auto_resume_main: false,
        }
    }
}

/// $SHELL からユーザのシェルを検出する。無ければ /bin/sh にフォールバックする。
pub(super) fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| String::from("/bin/sh"))
}

/// [terminal] セクション。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// 非アクティブ(バックグラウンド)セッションで保持するスクロールバック行数。
    pub inactive_scrollback: usize,
    /// アクティブ(フォアグラウンド)セッションで保持するスクロールバック行数。
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerConfig {
    /// シンタックスハイライトのテーマ名。
    pub theme: String,
    /// シンタックスハイライト用のカスタム .tmTheme ファイルへのパス。
    pub syntax_theme_file: Option<String>,
    /// タブ1つあたりのスペース数。
    pub tab_width: usize,
    /// 長い行をソフトラップするかどうか。
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

/// [diff] セクション。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffConfig {
    /// unified 表示か side-by-side 表示か。
    pub default_view: DiffView,
    /// 行内の単語単位の変更をハイライトするかどうか。
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

/// サポートする diff の表示スタイル。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffView {
    Unified,
    SideBySide,
}

/// [review] セクション。
///
/// いまは設定項目を持たない。レビューの生成に関わる設定 (どの AI がどの言語で
/// 書くか) は revidere 側の config.toml へ移った。セクション自体は残してある —
/// 既存の config.toml に [review] が書かれており、消すと未知セクションになる。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {}

/// [ccusage] セクション。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CcusageConfig {
    /// タイトルバーに Claude Code のトークン使用量表示を有効にする。
    pub enabled: bool,
    /// ポーリング間隔(秒)。
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

/// [updates] セクション。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatesConfig {
    /// 起動時に新しいバージョンをチェックする。
    pub check_on_startup: bool,
    /// アップデートチェックの最小間隔(秒、キャッシュの TTL)。
    pub check_interval_secs: u64,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            check_interval_secs: 3600, // 1時間
        }
    }
}

/// [api] セクション。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Gemini API のモデル ID。
    pub model: String,
    /// どの LLM プロバイダーを使うか。各プロバイダーは独立していて、失敗は別プロバイダーへの
    /// フォールバックではなくユーザに表面化する。"gemini" (既定) は HTTP API、"command" は
    /// ユーザが指定する外部コマンド。claude CLI を実行する組み込みプロバイダーは存在しない。
    pub provider: String,
    /// provider = "command" のときに実行する AI ツール。argv 形式で、シェルを介さず直接実行する。
    ///
    /// {prompt} と {workdir} は任意の引数の中で置換される。{prompt} が無い場合は prompt が
    /// stdin へ渡される。completion は stdout から読む。詳細は ai_caller.rs のプロトコル。
    pub command: Vec<String>,
    /// command プロバイダーの実時間タイムアウト(秒)。0 で無効化する。
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

/// [layout] セクション — パネル比率の上書き。
///
/// 値はパーセンテージ(0〜100)。worktree 列は常に幅0(worktree モニターは
/// 上部の帯に住んでいる); terminal 列は explorer と viewer が使った残りの
/// 幅を受け取る。これらの比率はデフォルト(最大化していない)レイアウトに
/// のみ適用される。パネルを最大化するとこれまでどおり上書きされる。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    /// フレーム全体の幅に対する Explorer 列の幅のパーセンテージ。
    pub explorer_width_pct: u16,
    /// フレーム全体の幅に対する Viewer 列の幅のパーセンテージ。
    pub viewer_width_pct: u16,
    /// terminal 列の高さに対する Claude Code 領域の高さのパーセンテージ。
    /// shell 領域は残りを受け取る。
    pub terminal_split_pct: u16,
    /// Explorer 列の高さに対するファイルツリーの高さのパーセンテージ。
    /// 下の changed-files 一覧は残りを受け取る。
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

/// [ui] セクション — UI 外観の上書き。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// UI カラーテーマ名。None の場合は後方互換性のため [viewer] theme に
    /// フォールバックする。Light テーマの選択肢: catppuccin-latte、
    /// solarized-light、github-light。Dark は Theme::all_names() を参照。
    pub theme: Option<String>,
    /// 現在のテーマに高コントラスト変換を適用する: 視認性を高めるため、
    /// 薄暗いグレー・本文テキスト・アクセントを(dark テーマでは)明るく、
    /// (light テーマでは)濃くする。組み込み・カスタムを問わず全テーマで
    /// 動作する。
    pub high_contrast: bool,
    /// ファイルアイコンに使う文字セット。None のときは初回起動時に端末を見て
    /// 決定し、その結果をこのファイルへ書き戻す (startup::apply_auto_icons)。
    pub icons: Option<IconSet>,
    /// 起動時にパネルが組み上がる演出を出すか。SSH 越しや描画の遅い端末では
    /// 邪魔になりうるので切れるようにしてある。
    pub startup_animation: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: None,
            high_contrast: false,
            icons: None,
            startup_animation: true,
        }
    }
}

impl UiConfig {
    /// 実際に描画で使う文字セット。未設定のまま自動判定も効かなかった場合は
    /// どの端末でも読めるフォールバックにする。
    pub fn icon_set(&self) -> IconSet {
        self.icons.unwrap_or(IconSet::Unicode)
    }
}
