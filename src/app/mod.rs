//! App state and focus management.
//!
//! This module defines the top-level application state, the unified panel
//! layout focus model, and transitions between panels.

mod appearance;
mod code_nav;
mod commands;
mod editor;
mod focus;
mod lifecycle;
mod panel_resize;
mod reflow;
mod repo;
mod review;
mod review_commands;
mod review_delete;
mod review_edit;
mod review_history;
mod review_publish;
mod review_walkthrough;
pub use review_walkthrough::WalkthroughGenerations;
mod state;
pub use state::{
    CodeNav, Highlighting, ListHover, LoadedWalkthrough, PanelLayout, PanelNumberOverlay,
    PublishState, RepoState, RichState, SessionStats, ThemeSelection, UpdateFlow, ViewRestore,
    WalkthroughState, WorktreeList, WtbarState,
};
mod terminal;
mod terminal_cc_state;
mod terminal_resize;
mod terminal_resume;
mod types;
mod update;
mod view_state;
mod walkthrough_view;
mod worktree;
mod worktree_branches;
mod worktree_commands;
mod worktree_crud;
mod worktree_grab;
mod worktree_grep;
mod worktree_pr;
mod worktree_smart;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;


use crate::config;
use crate::diff_state::DiffState;
use crate::git_engine;
use crate::keymap::KeyMap;
use crate::overlay::{ActiveOverlay, OverlayManager};
use crate::pty_manager;
use crate::review_state::ReviewState;
use crate::review_store::ReviewStore;
use crate::terminal_state::TerminalState;
use crate::theme::Theme;
use crate::viewer::ViewerState;
use crate::worktree_ops::WorktreeManager;

pub use code_nav::{
    UnderlineColorKind, masked_symbol_at_column, popup_highlight_range, underline_color_kind,
};
pub use editor::EditorPanel;
pub use focus::Focus;
pub use panel_resize::{Divider, ResizeDir};
pub use reflow::ReflowView;
pub use types::{
    BackgroundOps, BgDiffResult, CcusageInfo, DirtyPanels, GrabbedBranch, PendingViewRestore,
    PendingWorktree, PendingWorktreeOp, SmartGenResult, StatusLevel, StatusMessage,
    WorktreeInputMode, WorktreeListRow, WorktreeOpResult,
};
pub use update::UpdateState;

/// Top-level application state shared across all UI panels.
pub struct App {
    /// Tracks which panels need re-rendering.
    pub dirty: DirtyPanels,
    /// Current panel focus.
    pub focus: Focus,
    /// フォーカスが直前にあったパネルと、移った時刻。ボーダー色のグライド
    /// アニメーション (`animated_border_color`) だけがこの 2 つを読む。
    pub focus_prev: Focus,
    /// When focus last changed, for timing the border transition.
    pub focus_changed_at: std::time::Instant,
    /// All overlay popup states (switch-branch, grab, prune, help, etc.).
    pub overlays: OverlayManager,
    /// いま開いているリポジトリの同一性と、切り替え先の候補。
    pub repo: RepoState,
    /// Whether the application should quit on the next tick.
    pub should_quit: bool,
    /// The embedded editor panel, when active. `Some` ⟺ an editor PTY is running
    /// and occupying the merged Explorer+Viewer region; `None` is the normal
    /// (no-editor) layout. Set by [`App::open_in_editor`] and torn down by
    /// [`App::exit_editor`] (the only two methods that pair this field with
    /// `Focus::Editor`, keeping the invariant local).
    pub editor: Option<EditorPanel>,
    /// AI ウォークスルー: 生成中のものと、いま読み込まれているもの。
    pub walkthrough: WalkthroughState,
    /// レビューコメントの GitHub 公開フロー (確認待ち + 実行中の処理)。
    pub publish: PublishState,
    /// 発見済みの worktree 一覧と、そこへの選択 (行の平坦化リストを含む)。
    pub worktrees: WorktreeList,
    /// Application configuration loaded from config file.
    pub config: config::Config,
    /// Resolved keybinding map (defaults + user overrides).
    pub keymap: KeyMap,
    /// 描画に使う配色。フレームごとに読まれるので 1 階層浅いところに置く。
    /// 組み立ての元データは [`Self::theme_sel`]。
    pub theme: Theme,
    /// [`Self::theme`] を組み立てるための元データ (テーマ名 + ハイコントラスト)。
    pub theme_sel: ThemeSelection,
    /// State for the Explorer/Viewer panel (file tree + file content).
    pub viewer_state: ViewerState,
    /// State for the Diff data (used for inline highlights in Viewer).
    pub diff_state: DiffState,
    /// SQLite-backed review comment store. `None` if the DB could not be opened.
    pub review_store: Option<ReviewStore>,
    /// UI state for review comments.
    pub review_state: ReviewState,
    /// Terminal / PTY state.
    pub terminal: TerminalState,
    /// Worktree management state (creation, deletion, smart worktree, etc.).
    pub worktree_mgr: WorktreeManager,
    /// Status message (flash message) shown in the status bar.
    pub status_message: Option<StatusMessage>,
    /// Last known HEAD oid for the selected worktree (for change-detection polling).
    pub last_poll_head_oid: Option<String>,
    /// Last known status signature (added, modified, deleted) for the selected worktree.
    pub last_poll_status: Option<(usize, usize, usize, usize)>,

