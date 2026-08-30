//! App の構築: config を読み込み、review store を開き、シンタックス
//! ハイライトの種を仕込み、以前選択していた worktree/view/grab の状態を復元する。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::config;
use crate::diff_state::{DiffState, DiffViewMode};
use crate::explorer::ExplorerState;
use crate::git_engine;
use crate::keymap::KeyMap;
use crate::overlay::OverlayManager;
use crate::review_state::ReviewState;
use crate::review_store::{self, ReviewStore};
use crate::viewer::ViewerState;
use crate::viewer::code_nav_state::CodeNav;
use crate::worktree::ops::WorktreeManager;

use super::state::{
    Highlighting, PanelLayout, RepoState, SessionStats, ThemeSelection, UpdateFlow,
};
use super::types::BackgroundOps;
use super::{App, GrabbedBranch, StatusLevel};
use crate::types::Focus;

impl App {
    /// 与えられたリポジトリパスを根とする新しい App を作成する。
    pub fn new(repo_path: PathBuf) -> Self {
        let config = config::Config::load().unwrap_or_default();
        // config が構造体へ move される前に、設定された terminal split を
        // スナップショットしておく。これが実行時に調整可能な
        // terminal_split_pct の初期値になる。
        let config_terminal_split_pct = config.layout.terminal_split_pct;
        let view_mode = DiffViewMode::from(config.diff.default_view);
        let diff_state = DiffState::new(&config.general.main_branch, view_mode);

        // review store のデータベースを開く。
        let db = review_store::db_path(&repo_path);
        let review_store = match ReviewStore::open(&db) {
            Ok(store) => Some(store),
            Err(e) => {
                log::warn!("failed to open review store: {e}");
                None
            }
        };

        // syntect のシンタックスセットとテーマを初期化する。
        let syntax_set = two_face::syntax::extra_newlines();
        let syntect_themes = two_face::theme::extra();
        let syntect_theme = config::syntect_theme_for(&config, &syntect_themes);
        let syntect_theme_id = config::syntax_theme_id(&config);

        // 既知リポジトリの一覧を作る: まず現在のリポジトリ、続けて config の追加分。
        let mut repo_list = vec![repo_path.clone()];
        for extra in &config.general.repos {
            if extra != &repo_path && !repo_list.contains(extra) {
                repo_list.push(extra.clone());
            }
        }

        // ゲーミフィケーション統計のセッションを初期化する。
        let stats_session_id = review_store
            .as_ref()
            .and_then(|store| store.start_stats_session().ok());
        if let Some(store) = &review_store {
            let _ = store.increment_daily_stat("sessions_used");
        }
        let today_stats = review_store
            .as_ref()
            .and_then(|store| store.get_today_stats().ok());

        let (keymap, keybind_warnings) = KeyMap::with_warnings(&config.keybinds);
        let theme_name = super::resolve_theme_name(&config);
        let high_contrast = config.ui.high_contrast;
        let theme = super::build_theme(&theme_name, high_contrast);
        let auto_resume = config.general.auto_resume;

        // メインリポジトリの表示名を、メイン worktree のパスから導出する。
        let main_repo_name = git_engine::GitEngine::open(&repo_path)
            .and_then(|engine| engine.main_worktree_path())
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| {
                repo_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| repo_path.display().to_string())
            });

        let active_scrollback = config.terminal.active_scrollback;
        let inactive_scrollback = config.terminal.inactive_scrollback;

        let mut app = Self {
            // 最初のフレームで全パネルを描画するための初期値。
            needs_redraw: true,
            focus: Focus::Explorer,
            focus_prev: Focus::Explorer,
            // 最初のフレームで枠線の遷移演出が再生されないよう時刻を過去にずらす。
            focus_changed_at: std::time::Instant::now()
                - std::time::Duration::from_millis(crate::anim::FOCUS_MS),
            overlays: OverlayManager::default(),
            // 索引の探索起点は repo_path。構造体に move される前に取る。
            code_nav: CodeNav::new(repo_path.clone()),
            repo: RepoState {
                path: repo_path,
                main_name: main_repo_name,
                known: repo_list,
                known_index: 0,
            },
            should_quit: false,
            editor: None,
            revidere: Default::default(),
            publish: Default::default(),
            worktrees: Default::default(),
            config,
            keymap,
            theme,
            theme_sel: ThemeSelection {
                name: theme_name,
                high_contrast,
            },
            explorer: ExplorerState::default(),
            viewer: ViewerState::default(),
            diff_state,
            review_store,
            review_state: ReviewState::new(),
            terminal: crate::terminal_state::TerminalState::new(
                active_scrollback,
                inactive_scrollback,
            ),
            worktree_mgr: WorktreeManager::default(),
            status_message: None,
            last_poll_head_oid: None,
            last_poll_status: None,
            highlight: Highlighting {
                syntax_set,
                themes: syntect_themes,
                theme: syntect_theme,
                theme_id: syntect_theme_id,
                generation: 0,
            },
            markdown_cache: crate::ui::markdown::MarkdownCache::new(),
            expanded_panel: None,
            layout: PanelLayout {
                terminal_split_pct: config_terminal_split_pct,
                ..Default::default()
            },
            list_hover: Default::default(),
            ui_tick: 0,
            decoration_tick: 0,
            stats: SessionStats {
                session_id: stats_session_id,
                today: today_stats,
                ccusage: None,
            },
            worktree_heads: HashMap::new(),
            update: UpdateFlow::from_current_process(),
            clipboard: copypasta::ClipboardContext::new().ok(),
            decoration_states: Default::default(),
            branch_details: Default::default(),
            gh_available: Self::check_gh_available(),
            pending_auto_resume: auto_resume,
            view_restore: Default::default(),
            wtbar: Default::default(),
            menu: Default::default(),
            bg: BackgroundOps::default(),
            new_worktree_paths: HashSet::new(),
            panel_number_overlay: Default::default(),
            reflow: crate::reflow::ReflowView::default(),
        };

        // キーバインド設定の問題を表に出す: TUI は stdout を隠してしまうので、
        // 黙って log::warn! するだけではカスタマイズが無視されたユーザに
        // 決して届かない。個々をログに残しつつ、起動時にまとめて1行表示する。
        if !keybind_warnings.is_empty() {
            for w in &keybind_warnings {
                log::warn!("keybind config: {w}");
            }
            let msg = match keybind_warnings.as_slice() {
                [one] => format!("Keybind config: {one}"),
                many => format!(
                    "Keybind config: {} issues ignored (see log; run with RUST_LOG=warn)",
                    many.len()
                ),
            };
            app.set_status(msg, StatusLevel::Warning);
        }

        app.refresh_worktrees();
        // 以前選択していた worktree とその開いていたファイル/スクロール位置を
        // 復元する。これにより(アップデート後などの)再起動でユーザが
        // 元いた場所に戻れる。
        app.restore_selected_worktree_and_view();
        app.refresh_reviews();
        // 復元した worktree について Explorer のファイルツリーと
        // 「変更されたファイル」diff をすぐに仕込んでおく。これをしないと、
        // 3秒ごとの worktree_poll の陳腐化チェック(または worktree バーの
        // クリック)が発火するまで最初のフレームで diff 一覧が空のままになり、
        // ユーザにはバーをクリックするまでパネルが「表示されない」ように
        // 見えてしまう。check_diff_viewer_staleness の
        // refresh_viewer + refresh_diff の組み合わせと同じ構図。
        app.refresh_viewer();
        app.refresh_diff();

        // $git_common_dir/wt-grab が存在すれば grab 状態を復元する。
        if let Ok(engine) = git_engine::GitEngine::open(&app.repo.path) {
            match engine.load_grab_state() {
                Ok(Some((branch, source_worktree, _stash_branch, claude_session_id))) => {
                    if source_worktree.exists() {
                        app.worktree_mgr.grabbed_branch = Some(GrabbedBranch {
                            branch,
                            source_worktree,
                            claude_session_id,
                        });
                        log::info!("Restored grab state from wt-grab file");
                    } else {
                        log::warn!(
                            "Stale wt-grab: source worktree '{}' no longer exists, cleaning up",
                            source_worktree.display()
                        );
                        if let Err(e) = engine.remove_grab_state() {
                            log::warn!("failed to remove stale wt-grab: {e}");
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("failed to load grab state: {e}");
                }
            }
        }

        app
    }
}
