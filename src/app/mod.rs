//! アプリケーション状態と、パネル間のフォーカス遷移。

mod appearance;
mod code_nav;
mod commands;
mod focus;
pub use focus::FocusState;
mod lifecycle;
mod panel_resize;
mod repo;
mod review;
mod review_commands;
mod review_edit;
mod review_history;
mod review_publish;
mod state;
pub use state::{
    Appearance, ChangeWatch, PanelLayout, PanelNumberOverlay, PublishState, RepoState,
    RevidereState, SessionStats, Ticks, UpdateFlow, ViewRestore, WtbarState,
};
mod types;
mod update;
mod view_state;
pub use view_state::OpenAs;

use std::collections::HashSet;
use std::path::PathBuf;

use crate::config;
use crate::diff_state::DiffState;
use crate::explorer::Explorer;
use crate::explorer::hover::ListHover;
use crate::git_engine;
use crate::keymap::KeyMap;
use crate::overlay::{ActiveOverlay, OverlayManager};
use crate::pty_manager;
use crate::reflow::ReflowView;
use crate::review_state::ReviewState;
use crate::review_store::ReviewStore;
use crate::terminal::editor::EditorPanel;
use crate::terminal::state::TerminalState;
use crate::theme::Theme;
use crate::viewer::ViewerState;
use crate::viewer::code_nav_state::CodeNav;
use crate::worktree::ops::WorktreeManager;
use crate::worktree::state::WorktreeList;

pub use crate::types::Focus;
pub use crate::types::{
    GrabbedBranch, Notice, PendingViewRestore, PendingWorktree, PendingWorktreeOp, SmartGenResult,
    StatusLevel, StatusMessage, WorktreeInputMode, WorktreeListRow, WorktreeOpResult,
};
pub use panel_resize::{Divider, ResizeDir};
pub use types::{BackgroundOps, BgDiffResult, CcusageInfo};
pub use update::UpdateState;

/// すべての UI パネルで共有されるトップレベルの状態。
pub struct App {
    pub needs_redraw: bool,
    pub focus: FocusState,
    pub overlays: OverlayManager,
    /// いま開いているリポジトリの同一性と、切り替え先の候補。
    pub repo: RepoState,
    pub should_quit: bool,
    /// Some ⟺ エディタの PTY が動いていて Explorer+Viewer を占有している。
    /// [App::open_in_editor] と [App::exit_editor] だけがこれと Focus::Editor を
    /// 対で扱うので、不変条件はその 2 つの中に閉じている。
    pub editor: Option<EditorPanel>,
    /// revidere の成果物と、実行中の解析。
    pub revidere: RevidereState,
    /// レビューコメントの GitHub 公開フロー (確認待ち + 実行中の処理)。
    pub publish: PublishState,
    /// 発見済みの worktree 一覧と、そこへの選択。
    pub worktrees: WorktreeList,
    pub config: config::Config,
    /// デフォルトにユーザの上書きを重ねた解決済みのマップ。
    pub keymap: KeyMap,
    pub appearance: Appearance,
    pub explorer: Explorer,
    pub viewer: ViewerState,
    pub diff_state: DiffState,
    /// DB を開けなかった場合は None。
    pub review_store: Option<ReviewStore>,
    pub review_state: ReviewState,
    pub terminal: TerminalState,
    pub worktree_mgr: WorktreeManager,
    pub status_message: Option<StatusMessage>,
    pub change_watch: ChangeWatch,

    /// レイアウト矩形のキャッシュ、ターミナル列の分割比、境界のドラッグ。
    pub layout: PanelLayout,

    pub list_hover: ListHover,

    pub ticks: Ticks,

    /// セッション統計 (ゲーミフィケーション) と ccusage のキャッシュ。
    pub stats: SessionStats,

    /// 自己更新: 検出 → 確認 → インストール → 再起動。
    pub update: UpdateFlow,

    pub clipboard: Option<copypasta::ClipboardContext>,

    pub decoration_states: crate::worktree::decoration::DecorationStates,

    /// 選択中 worktree のブランチ系譜と PR 情報。
    pub branch_details: git_engine::BranchDetails,
    pub gh_available: bool,

    /// 次のフレームで Claude セッションを自動再開する (一度きり)。
    pub pending_auto_resume: bool,

    /// 「ユーザがどこを見ていたか」の保存と復元。
    pub view_restore: ViewRestore,

    /// 画面上端の worktree モニタストリップ (横スクロール位置 + 当たり判定)。
    pub wtbar: WtbarState,

    /// どのメニューが開いているかと、直近の描画で記録したクリック領域。
    pub menu: crate::menu::MenuState,

    /// シンボル索引、ジャンプ履歴、付随するポップアップ。
    pub code_nav: CodeNav,

    /// イベントループがポーリングするバックグラウンド処理。
    pub bg: BackgroundOps,

    /// 最近作った worktree。バッジを出し、選択でクリアする。
    pub new_worktree_paths: HashSet<PathBuf>,

    /// Alt+/ で出す各パネルの番号バッジ。2 秒で消える。
    pub panel_number_overlay: PanelNumberOverlay,

    pub reflow: ReflowView,
}