    /// syntect によるシンタックスハイライトの共有資源。
    pub highlight: Highlighting,
    /// Per-id cache of rendered Markdown (comment/reply bodies), so the inline
    /// thread box doesn't re-parse/highlight every frame.
    pub markdown_cache: crate::ui::markdown::MarkdownCache,

    /// Which panel is currently expanded to 100% (via the [<=>] button).
    /// `None` means no panel is expanded (default layout).
    pub expanded_panel: Option<Focus>,

    /// パネルの幾何: レイアウト矩形のキャッシュ、ターミナル列の分割比、
    /// マウスによる境界リサイズ。
    pub layout: PanelLayout,

    /// Explorer の 2 つのリスト (ファイルツリー / Changed files) のホバー追跡。
    pub list_hover: ListHover,

    /// Frame counter for UI animations (e.g. waiting-state pulse).
    pub ui_tick: u64,
    /// Independent tick counter for decoration animation (incremented at fixed interval).
    pub decoration_tick: u64,

    /// Notification bar badge positions: (start_col, end_col, branch_name).
    /// Populated during rendering for click-to-jump.
    pub notification_bar_badges: Vec<(u16, u16, String)>,

    /// セッション統計 (ゲーミフィケーション) と ccusage のキャッシュ。
    pub stats: SessionStats,
    /// HEAD oid per worktree branch (for commit detection).
    pub worktree_heads: HashMap<String, String>,

    /// 自己更新フロー: 新バージョンの検出 → 確認 → インストール → 再起動。
    pub update: UpdateFlow,

    /// System clipboard context for Ctrl+V paste support.
    pub clipboard: Option<copypasta::ClipboardContext>,

    /// Animation state for all decoration modes.
    pub decoration_states: crate::ui::decoration::DecorationStates,

    // ── Branch details (worktree detail panel) ────────────────────
    /// Computed branch lineage and PR info for the selected worktree.
    pub branch_details: git_engine::BranchDetails,
    /// Whether the `gh` CLI is available on this system.
    pub gh_available: bool,

    // ── Auto-resume Claude sessions ─────────────────────────────
    /// Whether auto-resume should run on the next frame (one-shot).
    pub pending_auto_resume: bool,

    /// 「ユーザーがどこを見ていたか」の保存と復元。
    pub view_restore: ViewRestore,

    /// 画面上端の worktree モニタストリップ (横スクロール位置 + 当たり判定)。
    pub wtbar: WtbarState,

    /// Menu bar interaction state: which menu is focused or open, and the
    /// click regions recorded by the last bar/dropdown render.
    pub menu: crate::menu::MenuState,

    /// コードナビゲーション: シンボル索引、ジャンプ履歴、付随するポップアップ。
    pub code_nav: CodeNav,

    // ── Background operations (polled by the event loop) ─────────
    pub bg: BackgroundOps,

    // ── New worktree badge ──────────────────────────────────────
    /// Paths of worktrees recently created (for badge display). Cleared on selection.
    pub new_worktree_paths: HashSet<PathBuf>,

