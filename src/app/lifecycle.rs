//! `App` construction: loads config, opens the review store, seeds syntax
//! highlighting, and restores the previously selected worktree/view/grab state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use syntect::highlighting::ThemeSet;

use crate::background::BackgroundOp;
use crate::config;
use crate::diff_state::{DiffState, DiffViewMode};
use crate::git_engine;
use crate::jump_history::JumpHistory;
use crate::keymap::KeyMap;
use crate::overlay::{
    HoverInfoOverlay, OverlayManager, ReferencesOverlay, SymbolActionOverlay, SymbolHintOverlay,
};
use crate::review_state::ReviewState;
use crate::review_store::{self, ReviewStore};
use crate::symbol_index::SymbolIndex;
use crate::viewer::ViewerState;
use crate::worktree_ops::WorktreeManager;

use super::focus::Focus;
use super::types::{BackgroundOps, DirtyPanels};
use super::update::UpdateState;
use super::{App, GrabbedBranch, StatusLevel};

impl App {
    /// Create a new `App` rooted at the given repository path.
    pub fn new(repo_path: PathBuf) -> Self {
        let config = config::Config::load().unwrap_or_default();
        // Snapshot the configured terminal split before `config` is moved into
        // the struct; this seeds the runtime-adjustable `terminal_split_pct`.
        let config_terminal_split_pct = config.layout.terminal_split_pct;
        let view_mode = DiffViewMode::from(config.diff.default_view);
        let diff_state = DiffState::new(&config.general.main_branch, view_mode);

        // Open the review store database.
        let db = review_store::db_path(&repo_path);
        let review_store = match ReviewStore::open(&db) {
            Ok(store) => Some(store),
            Err(e) => {
                log::warn!("failed to open review store: {e}");
                None
            }
        };

        // Initialize syntect syntax set and theme.
        let syntax_set = two_face::syntax::extra_newlines();
        let ts = ThemeSet::load_defaults();
        let syntect_theme = config::syntect_theme_for(&config.viewer, &ts);

        // Build the list of known repositories: current repo first, then extras from config.
        let mut repo_list = vec![repo_path.clone()];
        for extra in &config.general.repos {
            if extra != &repo_path && !repo_list.contains(extra) {
                repo_list.push(extra.clone());
            }
        }

        // Initialize gamification stats session.
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

        // Derive the main repo display name from the main worktree path.
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
            dirty: DirtyPanels::all(),
            focus: Focus::Explorer,
            focus_prev: Focus::Explorer,
            // Backdate so no border transition plays on the first frame.
            focus_changed_at: std::time::Instant::now()
                - std::time::Duration::from_millis(crate::anim::FOCUS_MS),
            overlays: OverlayManager::default(),
            repo_path,
            main_repo_name,
            should_quit: false,
            editor: None,
            walkthrough_gens: Default::default(),
            current_walkthrough: None,
            publish_confirm: None,
            publish_op: BackgroundOp::default(),
            selected_worktree: 0,
            worktrees: Vec::new(),
            config,
            keymap,
            theme,
            theme_name,
            high_contrast,
            viewer_state: ViewerState::default(),
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
            repo_list,
            repo_list_index: 0,
            syntax_set,
            syntect_theme,
            markdown_cache: crate::ui::markdown::MarkdownCache::new(),
            expanded_panel: None,
            terminal_split_pct: config_terminal_split_pct,
            divider_drag: None,
            divider_hover: None,
            explorer_tree_hover: crate::ui::common::list_row::HoverRow::default(),
            diff_list_hover: crate::ui::common::list_row::HoverRow::default(),
            ui_tick: 0,
            decoration_tick: 0,
            notification_bar_badges: Vec::new(),
            worktree_list_rows: Vec::new(),
            worktree_list_selected: 0,
            stats_session_id,
            today_stats,
            worktree_heads: HashMap::new(),
            ccusage_info: None,
            update_info: None,
            update_check_requested: false,
            update_state: UpdateState::Idle,
            update_op: BackgroundOp::default(),
            update_progress_message: String::new(),
            startup_exe: std::env::current_exe().unwrap_or_default(),
            startup_args: std::env::args().skip(1).collect(),
            should_restart: false,
            update_badge_cols: None,
            clipboard: copypasta::ClipboardContext::new().ok(),
            decoration_states: Default::default(),
            branch_details: Default::default(),
            gh_available: Self::check_gh_available(),
            pending_auto_resume: auto_resume,
            current_view_branch: None,
            pending_view_restore: None,
            layout_cache: Default::default(),
            wtbar_hits: Vec::new(),
            wtbar_scroll: 0,
            wtbar_reveal_selected: false,
            wtbar_hover: None,
            menu: Default::default(),
            symbol_index: SymbolIndex::new(PathBuf::new()),
            jump_history: JumpHistory::new(),
            references_overlay: ReferencesOverlay::default(),
            symbol_hint_overlay: SymbolHintOverlay::default(),
            symbol_action_overlay: SymbolActionOverlay::default(),
            hover_info_overlay: HoverInfoOverlay::default(),
            bg: BackgroundOps::default(),
            new_worktree_paths: HashSet::new(),
            show_panel_number_overlay: false,
            panel_overlay_since: None,
            party_mode: false,
            rich_tier: crate::term_caps::RichTier::Off,
            rich_picker: None,
            rich_tier_available: crate::term_caps::RichTier::Off,
            rich_epoch: std::time::Instant::now(),
            reflow: super::reflow::ReflowView::default(),
        };
        app.symbol_index = SymbolIndex::new(app.repo_path.clone());

        // Surface keybind config problems: a TUI hides stdout, so a silent
        // log::warn! would never reach the user whose customizations were
        // dropped. Log each, flash one consolidated line on startup.
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
        // Restore the previously selected worktree + its open file/scroll so a
        // restart (e.g. after an update) lands the user where they left off.
        app.restore_selected_worktree_and_view();
        app.refresh_reviews();
        // Seed the Explorer's file tree and the "Changed files" diff for the
        // restored worktree right away. Without this the diff list stays empty
        // on the first frame and only fills in once the 3s `worktree_poll`
        // staleness check (or a worktree-bar click) fires — the panel appeared
        // to "not show up" until the user clicked the bar. Mirrors the
        // refresh_viewer + refresh_diff pairing in `check_diff_viewer_staleness`.
        app.refresh_viewer();
        app.refresh_diff();

        // Restore grab state from $git_common_dir/wt-grab if it exists.
        if let Ok(engine) = git_engine::GitEngine::open(&app.repo_path) {
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
