//! アプリケーション状態とフォーカス管理。
//!
//! このモジュールはトップレベルのアプリケーション状態、統一されたパネル
//! レイアウトのフォーカスモデル、パネル間の遷移を定義する。

mod appearance;
mod code_nav;
mod commands;
mod focus;
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
    Highlighting, PanelLayout, PanelNumberOverlay, PublishState, RepoState, RevidereState,
    SessionStats, ThemeSelection, UpdateFlow, ViewRestore, WtbarState,
};
mod types;
mod update;
mod view_state;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::config;
use crate::diff_state::DiffState;
use crate::explorer::ExplorerState;
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

/// すべてのUIパネルで共有されるトップレベルのアプリケーション状態。
pub struct App {
    /// 再描画が必要かどうか。
    pub needs_redraw: bool,
    /// 現在のパネルフォーカス。
    pub focus: Focus,
    /// フォーカスが直前にあったパネルと、移った時刻。ボーダー色のグライド
    /// アニメーション (animated_border_color) だけがこの 2 つを読む。
    pub focus_prev: Focus,
    /// フォーカスが最後に変わった時刻。境界線遷移のタイミング計測に使う。
    pub focus_changed_at: std::time::Instant,
    /// すべてのオーバーレイポップアップ状態（ブランチ切り替え、grab、prune、ヘルプなど）。
    pub overlays: OverlayManager,
    /// いま開いているリポジトリの同一性と、切り替え先の候補。
    pub repo: RepoState,
    /// 次のティックでアプリケーションを終了すべきかどうか。
    pub should_quit: bool,
    /// 有効時の埋め込みエディタパネル。Some ⟺ エディタのPTYが動作していて、
    /// マージされたExplorer+Viewer領域を占有している状態。None は通常の
    /// （エディタなしの）レイアウト。[App::open_in_editor] でセットされ、
    /// [App::exit_editor] で解体される（このフィールドと Focus::Editor を
    /// 対にする唯一の2つのメソッドであり、不変条件をこの中に閉じ込めている）。
    pub editor: Option<EditorPanel>,
    /// revidere の成果物と、実行中の解析。
    pub revidere: RevidereState,
    /// レビューコメントの GitHub 公開フロー (確認待ち + 実行中の処理)。
    pub publish: PublishState,
    /// 発見済みの worktree 一覧と、そこへの選択 (行の平坦化リストを含む)。
    pub worktrees: WorktreeList,
    /// 設定ファイルから読み込まれたアプリケーション設定。
    pub config: config::Config,
    /// 解決済みのキーバインドマップ（デフォルト + ユーザーによる上書き）。
    pub keymap: KeyMap,
    /// 描画に使う配色。フレームごとに読まれるので 1 階層浅いところに置く。
    /// 組み立ての元データは [Self::theme_sel]。
    pub theme: Theme,
    /// [Self::theme] を組み立てるための元データ (テーマ名 + ハイコントラスト)。
    pub theme_sel: ThemeSelection,
    /// Explorerパネルの状態（ファイルツリー + diff一覧/コメント一覧の選択）。
    pub explorer: ExplorerState,
    /// Viewerパネルの状態（開いているファイルのタブとその内容）。
    pub viewer: ViewerState,
    /// Diffデータの状態（Viewerのインラインハイライトに使われる）。
    pub diff_state: DiffState,
    /// SQLiteによるレビューコメントストア。DBを開けなかった場合は None。
    pub review_store: Option<ReviewStore>,
    /// レビューコメントのUI状態。
    pub review_state: ReviewState,
    /// ターミナル / PTYの状態。
    pub terminal: TerminalState,
    /// worktree管理の状態（作成、削除、スマートworktreeなど）。
    pub worktree_mgr: WorktreeManager,
    /// ステータスバーに表示されるステータスメッセージ（フラッシュメッセージ）。
    pub status_message: Option<StatusMessage>,
    /// 選択中worktreeの最後に確認したHEAD oid（変更検知ポーリング用）。
    pub last_poll_head_oid: Option<String>,
    /// 選択中worktreeの最後に確認したステータス署名（追加・変更・削除件数）。
    pub last_poll_status: Option<(usize, usize, usize, usize)>,