    /// Alt+/ で出す、各パネル上の番号バッジ (2 秒で自動的に消える)。
    pub panel_number_overlay: PanelNumberOverlay,

    // ── Party mode (hidden easter egg) ───────────────────────────
    /// When true, the UI goes full party: the focused panel's border
    /// glows in a flowing rainbow, syntax tokens turn rainbow, the title
    /// bar shimmers, and confetti drifts across the screen. Toggled from
    /// the command palette; not persisted (session-only secret).
    pub party_mode: bool,

    /// リッチモード (端末グラフィックス) の描画ティアと、それに紐づく資源。
    pub rich: RichState,

    // ── Reflow transcript view ───────────────────────────────────────────
    /// State for the read-only, word-wrapped session-log viewer that
    /// overlays the Claude PTY panel during infinite-scrollback mode.
    pub reflow: ReflowView,
}

/// Resolve the active UI theme name from config.
///
/// `[ui] theme` takes precedence; when absent, `[viewer] theme` is used for
/// backward compatibility with configs that predate the `[ui]` section.
fn resolve_theme_name(cfg: &config::Config) -> String {
    cfg.ui
        .theme
        .as_deref()
        .unwrap_or(&cfg.viewer.theme)
        .to_string()
}

/// Build the active [`Theme`] from a name, applying the high-contrast transform
/// when enabled. The single construction point so every call site (startup,
/// theme picker, live reload, OSC11 auto-switch) honors the toggle identically.
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
            || self.worktree_mgr.skip_reason.is_some()
    }

    /// Request the application to quit.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Set a styled status message.
    pub fn set_status(&mut self, text: String, level: StatusLevel) {
        self.status_message = Some(StatusMessage::new(text, level, self.ui_tick));
    }

    /// Set a plain info status message (backward-compatible shorthand).
    pub fn set_status_info(&mut self, text: String) {
        self.set_status(text, StatusLevel::Info);
    }

    // ── Public accessor helpers ─────────────────────────────────────

    /// Return the branch name used as the worktree identifier.
    pub fn selected_worktree_branch(&self) -> String {
        self.worktrees
            .get(self.worktrees.selected_index())
            .map(|w| w.branch.clone())
            .unwrap_or_default()
    }

    /// Return `true` if the currently selected worktree is on a `__grab` branch
    /// (i.e. its real branch was grabbed away to main and it holds a temporary checkout).
    pub fn is_selected_worktree_grabbed(&self) -> bool {
        self.worktrees
            .get(self.worktrees.selected_index())
            .map(|w| w.branch.ends_with("__grab"))
            .unwrap_or(false)
    }

    /// Return the directory path for the currently selected worktree.
    pub fn selected_worktree_path(&self) -> PathBuf {
        self.worktrees
            .get(self.worktrees.selected_index())
            .map(|w| w.path.clone())
            .unwrap_or_else(|| self.repo.path.clone())
    }

    /// Return all Claude Code sessions grouped by worktree.
    ///
    /// Returns `Vec<(wt_index, branch_name, sessions)>` where each session is
    /// `(pty_index, label)`, sorted by worktree index.
    #[allow(clippy::type_complexity)]
    pub fn all_cc_sessions_by_worktree(&self) -> Vec<(usize, String, Vec<(usize, String)>)> {
        use std::collections::BTreeMap;

        let sessions = self.terminal.pty_manager.sessions();
        // Group by worktree index.
        let mut groups: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();

        for (pty_idx, session) in sessions.iter().enumerate() {
            if session.kind != pty_manager::SessionKind::ClaudeCode {
                continue;
            }
            // Match session working_dir to a worktree.
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

    /// Rebuild the flat list of worktree + inline session rows.
    pub fn rebuild_worktree_list_rows(&mut self) {
        let groups = self.all_cc_sessions_by_worktree();
        let mut rows = Vec::new();
        for (i, _wt) in self.worktrees.iter().enumerate() {
            rows.push(WorktreeListRow::Worktree(i));
            // Find sessions belonging to this worktree.
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

    /// 行の選択 (`row_selected`) から worktree の選択を導出する。
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