/// 解決規則は [config::Config::theme_name] が持つ。ハイライト側も同じここを
/// 通すので、UI とコードで別のテーマを見ることはない。
fn resolve_theme_name(cfg: &config::Config) -> String {
    cfg.theme_name().to_string()
}

/// [Theme] の唯一の構築点。起動時・ピッカー・ライブリロード・OSC11 の自動切り替えが
/// ハイコントラストのトグルを取り違えないよう、ここに集めている。
fn build_theme(name: &str, high_contrast: bool) -> Theme {
    let theme = Theme::from_name(name);
    if high_contrast {
        theme.high_contrast()
    } else {
        theme
    }
}

impl App {
    pub fn is_any_overlay_active(&self) -> bool {
        self.overlays.active != ActiveOverlay::None
            || self.worktree_mgr.input_mode != WorktreeInputMode::Normal
            || self.review_state.input_mode != crate::review_state::ReviewInputMode::Normal
            || self.review_state.template_picker_active
            || self.review_state.comment_detail_active
            || self.update.is_active()
    }

    /// アプリケーションの終了をリクエストする。
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// 次のフレームでの再描画をリクエストする。
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// スタイル付きのステータスメッセージを設定する。
    pub fn set_status(&mut self, text: String, level: StatusLevel) {
        self.status_message = Some(StatusMessage::new(text, level, self.ticks.ui()));
    }

    /// 通常のinfoステータスメッセージを設定する（後方互換のための省略形）。
    pub fn set_status_info(&mut self, text: String) {
        self.set_status(text, StatusLevel::Info);
    }

    // 公開アクセサヘルパー

    /// worktreeの識別子として使われるブランチ名を返す。
    pub fn selected_worktree_branch(&self) -> String {
        self.worktrees
            .get(self.worktrees.selected_index())
            .map(|w| w.branch.clone())
            .unwrap_or_default()
    }

    /// 現在選択中のworktreeが __grab ブランチ上にある場合 true を返す
    /// （つまり実際のブランチがmainへgrabされ、一時的なチェックアウトを保持している状態）。
    pub fn is_selected_worktree_grabbed(&self) -> bool {
        self.worktrees
            .get(self.worktrees.selected_index())
            .map(|w| w.branch.ends_with("__grab"))
            .unwrap_or(false)
    }

    /// 現在選択中のworktreeのディレクトリパスを返す。
    pub fn selected_worktree_path(&self) -> PathBuf {
        self.worktrees
            .get(self.worktrees.selected_index())
            .map(|w| w.path.clone())
            .unwrap_or_else(|| self.repo.path.clone())
    }

    /// worktreeごとにグループ化されたすべてのClaude Codeセッションを返す。
    ///
    /// Vec<(wt_index, branch_name, sessions)> を返す。各セッションは
    /// (pty_index, label) で、worktreeインデックス順にソートされている。
    #[allow(clippy::type_complexity)]
    pub fn all_cc_sessions_by_worktree(&self) -> Vec<(usize, String, Vec<(usize, String)>)> {
        use std::collections::BTreeMap;

        let sessions = self.terminal.pty_manager.sessions();
        // worktreeインデックスでグループ化する。
        let mut groups: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();

        for (pty_idx, session) in sessions.iter().enumerate() {
            if session.kind != pty_manager::SessionKind::ClaudeCode {
                continue;
            }
            // セッションのworking_dirをworktreeに突き合わせる。
            if let Some(wt_idx) = self
                .worktrees
                .iter()
                .position(|wt| wt.path == session.working_dir)
            {
                groups
                    .entry(wt_idx)
                    .or_default()
                    .push((pty_idx, session.label.clone()));
            }
        }

        groups
            .into_iter()
            .map(|(wt_idx, sessions)| {
                let branch = self
                    .worktrees
                    .get(wt_idx)
                    .map(|wt| wt.branch.clone())
                    .unwrap_or_default();
                (wt_idx, branch, sessions)
            })
            .collect()
    }

    /// worktree + インラインセッション行のフラットなリストを再構築する。
    pub fn rebuild_worktree_list_rows(&mut self) {
        let groups = self.all_cc_sessions_by_worktree();
        let mut rows = Vec::new();
        for (i, _wt) in self.worktrees.iter().enumerate() {
            rows.push(WorktreeListRow::Worktree(i));
            // このworktreeに属するセッションを探す。
            if let Some((_, _, sessions)) = groups.iter().find(|(wt_idx, _, _)| *wt_idx == i) {
                for (pty_idx, _label) in sessions {
                    rows.push(WorktreeListRow::Session {
                        wt_idx: i,
                        pty_idx: *pty_idx,
                    });
                }
            }
        }
        // 行の選択のクランプは set_rows が担う。
        self.worktrees.set_rows(rows);
    }

    /// 行の選択 (row_selected) から worktree の選択を導出する。
    pub fn sync_selected_worktree(&mut self) {
        if let Some(row) = self.worktrees.rows.get(self.worktrees.row_selected) {
            let wt_idx = match *row {
                WorktreeListRow::Worktree(i) => i,
                WorktreeListRow::Session { wt_idx, .. } => wt_idx,
            };
            if wt_idx < self.worktrees.len() {
                self.worktrees.select(wt_idx);
            }
        }
    }
}