    /// syntect によるシンタックスハイライトの共有資源。
    pub highlight: Highlighting,
    /// レンダリング済みMarkdown（コメント/返信の本文）のID別キャッシュ。
    /// インラインスレッドボックスが毎フレーム再パース・再ハイライトしないため。
    pub markdown_cache: crate::ui::markdown::MarkdownCache,

    /// 現在100%に拡大されているパネル（[<=>]ボタン経由）。
    /// None はどのパネルも拡大されていない（デフォルトレイアウト）ことを表す。
    pub expanded_panel: Option<Focus>,

    /// パネルの幾何: レイアウト矩形のキャッシュ、ターミナル列の分割比、
    /// マウスによる境界リサイズ。
    pub layout: PanelLayout,

    /// Explorer の 2 つのリスト (ファイルツリー / Changed files) のホバー追跡。
    pub list_hover: ListHover,

    /// UIアニメーション用のフレームカウンタ（例: waiting状態のパルス）。
    pub ui_tick: u64,
    /// デコレーションアニメーション用の独立したティックカウンタ（一定間隔で増加）。
    pub decoration_tick: u64,

    /// セッション統計 (ゲーミフィケーション) と ccusage のキャッシュ。
    pub stats: SessionStats,
    /// worktreeのブランチごとのHEAD oid（コミット検知用）。
    pub worktree_heads: HashMap<String, String>,

    /// 自己更新フロー: 新バージョンの検出 → 確認 → インストール → 再起動。
    pub update: UpdateFlow,

    /// Ctrl+V貼り付けをサポートするためのシステムクリップボードコンテキスト。
    pub clipboard: Option<copypasta::ClipboardContext>,

    /// すべてのデコレーションモードのアニメーション状態。
    pub decoration_states: crate::worktree::decoration::DecorationStates,

    // ブランチ詳細 (worktree詳細パネル)
    /// 選択中worktreeの計算済みブランチ系譜とPR情報。
    pub branch_details: git_engine::BranchDetails,
    /// このシステムで gh CLIが利用可能かどうか。
    pub gh_available: bool,

    // Claudeセッションの自動再開
    /// 次のフレームで自動再開を実行すべきかどうか（一度きり）。
    pub pending_auto_resume: bool,

    /// 「ユーザーがどこを見ていたか」の保存と復元。
    pub view_restore: ViewRestore,

    /// 画面上端の worktree モニタストリップ (横スクロール位置 + 当たり判定)。
    pub wtbar: WtbarState,

    /// メニューバーの操作状態: どのメニューがフォーカス/オープン中か、
    /// 直近のバー/ドロップダウン描画で記録されたクリック領域。
    pub menu: crate::menu::MenuState,

    /// コードナビゲーション: シンボル索引、ジャンプ履歴、付随するポップアップ。
    pub code_nav: CodeNav,

    // バックグラウンド処理 (イベントループがポーリング)
    pub bg: BackgroundOps,

    // 新規worktreeバッジ
    /// 最近作成されたworktreeのパス（バッジ表示用）。選択時にクリアされる。
    pub new_worktree_paths: HashSet<PathBuf>,

    /// Alt+/ で出す、各パネル上の番号バッジ (2 秒で自動的に消える)。
    pub panel_number_overlay: PanelNumberOverlay,

    // リフロー・トランスクリプトビュー
    /// 無限スクロールバックモード中にClaude PTYパネルへオーバーレイされる、
    /// 読み取り専用・折り返し表示のセッションログビューアの状態。
    pub reflow: ReflowView,
}

/// configから有効なUIテーマ名を解決する。
///
/// 解決規則そのものは [config::Config::theme_name] が持つ。シンタックス
/// ハイライト側も同じ関数を通すので、UIとコードで別のテーマ名を見てしまう
/// ことはない。
fn resolve_theme_name(cfg: &config::Config) -> String {
    cfg.theme_name().to_string()
}

/// 名前から有効な [Theme] を組み立て、有効ならハイコントラスト変換を
/// 適用する。すべての呼び出し元（起動時、テーマピッカー、ライブリロード、
/// OSC11自動切り替え）がこのトグルを同一に扱うための唯一の構築ポイント。
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
        self.status_message = Some(StatusMessage::new(text, level, self.ui_tick));
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
