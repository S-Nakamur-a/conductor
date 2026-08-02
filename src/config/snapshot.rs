//! 外観スナップショットと再起動要否の変更検出。
//!
//! [AppearanceSnapshot] は [Config](super::Config) のうち Conductor を
//! 再起動せずに live で再読込できるフィールドの部分集合を捉える。
//! [has_restart_changes] はそれ以外すべてを識別する。

use super::{Config, DiffView};

/// "live-reloadable"(外観)なフィールドすべてを、ある時点でまるごと捉えたもの。
///
/// App::reload_appearance_config での冪等性ガードとして等価比較を使う:
/// スナップショットが実行中の状態と一致していれば何もしない。これによって
/// アプリ内テーマピッカーによる自己書き込みループが自然に吸収される。
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceSnapshot {
    pub ui_theme: Option<String>,
    pub ui_high_contrast: bool,
    pub viewer_theme: String,
    pub viewer_syntax_theme_file: Option<String>,
    pub viewer_tab_width: usize,
    // viewer.word_wrap は意図的に含めていない — 描画処理が未実装のため、
    // word_wrap を保存しても "Config reloaded" のフラッシュや見た目の変化を
    // 起こすべきではない。描画処理を組み込んだらここに追加すること。
    pub diff_word_diff: bool,
    pub diff_default_view: DiffView,
    pub general_decoration: String,
    pub layout_explorer_width_pct: u16,
    pub layout_viewer_width_pct: u16,
    pub layout_terminal_split_pct: u16,
    pub layout_explorer_split_pct: u16,
}

impl Config {
    /// 外観(live-reloadable)フィールドのスナップショットを捉える。
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

    /// live-reloadable な外観フィールドをすべて new から self へコピーする。
    ///
    /// 更新されるのは [AppearanceSnapshot] が追跡するフィールドと、config
    /// には追跡されているがまだスナップショットには入っていない
    /// viewer.word_wrap のみ。再起動が必要なフィールド(shell, scrollback,
    /// API 設定, keybinds など)は意図的に触らない。App::apply_appearance
    /// から、派生状態(syntect テーマ, diff, layout キャッシュなど)を
    /// 再構築する前に呼ばれる。
    pub fn adopt_appearance(&mut self, new: &Config) {
        self.ui.theme = new.ui.theme.clone();
        self.ui.high_contrast = new.ui.high_contrast;
        self.viewer.theme = new.viewer.theme.clone();
        self.viewer.syntax_theme_file = new.viewer.syntax_theme_file.clone();
        self.viewer.tab_width = new.viewer.tab_width;
        // word_wrap: 永続化のため config へはコピーするが、描画処理が未実装
        // なので AppearanceSnapshot には含めない。
        self.viewer.word_wrap = new.viewer.word_wrap;
        self.diff.word_diff = new.diff.word_diff;
        self.diff.default_view = new.diff.default_view;
        self.general.decoration = new.general.decoration.clone();
        self.layout = new.layout.clone();
    }
}

/// new が old と再起動必須フィールドのいずれかで異なる場合に true を返す。
///
/// 再起動必須フィールドは AppearanceSnapshot に含まれないもの全部を指す:
/// general.{repo, repos, worktree_dir, shell, main_branch, auto_resume,
/// auto_resume_main}, terminal.{active_scrollback, inactive_scrollback},
/// api.*, updates.*, ccusage.*, review.*, keybinds。
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
        || old.api.model != new.api.model
        || old.api.provider != new.api.provider
        || old.api.command != new.api.command
        || old.api.command_timeout_secs != new.api.command_timeout_secs
        || old.updates.check_on_startup != new.updates.check_on_startup
        || old.updates.check_interval_secs != new.updates.check_interval_secs
        || old.ccusage.enabled != new.ccusage.enabled
        || old.ccusage.poll_interval_secs != new.ccusage.poll_interval_secs
        || old.keybinds != new.keybinds
}
