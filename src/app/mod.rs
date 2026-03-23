//! App state and focus management.
//!
//! This module defines the top-level application state, the unified panel
//! layout focus model, and transitions between panels.

mod terminal;
mod review;
mod worktree;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

use crate::background::BackgroundOp;

use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::config;
use crate::diff_state::{DiffState, DiffViewMode};
use crate::git_engine;
use crate::jump_history::JumpHistory;
use crate::overlay::{ReferencesOverlay, SymbolHintOverlay, SymbolActionOverlay};
use crate::symbol_index::SymbolIndex;
use crate::grep_search::GrepProgress;
use crate::keymap::KeyMap;
use crate::overlay::{ActiveOverlay, OverlayManager};
use crate::pty_manager;
use crate::review_state::ReviewState;
use crate::terminal_state::TerminalState;
use crate::review_store::{self, Author, CommentKind, ReviewStore};
use crate::worktree_ops::WorktreeManager;
use crate::theme::Theme;
use crate::viewer::ViewerState;

/// The severity/type of a status message, used for styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Success,
    Error,
    Warning,
    Info,
}

/// A status message with metadata for styled, timed display.
#[derive(Debug, Clone)]
pub struct StatusMessage {
    /// The text content of the message.
    pub text: String,
    /// The severity level (determines color and icon).
    pub level: StatusLevel,
    /// The `ui_tick` at which this message was created.
    pub created_at_tick: u64,
}

impl StatusMessage {
    pub fn new(text: String, level: StatusLevel, tick: u64) -> Self {
        Self { text, level, created_at_tick: tick }
    }

    /// Return the icon prefix for this message level.
    pub fn icon(&self) -> &'static str {
        match self.level {
            StatusLevel::Success => "\u{2713} ", // ✓
            StatusLevel::Error   => "\u{2717} ", // ✗
            StatusLevel::Warning => "\u{26A1} ", // ⚡
            StatusLevel::Info    => "\u{2139} ", // ℹ
        }
    }
}

impl From<String> for StatusMessage {
    fn from(text: String) -> Self {
        Self { text, level: StatusLevel::Info, created_at_tick: 0 }
    }
}

/// A row in the flattened worktree list (worktree headers + inline session rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeListRow {
    /// A worktree entry at `worktrees[idx]`.
    Worktree(usize),
    /// A Claude Code session under a worktree.
    Session { wt_idx: usize, pty_idx: usize },
}

/// Which panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Worktree,
    Explorer,
    Viewer,
    TerminalClaude,
    TerminalShell,
}

/// Input mode for worktree operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeInputMode {
    /// Normal navigation.
    Normal,
    /// Typing a new worktree branch name (step 1 of create).
    CreatingWorktree,
    /// Typing a base branch for the new worktree (step 2 of create).
    CreatingWorktreeBase,
    /// Confirming worktree deletion (y/n).
    ConfirmingDelete,
    /// Confirming branch deletion after worktree removal (y/n/f).
    #[allow(dead_code)]
    ConfirmingDeleteBranch,
    /// Confirming ungrab (y/n).
    ConfirmingUngrab,
    /// Smart Worktree: typing a multi-line task description.
    SmartDescription,
}

/// The kind of pending worktree background operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingWorktreeOp {
    Creating,
    Deleting,
    /// Smart worktree: LLM generation + worktree creation running in background.
    SmartCreating,
}

/// A worktree operation currently running in a background thread.
#[derive(Debug, Clone)]
pub struct PendingWorktree {
    pub branch: String,
    pub op: PendingWorktreeOp,
    pub base_ref: String,
    pub worktree_path: Option<PathBuf>,
    pub auto_spawn: bool,
    pub smart_prompt: String,
    pub delete_branch_after: bool,
    /// Task description for smart worktree (displayed while LLM is generating).
    pub description: String,
    /// When this pending entry was created (for timeout detection).
    pub created_at: std::time::Instant,
    /// Cancellation token: set to `true` to request cancellation of the background thread.
    pub cancel_token: Arc<AtomicBool>,
}

/// Result of a background worktree operation.
#[derive(Debug)]
#[allow(dead_code)]
pub enum WorktreeOpResult {
    Created { path: PathBuf, pending: PendingWorktree },
    CreateFailed { error: String, pending: PendingWorktree },
    Deleted { branch: String },
    DeleteFailed { error: String, branch: String },
    Skipped { branch: String, reason: String },
    /// Smart worktree: LLM resolved a branch name (for UI update).
    SmartBranchResolved { description: String, branch: String, prompt: String },
    /// Smart worktree: entire operation failed.
    SmartFailed { description: String, error: String },
}

/// Result from the smart worktree LLM generation.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SmartGenResult {
    pub branch: String,
    pub prompt: String,
}


/// Info about a grabbed branch (branch checkout swap with main).
#[derive(Debug, Clone)]
pub struct GrabbedBranch {
    /// The original branch name (e.g., "feature-x").
    pub branch: String,
    /// Path of the worktree that originally had this branch.
    pub source_worktree: PathBuf,
    /// Claude Code session ID from the source worktree (for resume after grab).
    pub claude_session_id: Option<String>,
}

/// State of the in-app update flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    /// Normal operation — no update in progress.
    Idle,
    /// Confirmation dialog is shown.
    Confirming,
    /// Download & build running in background thread.
    InProgress,
    /// About to restart the process.
    Restarting,
    /// An error occurred — message shown until dismissed.
    Failed,
}

/// Messages sent from the background update thread.
#[derive(Debug, Clone)]
pub enum UpdateProgress {
    /// Intermediate status message.
    Status(String),
    /// Update completed successfully.
    Done(String),
    /// Update failed with an error message.
    Error(String),
}

/// Tracks which UI panels need re-rendering.
#[derive(Default, Clone, Copy)]
pub struct DirtyPanels(u8);

impl DirtyPanels {
    pub const WORKTREE: u8 = 0b0000_0001;
    pub const EXPLORER: u8 = 0b0000_0010;
    #[allow(dead_code)]
    pub const VIEWER: u8   = 0b0000_0100;
    pub const TERMINAL: u8 = 0b0000_1000;
    pub const ALL: u8      = 0b0000_1111;

    pub fn mark(&mut self, bits: u8) { self.0 |= bits; }
    pub fn mark_all(&mut self) { self.0 = Self::ALL; }
    #[allow(dead_code)]
    pub fn is_dirty(&self, bits: u8) -> bool { self.0 & bits != 0 }
    pub fn any(&self) -> bool { self.0 != 0 }
    pub fn clear(&mut self) { self.0 = 0; }
}

/// Top-level application state shared across all UI panels.
pub struct App {
    /// Tracks which panels need re-rendering.
    pub dirty: DirtyPanels,
    /// Current panel focus.
    pub focus: Focus,
    /// All overlay popup states (switch-branch, grab, prune, help, etc.).
    pub overlays: OverlayManager,
    /// Working directory of the repository being inspected.
    pub repo_path: PathBuf,
    /// Display name of the main repository (directory name of the main worktree).
    pub main_repo_name: String,
    /// Whether the application should quit on the next tick.
    pub should_quit: bool,
    /// Index of the currently selected worktree in the worktree list.
    pub selected_worktree: usize,
    /// Cached list of worktrees discovered in the repository.
    pub worktrees: Vec<git_engine::WorktreeInfo>,
    /// Application configuration loaded from config file.
    pub config: config::Config,
    /// Resolved keybinding map (defaults + user overrides).
    pub keymap: KeyMap,
    /// UI color theme.
    pub theme: Theme,
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
    pub last_poll_status: Option<(usize, usize, usize)>,
    /// List of known repository paths (including the current one).
    pub repo_list: Vec<std::path::PathBuf>,
    /// Index of the currently active repository in repo_list.
    pub repo_list_index: usize,

    // ── Syntax highlighting (syntect) ──────────────────────────
    /// Shared syntect syntax definitions.
    pub syntax_set: SyntaxSet,
    /// Active syntect highlighting theme.
    pub syntect_theme: syntect::highlighting::Theme,

    /// Which panel is currently expanded to 100% (via the [<=>] button).
    /// `None` means no panel is expanded (default layout).
    pub expanded_panel: Option<Focus>,

    /// Frame counter for UI animations (e.g. waiting-state pulse).
    pub ui_tick: u64,
    /// Independent tick counter for decoration animation (incremented at fixed interval).
    pub decoration_tick: u64,

    /// Notification bar badge positions: (start_col, end_col, branch_name).
    /// Populated during rendering for click-to-jump.
    pub notification_bar_badges: Vec<(u16, u16, String)>,

    // ── Inline worktree+session list ────────────────────────────────
    /// Flattened list of worktree rows and inline session rows.
    pub worktree_list_rows: Vec<WorktreeListRow>,
    /// Selected index within `worktree_list_rows`.
    pub worktree_list_selected: usize,


    // ── Gamification (session stats + streak) ────────────────────
    /// ID of the current stats session (for gamification tracking).
    pub stats_session_id: Option<String>,
    /// Cached today's activity stats (refreshed periodically).
    pub today_stats: Option<review_store::DailyStats>,
    /// HEAD oid per worktree branch (for commit detection).
    pub worktree_heads: HashMap<String, String>,

    // ── ccusage (token/cost tracking) ────────────────────────────
    /// Cached ccusage info (refreshed periodically via background thread).
    pub ccusage_info: Option<CcusageInfo>,

    // ── Update check ───────────────────────────────────────────
    /// Latest release info when a newer version is available.
    pub update_info: Option<crate::update_checker::UpdateInfo>,
    /// Background update check operation.
    pub bg_update_check_op: BackgroundOp<Option<crate::update_checker::UpdateInfo>>,

    // ── ccusage background op ────────────────────────────────────
    /// Background ccusage fetch operation.
    pub bg_ccusage_op: BackgroundOp<CcusageInfo>,

    // ── Update & restart ──────────────────────────────────────
    /// Current state of the update flow.
    pub update_state: UpdateState,
    /// Background update operation.
    pub update_op: BackgroundOp<UpdateProgress>,
    /// Latest progress message to display in the overlay.
    pub update_progress_message: String,
    /// Path to the executable at startup (for exec-based restart).
    pub startup_exe: PathBuf,
    /// Command-line arguments at startup (for exec-based restart).
    pub startup_args: Vec<String>,
    /// Set to `true` when the update is done and the app should restart.
    pub should_restart: bool,
    /// Column range (start, end) of the update badge in the title bar.
    pub update_badge_cols: Option<(u16, u16)>,

    // ── Background fetch for switch-branch overlay ──────────────
    /// Background branch list fetch.
    pub bg_branch_op: BackgroundOp<Vec<String>>,

    // ── Background pull ────────────────────────────────────────
    /// Background pull operation.
    pub bg_pull_op: BackgroundOp<Result<String, String>>,


    /// System clipboard context for Ctrl+V paste support.
    pub clipboard: Option<copypasta::ClipboardContext>,

    /// Animation state for all decoration modes.
    pub decoration_states: crate::ui::decoration::DecorationStates,

    // ── Branch details (worktree detail panel) ────────────────────
    /// Computed branch lineage and PR info for the selected worktree.
    pub branch_details: git_engine::BranchDetails,
    /// Background `gh pr view` lookup.
    pub bg_pr_url_op: BackgroundOp<Option<String>>,
    /// Whether the `gh` CLI is available on this system.
    pub gh_available: bool,

    // ── Background worktree-switch operations ────────────────────
    /// Background diff computation.
    pub bg_diff_op: BackgroundOp<BgDiffResult>,
    /// Background file tree walk.
    pub bg_file_tree_op: BackgroundOp<Vec<crate::viewer::FileTreeEntry>>,
    /// Background branch details computation.
    pub bg_branch_details_op: BackgroundOp<git_engine::BranchDetails>,

    // ── Auto-resume Claude sessions ─────────────────────────────
    /// Whether auto-resume should run on the next frame (one-shot).
    pub pending_auto_resume: bool,

    /// Cached layout rectangles (recomputed when frame size or expansion state changes).
    pub layout_cache: crate::ui::layout::LayoutCache,

    // ── Code navigation (symbol index + jump history) ───────────
    pub symbol_index: SymbolIndex,
    pub jump_history: JumpHistory,
    pub references_overlay: ReferencesOverlay,
    pub symbol_hint_overlay: SymbolHintOverlay,
    pub symbol_action_overlay: SymbolActionOverlay,
    pub bg_symbol_index_op: BackgroundOp<Result<usize, String>>,

    // ── New worktree badge ──────────────────────────────────────
    /// Paths of worktrees recently created (for badge display). Cleared on selection.
    pub new_worktree_paths: HashSet<PathBuf>,

    // ── Panel number overlay (Alt key hold) ─────────────────────
    /// Whether to show the panel number overlay (true while Alt key is held).
    pub show_panel_overlay: bool,
}

/// Result of a background diff computation.
pub struct BgDiffResult {
    pub committed: Vec<crate::diff_state::FileDiff>,
    pub uncommitted: Vec<crate::diff_state::FileDiff>,
    pub error: Option<String>,
}

/// Aggregated token usage and cost from ccusage.
#[derive(Debug, Clone)]
pub struct CcusageInfo {
    pub total_tokens: u64,
    pub total_cost: f64,
}

impl App {
    /// Returns `true` when any overlay popup is visible on top of the main panels.
    ///
    /// Used by panel renderers to suppress cursor positioning so that the
    /// terminal cursor (and therefore the IME candidate window) appears at
    /// the overlay's input field, not at the underlying panel.
    pub fn is_any_overlay_active(&self) -> bool {
        self.overlays.active != ActiveOverlay::None
            || self.worktree_mgr.input_mode != WorktreeInputMode::Normal
            || self.review_state.input_mode != crate::review_state::ReviewInputMode::Normal
            || self.review_state.template_picker_active
            || self.review_state.comment_detail_active
            || self.update_state != UpdateState::Idle
            || self.worktree_mgr.skip_reason.is_some()
    }

    /// Create a new `App` rooted at the given repository path.
    pub fn new(repo_path: PathBuf) -> Self {
        let config = config::Config::load().unwrap_or_default();
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
        let syntect_theme = if let Some(ref path) = config.viewer.syntax_theme_file {
            match ThemeSet::get_theme(path) {
                Ok(theme) => theme,
                Err(e) => {
                    log::warn!("failed to load syntax theme file {path}: {e}; falling back to built-in theme");
                    let name = match config.viewer.theme.as_str() {
                        "catppuccin-mocha" => "base16-mocha.dark",
                        "dracula" => "base16-eighties.dark",
                        "nord" => "base16-ocean.dark",
                        "solarized-dark" => "Solarized (dark)",
                        _ => "base16-mocha.dark",
                    };
                    ts.themes.get(name).cloned().unwrap_or_else(|| ts.themes["base16-mocha.dark"].clone())
                }
            }
        } else {
            let syntect_theme_name = match config.viewer.theme.as_str() {
                "catppuccin-mocha" => "base16-mocha.dark",
                "dracula" => "base16-eighties.dark",
                "nord" => "base16-ocean.dark",
                "solarized-dark" => "Solarized (dark)",
                _ => "base16-mocha.dark",
            };
            ts.themes.get(syntect_theme_name).cloned().unwrap_or_else(|| ts.themes["base16-mocha.dark"].clone())
        };

        // Build the list of known repositories: current repo first, then extras from config.
        let mut repo_list = vec![repo_path.clone()];
        for extra in &config.general.repos {
            if extra != &repo_path && !repo_list.contains(extra) {
                repo_list.push(extra.clone());
            }
        }

        // Initialize gamification stats session.
        let stats_session_id = review_store.as_ref().and_then(|store| {
            store.start_stats_session().ok()
        });
        if let Some(store) = &review_store {
            let _ = store.increment_daily_stat("sessions_used");
        }
        let today_stats = review_store.as_ref().and_then(|store| store.get_today_stats().ok());

        let keymap = KeyMap::new(&config.keybinds);
        let theme = Theme::from_name(&config.viewer.theme);
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
            dirty: DirtyPanels(DirtyPanels::ALL),
            focus: Focus::Worktree,
            overlays: OverlayManager::default(),
            repo_path,
            main_repo_name,
            should_quit: false,
            selected_worktree: 0,
            worktrees: Vec::new(),
            config,
            keymap,
            theme,
            viewer_state: ViewerState::default(),
            diff_state,
            review_store,
            review_state: ReviewState::new(),
            terminal: TerminalState::new(active_scrollback, inactive_scrollback),
            worktree_mgr: WorktreeManager::default(),
            status_message: None,
            last_poll_head_oid: None,
            last_poll_status: None,
            repo_list,
            repo_list_index: 0,
            syntax_set,
            syntect_theme,
            expanded_panel: None,
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
            bg_update_check_op: BackgroundOp::default(),
            bg_ccusage_op: BackgroundOp::default(),
            update_state: UpdateState::Idle,
            update_op: BackgroundOp::default(),
            update_progress_message: String::new(),
            startup_exe: std::env::current_exe().unwrap_or_default(),
            startup_args: std::env::args().skip(1).collect(),
            should_restart: false,
            update_badge_cols: None,
            bg_branch_op: BackgroundOp::default(),
            bg_pull_op: BackgroundOp::default(),
            clipboard: copypasta::ClipboardContext::new().ok(),
            decoration_states: Default::default(),
            branch_details: Default::default(),
            bg_pr_url_op: BackgroundOp::default(),
            gh_available: Self::check_gh_available(),
            bg_diff_op: BackgroundOp::default(),
            bg_file_tree_op: BackgroundOp::default(),
            bg_branch_details_op: BackgroundOp::default(),
            pending_auto_resume: auto_resume,
            layout_cache: Default::default(),
            symbol_index: SymbolIndex::new(PathBuf::new()),
            jump_history: JumpHistory::new(),
            references_overlay: ReferencesOverlay::default(),
            symbol_hint_overlay: SymbolHintOverlay::default(),
            symbol_action_overlay: SymbolActionOverlay::default(),
            bg_symbol_index_op: BackgroundOp::default(),
            new_worktree_paths: HashSet::new(),
            show_panel_overlay: false,
        };
        app.symbol_index = SymbolIndex::new(app.repo_path.clone());
        app.refresh_worktrees();
        app.refresh_reviews();

        // Restore grab state from $git_common_dir/wt-grab if it exists.
        if let Ok(engine) = git_engine::GitEngine::open(&app.repo_path) {
            match engine.load_grab_state() {
                Ok(Some((branch, source_worktree, _stash_branch, claude_session_id))) => {
                    app.worktree_mgr.grabbed_branch = Some(GrabbedBranch {
                        branch,
                        source_worktree,
                        claude_session_id,
                    });
                    log::info!("Restored grab state from wt-grab file");
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("failed to load grab state: {e}");
                }
            }
        }

        app
    }

    /// Switch to a different repository by index in `repo_list`.
    pub fn switch_repo(&mut self, index: usize) {
        if index >= self.repo_list.len() {
            return;
        }
        self.repo_list_index = index;
        self.repo_path = self.repo_list[index].clone();

        // Re-open the review store for the new repo path.
        let db = review_store::db_path(&self.repo_path);
        self.review_store = match ReviewStore::open(&db) {
            Ok(store) => Some(store),
            Err(e) => {
                log::warn!("failed to open review store for new repo: {e}");
                None
            }
        };

        // Update main repo name for the new repository.
        self.main_repo_name = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|engine| engine.main_worktree_path())
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| {
                self.repo_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| self.repo_path.display().to_string())
            });

        // Refresh worktrees and reviews eagerly; viewer/diff will lazy-load.
        self.selected_worktree = 0;
        self.refresh_worktrees();
        self.viewer_state = ViewerState::default();
        self.diff_state = DiffState::new(&self.config.general.main_branch, self.diff_state.view_mode);
        self.refresh_reviews();
        self.terminal.active_claude_session = None;
        self.terminal.active_shell_session = None;

        self.set_status(format!("Switched to repository: {}", self.main_repo_name), StatusLevel::Success);
    }

    /// Open a repository from an arbitrary filesystem path.
    pub fn open_repo_from_path(&mut self, path: &str) {
        // Expand ~ to home directory.
        let expanded = if let Some(stripped) = path.strip_prefix('~') {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped.strip_prefix('/').unwrap_or(stripped))
            } else {
                std::path::PathBuf::from(path)
            }
        } else {
            std::path::PathBuf::from(path)
        };

        // Canonicalize if possible, otherwise use as-is.
        let canonical = expanded.canonicalize().unwrap_or(expanded);

        if !canonical.is_dir() {
            self.set_status(format!("Not a directory: {}", canonical.display()), StatusLevel::Error);
            return;
        }

        // Try to discover a git repository at this path.
        match git_engine::GitEngine::open(&canonical) {
            Ok(_engine) => {
                // Valid git repo — switch to it.
                self.repo_path = canonical.clone();

                // Re-open the review store for the new repo path.
                let db = review_store::db_path(&self.repo_path);
                self.review_store = match ReviewStore::open(&db) {
                    Ok(store) => Some(store),
                    Err(e) => {
                        log::warn!("failed to open review store for new repo: {e}");
                        None
                    }
                };

                self.selected_worktree = 0;
                self.refresh_worktrees();
                self.viewer_state = ViewerState::default();
                self.diff_state = DiffState::new(&self.config.general.main_branch, self.diff_state.view_mode);
                self.refresh_reviews();
                self.terminal.active_claude_session = None;
                self.terminal.active_shell_session = None;

                // Add to repo_list if not already present.
                if !self.repo_list.contains(&canonical) {
                    self.repo_list.push(canonical.clone());
                }
                // Update repo_list_index to point to this repo.
                self.repo_list_index = self
                    .repo_list
                    .iter()
                    .position(|p| p == &canonical)
                    .unwrap_or(0);

                let repo_name = canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| canonical.display().to_string());
                self.set_status(format!("Opened repository: {repo_name}"), StatusLevel::Success);
            }
            Err(e) => {
                self.set_status(format!("Not a git repository: {} ({e})", canonical.display()), StatusLevel::Error);
            }
        }
    }

    /// Refresh the cached worktree list from the repository.
    ///
    /// Returns `true` if the worktree list actually changed (different count,
    /// branch names, or status counts), so callers can skip redraws when
    /// nothing is different.
    pub fn refresh_worktrees(&mut self) -> bool {
        let mut changed = false;
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_worktrees() {
                    Ok(worktrees) => {
                        // Detect whether the worktree list changed before replacing it.
                        if worktrees.len() != self.worktrees.len() {
                            changed = true;
                        } else {
                            for (old, new) in self.worktrees.iter().zip(worktrees.iter()) {
                                if old.branch != new.branch
                                    || old.added != new.added
                                    || old.modified != new.modified
                                    || old.deleted != new.deleted
                                    || old.is_clean != new.is_clean
                                {
                                    changed = true;
                                    break;
                                }
                            }
                        }
                        self.worktrees = worktrees;
                        if !self.worktrees.is_empty() && self.selected_worktree >= self.worktrees.len()
                        {
                            self.selected_worktree = self.worktrees.len() - 1;
                        }
                        // Detect commits by HEAD oid changes.
                        for wt in &self.worktrees {
                            if let Ok(wt_engine) = git_engine::GitEngine::open(&wt.path) {
                                if let Ok(head_oid) = wt_engine.head_oid_string() {
                                    if let Some(old) = self.worktree_heads.get(&wt.branch) {
                                        if old != &head_oid {
                                            self.record_stat("commits_made");
                                            changed = true;
                                        }
                                    }
                                    self.worktree_heads.insert(wt.branch.clone(), head_oid);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("failed to list worktrees: {e}");
                    }
                }
                // Refresh local branches for the detail zone.
                if let Ok(branches) = engine.list_local_branches() {
                    if branches != self.worktree_mgr.local_branches {
                        changed = true;
                    }
                    self.worktree_mgr.local_branches = branches;
                }
            }
            Err(e) => {
                log::warn!("failed to open git repository: {e}");
            }
        }
        self.rebuild_worktree_list_rows();
        changed
    }

    /// Advance the decoration animation by one tick. Returns `true` when
    /// an animation was actually updated (i.e. mode is not `None`).
    pub fn tick_decoration(&mut self, width: u16, height: u16) -> bool {
        use crate::ui::decoration::{DecorationActivity, DecorationMode};
        let mode = DecorationMode::from_str(&self.config.general.decoration);
        if !mode.has_animation() {
            return false;
        }
        self.decoration_tick = self.decoration_tick.wrapping_add(1);
        let activity = if self.terminal.cc_waiting_worktrees.is_empty() {
            DecorationActivity::Calm
        } else {
            DecorationActivity::Active
        };
        crate::ui::decoration::tick_decoration(
            &mut self.decoration_states,
            self.decoration_tick,
            width,
            height,
            activity,
            mode,
        );
        true
    }

    /// Reload the viewer file tree for the currently selected worktree.
    ///
    /// Preserves the currently open file and scroll position so that
    /// file-watcher refreshes don't disrupt the user's view.
    pub fn refresh_viewer(&mut self) {
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let path = wt.path.clone();
            let tab_width = self.config.viewer.tab_width;
            self.viewer_state.load_file_tree(&path, tab_width);
            self.rehighlight_viewer();
        }
    }

    /// Run syntect highlighting on the currently loaded file content.
    pub fn rehighlight_viewer(&mut self) {
        // Use disjoint field borrows to satisfy the borrow checker.
        let syntax_set = &self.syntax_set;
        let theme = &self.syntect_theme;
        self.viewer_state.highlight_content(syntax_set, theme);
    }

    /// Load (or reload) the diff for the currently selected worktree
    /// against the configured main branch.
    pub fn refresh_diff(&mut self) {
        let base_branch = self.config.general.main_branch.clone();
        let word_diff = self.config.diff.word_diff;
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let path = wt.path.clone();
            let tab_width = self.config.viewer.tab_width;
            self.diff_state.load_diff(&path, &base_branch, word_diff, tab_width);
            self.viewer_state.invalidate_diff_annotations();
        }
    }

    /// Set focus to a panel, lazily loading data when first needed.
    pub fn set_focus(&mut self, focus: Focus) {
        // Collapse expanded panel when focus moves to a panel that would have zero width.
        if let Some(expanded) = self.expanded_panel {
            let dominated = match expanded {
                Focus::TerminalClaude | Focus::TerminalShell => {
                    matches!(focus, Focus::TerminalClaude | Focus::TerminalShell)
                }
                other => other == focus,
            };
            if !dominated {
                self.expanded_panel = None;
            }
        }
        match focus {
            Focus::Explorer | Focus::Viewer => {
                if self.viewer_state.tree.file_tree.is_empty() {
                    self.refresh_viewer();
                }
                if self.diff_state.committed_files.is_empty() && self.diff_state.uncommitted_files.is_empty() {
                    self.refresh_diff();
                }
            }
            Focus::TerminalClaude => {
                // Clear CC waiting signal when user focuses on the terminal panel,
                // not just when they actually type into it.
                if let Some(idx) = self.terminal.active_claude_session {
                    self.clear_cc_waiting_signal(idx);
                }
            }
            _ => {}
        }
        self.focus = focus;
    }

    /// Request the application to quit.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Return a help text string describing the keybindings for the current focus.
    pub fn status_bar_text(&self) -> &'static str {
        #[cfg(target_os = "macos")]
        {
            match self.focus {
                Focus::Worktree => "Cmd+1-5: jump | Tab: next | q: quit | j/k: nav | w/W: new/del | s: switch | g: grab | G: ungrab | P: prune",
                Focus::Explorer => "Cmd+1-5: jump | Tab: next panel | j/k: navigate | Enter: open file | h/l: collapse/expand | d: diff list",
                Focus::Viewer => "Cmd+1-5: jump | Tab: next panel | Esc: back to explorer | j/k: scroll | /: search | c: comment",
                Focus::TerminalClaude => "Cmd+1-5: jump | Alt+h/l: panel | Ctrl+n: new CC | Ctrl+p: palette | keys → PTY",
                Focus::TerminalShell => "Cmd+1-5: jump | Alt+h/l: panel | Ctrl+t: new shell | keys → PTY",
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.focus {
                Focus::Worktree => "Cmd+1-5: jump | Tab: next | q: quit | j/k: nav | w/W: new/del | s: switch | g: grab | G: ungrab | P: prune",
                Focus::Explorer => "Cmd+1-5: jump | Tab: next panel | j/k: navigate | Enter: open file | h/l: collapse/expand | d: diff list",
                Focus::Viewer => "Cmd+1-5: jump | Tab: next panel | Esc: back to explorer | j/k: scroll | /: search | c: comment",
                Focus::TerminalClaude => "Cmd+1-5: jump | Alt+h/l: panel | Ctrl+n: new CC | Ctrl+p: palette | keys → PTY",
                Focus::TerminalShell => "Cmd+1-5: jump | Alt+h/l: panel | Ctrl+t: new shell | keys → PTY",
            }
        }
    }

    /// Set a styled status message.
    pub fn set_status(&mut self, text: String, level: StatusLevel) {
        self.status_message = Some(StatusMessage::new(text, level, self.ui_tick));
    }

    /// Set a plain info status message (backward-compatible shorthand).
    pub fn set_status_info(&mut self, text: String) {
        self.set_status(text, StatusLevel::Info);
    }

    // ── Code navigation helpers ────────────────────────────────────

    /// Extract the symbol under the cursor from the current viewer line.
    pub fn get_symbol_at_cursor(&self) -> Option<String> {
        let scroll = self.viewer_state.content.file_scroll;
        let line = self.viewer_state.content.file_content.get(scroll)?;
        extract_symbol_from_line(line)
    }

    /// Check if the cursor is currently at (or very near) a definition site
    /// for the given symbol. Returns `true` when the current file + line
    /// matches one of the symbol's definition locations.
    pub fn is_cursor_at_definition(&self, symbol: &str) -> bool {
        let cur_file = match &self.viewer_state.content.current_file {
            Some(f) => f,
            None => return false,
        };
        // Cursor line is 1-indexed (file_scroll is 0-indexed).
        let cursor_line = self.viewer_state.content.file_scroll + 1;
        let defs = self.symbol_index.find_definitions(symbol);
        defs.iter().any(|d| {
            d.file_path == *cur_file
                && (d.line as isize - cursor_line as isize).unsigned_abs() <= 2
        })
    }

    /// Jump to a file location, pushing the current position onto the history.
    ///
    /// `source_screen_row` is the screen row (0-indexed) where the source
    /// symbol was displayed. The target line will be placed at the same row
    /// so the user's eye position is preserved.
    pub fn jump_to_location(&mut self, file_path: &str, line: usize, source_screen_row: usize) {
        // Skip self-referencing jumps (destination == current position).
        let target_line_0 = line.saturating_sub(1);
        if let Some(ref cur_file) = self.viewer_state.content.current_file {
            let current_line_0 = self.viewer_state.content.file_scroll + source_screen_row;
            if cur_file == file_path && current_line_0 == target_line_0 {
                return;
            }
        }

        // Save current location to history.
        if let Some(ref cur_file) = self.viewer_state.content.current_file.clone() {
            let loc = crate::jump_history::Location {
                file_path: cur_file.clone(),
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            };
            self.jump_history.push(loc);
        }

        // Open the target file.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let wt_path = wt.path.clone();
            let tab_width = self.config.viewer.tab_width;
            self.viewer_state.open_file(&wt_path, file_path, tab_width);
            self.rehighlight_viewer();
            self.viewer_state.reveal_file_in_tree(file_path, &wt_path);
        }

        // Scroll so the target line appears at the same screen row as the source symbol.
        let target_0 = line.saturating_sub(1);
        let total = self.viewer_state.content.file_content.len();
        let scroll = target_0.saturating_sub(source_screen_row).min(total.saturating_sub(1));
        self.viewer_state.content.file_scroll = scroll;
        self.viewer_state.content.h_scroll = 0;
        self.set_focus(Focus::Viewer);
    }

    /// Navigate back in the jump history.
    pub fn jump_back(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.jump_history.go_back(current) {
            if let Some(wt) = self.worktrees.get(self.selected_worktree) {
                let wt_path = wt.path.clone();
                let tab_width = self.config.viewer.tab_width;
                self.viewer_state.open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state.reveal_file_in_tree(&loc.file_path, &wt_path);
            }
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
        }
    }

    /// Navigate forward in the jump history.
    pub fn jump_forward(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.jump_history.go_forward(current) {
            if let Some(wt) = self.worktrees.get(self.selected_worktree) {
                let wt_path = wt.path.clone();
                let tab_width = self.config.viewer.tab_width;
                self.viewer_state.open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state.reveal_file_in_tree(&loc.file_path, &wt_path);
            }
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
        }
    }

    /// Start building the symbol index in the background.
    pub fn start_symbol_index_build(&mut self) {
        let index = self.symbol_index.clone();
        self.bg_symbol_index_op.start(move |tx| {
            let result = match index.build() {
                Ok(count) => Ok(count),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
    }

    /// Check whether a symbol has definitions in the symbol index.
    pub fn can_jump_to_symbol(&self, name: &str) -> bool {
        if !self.symbol_index.is_available() {
            return false;
        }
        !self.symbol_index.find_definitions(name).is_empty()
    }

    /// Build symbol hints for visible lines in the viewer.
    /// Returns hints with 2-character labels for jumpable symbols on screen.
    pub fn build_symbol_hints(&self, inner_height: usize) -> Vec<crate::overlay::SymbolHint> {
        let scroll = self.viewer_state.content.file_scroll;
        let total = self.viewer_state.content.file_content.len();
        let end = (scroll + inner_height).min(total);

        let re = match regex::Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\b") {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut seen = std::collections::HashSet::new();
        let mut candidates = Vec::new();

        for line_idx in scroll..end {
            let line = &self.viewer_state.content.file_content[line_idx];
            let line_1 = line_idx + 1;
            for cap in re.captures_iter(line) {
                if let Some(m) = cap.get(1) {
                    let word = m.as_str();
                    if word.len() <= 1 || is_rust_keyword(word) {
                        continue;
                    }
                    if !seen.insert(word.to_string()) {
                        continue;
                    }
                    if !self.can_jump_to_symbol(word) {
                        continue;
                    }
                    candidates.push((word.to_string(), line_1, m.start(), m.end()));
                }
            }
        }

        // Assign 2-character labels: aa, ab, ..., az, ba, bb, ...
        candidates
            .into_iter()
            .enumerate()
            .map(|(i, (name, line, start, end))| {
                let first = (b'a' + (i / 26) as u8) as char;
                let second = (b'a' + (i % 26) as u8) as char;
                crate::overlay::SymbolHint {
                    label: format!("{first}{second}"),
                    symbol_name: name,
                    line,
                    start_col: start,
                    end_col: end,
                }
            })
            .collect()
    }

    /// Open a file path (relative to the current worktree) in the Viewer panel.
    ///
    /// Optionally jumps to `line` (1-indexed). Reveals the file in the explorer
    /// tree, switches focus to Viewer, and shows a status message.
    pub fn open_file_in_viewer(&mut self, relative_path: &str, line: Option<usize>) {
        let wt_path = self.selected_worktree_path();
        let tab_width = self.config.viewer.tab_width;

        self.viewer_state.open_file(&wt_path, relative_path, tab_width);
        self.viewer_state.reveal_file_in_tree(relative_path, &wt_path);

        if let Some(ln) = line {
            let max = self.viewer_state.content.file_content.len().saturating_sub(1);
            self.viewer_state.content.file_scroll = (ln.saturating_sub(1)).min(max);
        }

        self.set_focus(Focus::Viewer);

        let msg = if let Some(ln) = line {
            format!("Opened {relative_path}:{ln} in Viewer")
        } else {
            format!("Opened {relative_path} in Viewer")
        };
        self.set_status(msg, StatusLevel::Success);
    }

    /// Execute a command selected from the command palette.
    pub fn execute_palette_command(&mut self, id: crate::command_palette::CommandId) {
        use crate::command_palette::CommandId;
        match id {
            // Navigation
            CommandId::FocusWorktree => self.set_focus(Focus::Worktree),
            CommandId::FocusExplorer => self.set_focus(Focus::Explorer),
            CommandId::FocusViewer => self.set_focus(Focus::Viewer),
            CommandId::FocusTerminalClaude => self.set_focus(Focus::TerminalClaude),
            CommandId::FocusTerminalShell => self.set_focus(Focus::TerminalShell),
            CommandId::TogglePanelExpand => self.cmd_toggle_panel_expand(),
            CommandId::CreateWorktree => self.cmd_create_worktree(),
            CommandId::DeleteWorktree => self.cmd_delete_worktree(),
            CommandId::SwitchBranch => self.cmd_switch_branch(),
            CommandId::GrabBranch => self.cmd_grab_branch(),
            CommandId::PruneWorktrees => self.cmd_prune_worktrees(),
            CommandId::MergeToMain => self.cmd_merge_to_main(),
            CommandId::RefreshWorktrees => { let _ = self.refresh_worktrees(); }
            CommandId::ResetMainToOrigin => self.cmd_reset_main_to_origin(),
            CommandId::CherryPick => self.cmd_cherry_pick(),
            CommandId::NewClaudeCode => self.cmd_new_claude_code(),
            CommandId::NewShell => self.cmd_new_shell(),
            CommandId::ResumeClaudeSession => self.cmd_resume_claude_session(),
            CommandId::RefreshDiff => self.refresh_diff(),
            CommandId::SearchInFile => self.cmd_search_in_file(),
            CommandId::ToggleHelp => self.cmd_toggle_help(),
            CommandId::ShowReviewComments => self.cmd_show_review_comments(),
            CommandId::ShowReviewTemplates => { self.review_state.template_picker_active = true; }
            CommandId::SessionHistory => self.cmd_session_history(),
            CommandId::OpenRepo => self.cmd_open_repo(),
            CommandId::SwitchRepo => self.cmd_switch_repo(),
            CommandId::UngrabBranch => self.cmd_ungrab_branch(),
            CommandId::ShowDiffList => self.cmd_show_diff_list(),
            CommandId::ShowCommentList => self.cmd_show_comment_list(),
            CommandId::AddReviewComment => self.cmd_add_review_comment(),
            CommandId::ViewCommentDetail => self.cmd_view_comment_detail(),
            CommandId::DeleteComment => self.cmd_delete_comment(),
            CommandId::ToggleCommentResolve => self.cmd_toggle_comment_resolve(),
            CommandId::EditComment => self.cmd_edit_comment(),
            CommandId::ReplyToComment => self.cmd_reply_to_comment(),
            CommandId::SaveSessionHistory => self.save_current_session_history(),
            CommandId::OpenPullRequest => self.open_pr_in_browser(),
            CommandId::UpdateAndRestart => self.cmd_update_and_restart(),
            CommandId::SearchFullText => self.cmd_search_full_text(),
            CommandId::Quit => self.should_quit = true,
        }
    }

    // ── Command palette handler methods ──────────────────────────────

    fn cmd_toggle_panel_expand(&mut self) {
        if self.expanded_panel == Some(self.focus) {
            self.expanded_panel = None;
        } else {
            self.expanded_panel = Some(self.focus);
        }
    }

    fn cmd_create_worktree(&mut self) {
        self.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
        self.worktree_mgr.input_buffer.clear();
        self.set_status_info("New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string());
    }

    fn cmd_delete_worktree(&mut self) {
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            if wt.is_main {
                self.set_status("Cannot delete the main worktree.".to_string(), StatusLevel::Warning);
            } else {
                let branch = wt.branch.clone();
                self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingDelete;
                self.set_status_info(format!("Delete worktree '{branch}'? (y/n)"));
            }
        }
    }

    fn cmd_switch_branch(&mut self) {
        self.set_status_info("Loading branches...".to_string());
        self.load_switch_branches();
        if !self.overlays.switch_branch.branches.is_empty() {
            self.overlays.active = ActiveOverlay::SwitchBranch;
            self.status_message = None;
        }
    }

    fn cmd_grab_branch(&mut self) {
        if self.worktree_mgr.grabbed_branch.is_some() {
            self.set_status("Already grabbing a branch. Ungrab first (Y).".to_string(), StatusLevel::Warning);
        } else {
            self.load_grab_branches();
            if self.overlays.grab.branches.is_empty() {
                self.set_status_info("No non-main worktrees to grab.".to_string());
            } else {
                self.overlays.active = ActiveOverlay::Grab;
            }
        }
    }

    fn cmd_prune_worktrees(&mut self) {
        match crate::git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => match engine.find_stale_worktrees() {
                Ok(stale) => {
                    if stale.is_empty() {
                        self.set_status_info("No stale worktrees found.".to_string());
                    } else {
                        self.overlays.prune.stale = stale;
                        self.overlays.active = ActiveOverlay::Prune;
                    }
                }
                Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
            },
            Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
        }
    }

    fn cmd_merge_to_main(&mut self) {
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            if wt.is_main {
                self.set_status("Cannot merge main into itself.".to_string(), StatusLevel::Warning);
            } else {
                let branch = wt.branch.clone();
                let main_branch = self.config.general.main_branch.clone();
                match crate::git_engine::GitEngine::open(&self.repo_path) {
                    Ok(engine) => match engine.merge_into_main(&branch, &main_branch) {
                        Ok(msg) => {
                            self.set_status(msg, StatusLevel::Success);
                            self.refresh_worktrees();
                        }
                        Err(e) => self.set_status(format!("Merge error: {e}"), StatusLevel::Error),
                    },
                    Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
                }
            }
        }
    }

    fn cmd_reset_main_to_origin(&mut self) {
        let main_branch = self.config.general.main_branch.clone();
        match crate::git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => match engine.reset_main_to_origin(&main_branch) {
                Ok(msg) => {
                    self.set_status(msg, StatusLevel::Success);
                    self.refresh_worktrees();
                }
                Err(e) => self.set_status(format!("Reset error: {e}"), StatusLevel::Error),
            },
            Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
        }
    }

    fn cmd_cherry_pick(&mut self) {
        let current_branch = self.selected_worktree_branch();
        let source = self.worktrees.iter()
            .find(|w| w.branch != current_branch)
            .map(|w| w.branch.clone());
        if let Some(branch) = source {
            self.overlays.cherry_pick.source_branch = branch;
            self.load_cherry_pick_commits();
            self.overlays.active = ActiveOverlay::CherryPick;
        } else {
            self.set_status_info("No other worktree branches available.".to_string());
        }
    }

    fn cmd_new_claude_code(&mut self) {
        if let Err(e) = self.spawn_claude_code() {
            self.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
        }
        self.set_focus(Focus::TerminalClaude);
    }

    fn cmd_new_shell(&mut self) {
        if let Err(e) = self.spawn_shell() {
            self.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
        }
        self.set_focus(Focus::TerminalShell);
    }

    fn cmd_resume_claude_session(&mut self) {
        self.overlays.active = ActiveOverlay::ResumeSession;
        self.load_resume_sessions();
    }

    fn cmd_search_in_file(&mut self) {
        self.viewer_state.search.search_active = true;
        self.viewer_state.search.search_query.clear();
        self.set_focus(Focus::Viewer);
    }

    fn cmd_toggle_help(&mut self) {
        self.overlays.help.context = self.focus;
        self.overlays.active = ActiveOverlay::Help;
    }

    fn cmd_show_review_comments(&mut self) {
        self.viewer_state.explorer.explorer_show_comments = true;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    fn cmd_session_history(&mut self) {
        self.overlays.active = ActiveOverlay::History;
        self.load_session_history();
    }

    fn cmd_open_repo(&mut self) {
        self.overlays.active = ActiveOverlay::OpenRepo;
        self.overlays.open_repo.buffer.set_text(&self.repo_path.display().to_string());
    }

    fn cmd_switch_repo(&mut self) {
        if self.repo_list.len() > 1 {
            self.overlays.active = ActiveOverlay::RepoSelector;
            self.overlays.repo_selector.selected = self.repo_list_index;
        }
    }

    fn cmd_ungrab_branch(&mut self) {
        if self.worktree_mgr.grabbed_branch.is_none() {
            self.set_status("Not grabbing — nothing to ungrab.".to_string(), StatusLevel::Warning);
        } else {
            self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingUngrab;
            self.set_status("Ungrab? Main will return to main branch. (y/n)".to_string(), StatusLevel::Warning);
        }
    }

    fn cmd_show_diff_list(&mut self) {
        self.viewer_state.explorer.explorer_show_comments = false;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    fn cmd_show_comment_list(&mut self) {
        self.viewer_state.explorer.explorer_show_comments = true;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    fn cmd_add_review_comment(&mut self) {
        if let Some(file_path) = self.viewer_state.content.current_file.clone() {
            let location = if let Some((start, end)) = self.viewer_state.selected_range() {
                if start == end {
                    format!("{file_path}:{start} ")
                } else {
                    format!("{file_path}:{start}-{end} ")
                }
            } else {
                let line = self.viewer_state.content.file_scroll + 1;
                format!("{file_path}:{line} ")
            };
            self.viewer_state.clear_selection();
            self.review_state.input_buffer.set_text(&location);
            self.review_state.input_kind = crate::review_store::CommentKind::Suggest;
            self.review_state.input_mode = crate::review_state::ReviewInputMode::AddingComment;
            self.review_state.status_message =
                Some("Add comment: [s:|q:]file:line body".to_string());
            self.set_focus(Focus::Viewer);
        } else {
            self.set_status("No file open in viewer.".to_string(), StatusLevel::Warning);
        }
    }

    fn cmd_view_comment_detail(&mut self) {
        // Try viewer context first (current line), then comment list context.
        if self.viewer_state.content.current_file.is_some() {
            let cursor_line = if let Some((start, _)) = self.viewer_state.selected_range() {
                start
            } else {
                self.viewer_state.content.file_scroll + 1
            };
            if let Some(comments) = self.review_state.file_comments.get(&cursor_line) {
                if !comments.is_empty() {
                    let target_id = &comments[0].id;
                    if let Some(idx) = self.review_state.comments.iter().position(|c| c.id == *target_id) {
                        let cid = target_id.clone();
                        if !self.review_state.cached_replies.contains_key(&cid) {
                            if let Some(store) = self.review_store.as_ref() {
                                if let Ok(replies) = store.get_replies(&cid) {
                                    self.review_state.cached_replies.insert(cid, replies);
                                }
                            }
                        }
                        self.review_state.comment_detail_idx = idx;
                        self.review_state.comment_detail_scroll = 0;
                        self.review_state.comment_detail_active = true;
                        self.set_focus(Focus::Viewer);
                        return;
                    }
                }
            }
        }
        self.set_status("No comment on current line.".to_string(), StatusLevel::Warning);
    }

    fn cmd_delete_comment(&mut self) {
        if self.viewer_state.explorer.explorer_show_comments
            && self.viewer_state.explorer.explorer_focus_on_diff_list
            && !self.review_state.comment_list_rows.is_empty()
        {
            self.delete_selected_review_comment();
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }

    fn cmd_toggle_comment_resolve(&mut self) {
        if self.viewer_state.explorer.explorer_show_comments
            && self.viewer_state.explorer.explorer_focus_on_diff_list
            && !self.review_state.comment_list_rows.is_empty()
        {
            self.toggle_selected_review_status();
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }

    fn cmd_edit_comment(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.explorer.comment_list_selected);
        if let Some(comment) = comment_idx.and_then(|idx| self.review_state.comments.get(idx)) {
            self.review_state.input_buffer.set_text(&comment.body);
            self.review_state.input_mode = crate::review_state::ReviewInputMode::EditingComment;
            self.review_state.selected = comment_idx.unwrap();
            self.review_state.status_message =
                Some("Edit comment (Enter to save, Esc to cancel)".to_string());
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }

    fn cmd_reply_to_comment(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.explorer.comment_list_selected);
        if let Some(idx) = comment_idx {
            self.review_state.input_buffer.clear();
            self.review_state.input_mode = crate::review_state::ReviewInputMode::ReplyingToComment;
            self.review_state.selected = idx;
            self.review_state.status_message =
                Some("Reply to comment (Enter to send, Esc to cancel)".to_string());
        } else {
            self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
        }
    }

    fn cmd_update_and_restart(&mut self) {
        if self.update_info.is_some() {
            self.start_update_confirm();
        } else {
            self.set_status("No update available.".to_string(), StatusLevel::Info);
        }
    }

    fn cmd_search_full_text(&mut self) {
        self.overlays.active = ActiveOverlay::GrepSearch;
        self.overlays.grep_search.query.clear();
        self.overlays.grep_search.result_tree = Default::default();
        self.overlays.grep_search.pending_matches.clear();
        self.overlays.grep_search.selected = 0;
        self.overlays.grep_search.scroll = 0;
        self.overlays.grep_search.running = false;
        self.overlays.grep_search.bg_op.clear();
        self.overlays.grep_search.bg_op_phase2.clear();
        self.overlays.grep_search.debounce_deadline = None;
        self.overlays.grep_search.phase1_active = false;
    }

    /// Show the update confirmation dialog.
    pub fn start_update_confirm(&mut self) {
        self.update_state = UpdateState::Confirming;
    }

    /// Kick off the background update thread.
    pub fn start_update_download(&mut self) {
        let Some(ref info) = self.update_info else { return };
        let version = info.latest_version.clone();
        let tarball_url = info.tarball_url.clone();
        let assets = info.assets.clone();

        self.update_state = UpdateState::InProgress;
        self.update_progress_message = "Preparing update...".to_string();

        self.update_op.start(move |tx| {
            perform_update(&tx, &version, &tarball_url, &assets);
        });
    }

    /// Poll for progress messages from the background update thread.
    pub fn poll_update_progress(&mut self) {
        for msg in self.update_op.poll_all() {
            match msg {
                UpdateProgress::Status(s) => {
                    self.update_progress_message = s;
                }
                UpdateProgress::Done(s) => {
                    self.update_progress_message = s;
                    self.update_state = UpdateState::Restarting;
                    self.should_restart = true;
                    self.should_quit = true;
                }
                UpdateProgress::Error(s) => {
                    self.update_progress_message = s;
                    self.update_state = UpdateState::Failed;
                }
            }
        }
    }

    /// Poll all background operations and apply their results.
    ///
    /// Consolidates the scattered `poll_*()` calls that were previously
    /// spread across `run_loop()` in `main.rs`.
    pub fn poll_all_background_ops(&mut self) {
        self.poll_bg_branches();
        self.poll_bg_pull();
        self.poll_grep_search();
        self.poll_update_progress();
        self.poll_pr_url();
        self.poll_worktree_switch_ops();
        self.poll_worktree_ops();

        // ccusage
        if let Some(info) = self.bg_ccusage_op.poll() {
            self.ccusage_info = Some(info);
        }

        // symbol index
        if let Some(result) = self.bg_symbol_index_op.poll() {
            match result {
                Ok(count) => {
                    log::info!("Symbol index built: {count} symbols");
                    self.set_status(format!("Symbol index ready ({count} symbols)"), StatusLevel::Success);
                }
                Err(msg) => {
                    log::warn!("Symbol index build failed: {msg}");
                }
            }
        }

        // update check
        if let Some(Some(info)) = self.bg_update_check_op.poll() {
            if crate::update_checker::is_newer(
                &info.latest_version,
                crate::update_checker::current_version(),
            ) {
                self.update_info = Some(info);
            }
        }
    }

    /// Record a stat event for both the current session and daily totals.
    fn record_stat(&self, field: &str) {
        if let Some(store) = &self.review_store {
            let _ = store.increment_daily_stat(field);
            if let Some(ref sid) = self.stats_session_id {
                let _ = store.increment_session_stat(sid, field);
            }
        }
    }

    // ── Focus cycling ────────────────────────────────────────────────

    /// Cycle focus forward: Worktree → Explorer → Viewer → TerminalClaude → TerminalShell → Worktree
    pub fn cycle_focus_forward(&mut self) {
        let next = match self.focus {
            Focus::Worktree => Focus::Explorer,
            Focus::Explorer => Focus::Viewer,
            Focus::Viewer => Focus::TerminalClaude,
            Focus::TerminalClaude => Focus::TerminalShell,
            Focus::TerminalShell => Focus::Worktree,
        };
        self.set_focus(next);
    }

    /// Cycle focus backward.
    pub fn cycle_focus_backward(&mut self) {
        let prev = match self.focus {
            Focus::Worktree => Focus::TerminalShell,
            Focus::Explorer => Focus::Worktree,
            Focus::Viewer => Focus::Explorer,
            Focus::TerminalClaude => Focus::Viewer,
            Focus::TerminalShell => Focus::TerminalClaude,
        };
        self.set_focus(prev);
    }


    // ── Public accessor helpers ─────────────────────────────────────

    /// Return the branch name used as the worktree identifier.
    pub fn selected_worktree_branch(&self) -> String {
        self.worktrees
            .get(self.selected_worktree)
            .map(|w| w.branch.clone())
            .unwrap_or_default()
    }

    /// Return `true` if the currently selected worktree is on a `__grab` branch
    /// (i.e. its real branch was grabbed away to main and it holds a temporary checkout).
    pub fn is_selected_worktree_grabbed(&self) -> bool {
        self.worktrees
            .get(self.selected_worktree)
            .map(|w| w.branch.ends_with("__grab"))
            .unwrap_or(false)
    }

    /// Return the directory path for the currently selected worktree.
    pub fn selected_worktree_path(&self) -> PathBuf {
        self.worktrees
            .get(self.selected_worktree)
            .map(|w| w.path.clone())
            .unwrap_or_else(|| self.repo_path.clone())
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
                    rows.push(WorktreeListRow::Session { wt_idx: i, pty_idx: *pty_idx });
                }
            }
        }
        self.worktree_list_rows = rows;
        // Clamp selected index.
        if !self.worktree_list_rows.is_empty() && self.worktree_list_selected >= self.worktree_list_rows.len() {
            self.worktree_list_selected = self.worktree_list_rows.len() - 1;
        }
    }

    /// Derive `selected_worktree` from the current `worktree_list_selected`.
    pub fn sync_selected_worktree(&mut self) {
        if let Some(row) = self.worktree_list_rows.get(self.worktree_list_selected) {
            let wt_idx = match *row {
                WorktreeListRow::Worktree(i) => i,
                WorktreeListRow::Session { wt_idx, .. } => wt_idx,
            };
            if wt_idx < self.worktrees.len() {
                self.selected_worktree = wt_idx;
            }
        }
    }

    /// Return `(worktree_name, working_dir)` for the currently selected worktree.
    fn selected_worktree_info(&self) -> (String, PathBuf) {
        self.worktrees
            .get(self.selected_worktree)
            .map(|w| (w.branch.clone(), w.path.clone()))
            .unwrap_or_else(|| ("default".to_string(), self.repo_path.clone()))
    }
}

/// Run the update download-and-build in a background thread.
///
/// Sends [`UpdateProgress`] messages via the channel to report status.
fn perform_update(
    tx: &mpsc::Sender<UpdateProgress>,
    version: &str,
    tarball_url: &str,
    assets: &[crate::update_checker::ReleaseAsset],
) {
    let tmpdir = std::env::temp_dir().join(format!("conductor-update-{version}"));
    let _ = std::fs::remove_dir_all(&tmpdir);
    if std::fs::create_dir_all(&tmpdir).is_err() {
        let _ = tx.send(UpdateProgress::Error("Failed to create temp directory".to_string()));
        return;
    }

    // Try pre-built binary first, then fall back to source build.
    if try_binary_update(tx, version, assets, &tmpdir) {
        let _ = std::fs::remove_dir_all(&tmpdir);
        let _ = tx.send(UpdateProgress::Done(format!(
            "v{version} installed successfully! Restarting..."
        )));
        return;
    }

    // Fallback: source build.
    log::info!("no pre-built binary available, falling back to source build");
    if try_source_update(tx, version, tarball_url, &tmpdir) {
        let _ = std::fs::remove_dir_all(&tmpdir);
        let _ = tx.send(UpdateProgress::Done(format!(
            "v{version} installed successfully! Restarting..."
        )));
    } else {
        let _ = std::fs::remove_dir_all(&tmpdir);
    }
}

/// Attempt to install via pre-built binary. Returns `true` on success.
fn try_binary_update(
    tx: &mpsc::Sender<UpdateProgress>,
    version: &str,
    assets: &[crate::update_checker::ReleaseAsset],
    tmpdir: &std::path::Path,
) -> bool {
    use std::process::Command;

    let asset = match crate::update_checker::find_binary_asset(assets) {
        Some(a) => a,
        None => {
            log::debug!("no matching binary asset for this platform");
            return false;
        }
    };

    let _ = tx.send(UpdateProgress::Status(format!(
        "Downloading pre-built binary v{version}..."
    )));

    let archive = tmpdir.join(&asset.name);
    let mut curl_args = vec![
        "-fL".to_string(),
        "--max-time".to_string(),
        "120".to_string(),
        "-o".to_string(),
        archive.to_string_lossy().to_string(),
    ];

    // Use GITHUB_TOKEN if available.
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        curl_args.push("-H".to_string());
        curl_args.push(format!("Authorization: token {token}"));
    }

    curl_args.push(asset.download_url.clone());

    let dl = Command::new("curl")
        .args(&curl_args)
        .stdin(std::process::Stdio::null())
        .output();
    match dl {
        Err(e) => {
            log::warn!("binary download failed (curl): {e}");
            return false;
        }
        Ok(out) if !out.status.success() => {
            log::warn!("binary download failed (HTTP error)");
            return false;
        }
        _ => {}
    }

    // Extract.
    let _ = tx.send(UpdateProgress::Status("Extracting binary...".to_string()));
    let extract = Command::new("tar")
        .arg("xzf")
        .arg(&archive)
        .arg("-C")
        .arg(tmpdir)
        .output();
    match extract {
        Err(e) => {
            log::warn!("binary extraction failed: {e}");
            return false;
        }
        Ok(out) if !out.status.success() => {
            log::warn!("binary extraction failed (tar error)");
            return false;
        }
        _ => {}
    }

    // The tar.gz contains the `conductor` binary at the top level.
    let binary = tmpdir.join("conductor");
    if !binary.exists() {
        log::warn!("conductor binary not found in archive");
        return false;
    }

    // Install to ~/.cargo/bin/ (same location as `cargo install`).
    let install_dir = match dirs::home_dir() {
        Some(h) => h.join(".cargo").join("bin"),
        None => {
            log::warn!("could not determine home directory");
            return false;
        }
    };
    if std::fs::create_dir_all(&install_dir).is_err() {
        log::warn!("could not create install dir");
        return false;
    }

    let dest = install_dir.join("conductor");
    let _ = tx.send(UpdateProgress::Status("Installing binary...".to_string()));

    if let Err(e) = std::fs::copy(&binary, &dest) {
        log::warn!("failed to install binary: {e}");
        return false;
    }

    // Ensure executable permission on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }

    // Remove macOS quarantine attribute so Gatekeeper won't kill the binary.
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr")
            .args(["-cr"])
            .arg(&dest)
            .output();
    }

    true
}

/// Attempt to install via source download + build. Returns `true` on success.
fn try_source_update(
    tx: &mpsc::Sender<UpdateProgress>,
    version: &str,
    tarball_url: &str,
    tmpdir: &std::path::Path,
) -> bool {
    use std::process::Command;

    // Resolve tarball URL — if empty, re-fetch from API.
    let url = if tarball_url.is_empty() {
        let _ = tx.send(UpdateProgress::Status("Fetching release info...".to_string()));
        match crate::update_checker::check_for_update() {
            Some(info) if !info.tarball_url.is_empty() => info.tarball_url,
            _ => {
                let _ = tx.send(UpdateProgress::Error("Could not find tarball URL".to_string()));
                return false;
            }
        }
    } else {
        tarball_url.to_string()
    };

    // Download.
    let _ = tx.send(UpdateProgress::Status(format!("Downloading source v{version}...")));
    let tarball = tmpdir.join("source.tar.gz");
    let dl = Command::new("curl")
        .args(["-sfL", "--max-time", "120", "-o"])
        .arg(&tarball)
        .arg(&url)
        .stdin(std::process::Stdio::null())
        .output();
    match dl {
        Err(e) => {
            let _ = tx.send(UpdateProgress::Error(format!("curl not found: {e}")));
            return false;
        }
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let _ = tx.send(UpdateProgress::Error(format!("Download failed: {stderr}")));
            return false;
        }
        _ => {}
    }

    // Extract.
    let _ = tx.send(UpdateProgress::Status("Extracting...".to_string()));
    let extract = Command::new("tar")
        .args(["xzf", "source.tar.gz"])
        .current_dir(tmpdir)
        .output();
    match extract {
        Err(e) => {
            let _ = tx.send(UpdateProgress::Error(format!("tar not found: {e}")));
            return false;
        }
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let _ = tx.send(UpdateProgress::Error(format!("Extraction failed: {stderr}")));
            return false;
        }
        _ => {}
    }

    // Find the extracted directory (GitHub tarballs extract to owner-repo-hash/).
    let src_dir = match std::fs::read_dir(tmpdir) {
        Ok(entries) => {
            let mut found = None;
            for entry in entries.flatten() {
                if entry.path().is_dir() && entry.file_name() != "source.tar.gz" {
                    found = Some(entry.path());
                    break;
                }
            }
            match found {
                Some(d) => d,
                None => {
                    let _ = tx.send(UpdateProgress::Error(
                        "No source directory found in tarball".to_string(),
                    ));
                    return false;
                }
            }
        }
        Err(e) => {
            let _ = tx.send(UpdateProgress::Error(format!("Failed to read temp dir: {e}")));
            return false;
        }
    };

    // Build & install.
    let _ = tx.send(UpdateProgress::Status(format!(
        "Building v{version}... (this may take a while)"
    )));
    let build = Command::new("make")
        .arg("install")
        .current_dir(&src_dir)
        .output();
    match build {
        Err(e) => {
            let _ = tx.send(UpdateProgress::Error(format!("make not found: {e}")));
            return false;
        }
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let msg = if stderr.len() > 200 {
                format!("Build failed: ...{}", &stderr[stderr.len() - 200..])
            } else {
                format!("Build failed: {stderr}")
            };
            let _ = tx.send(UpdateProgress::Error(msg));
            return false;
        }
        _ => {}
    }

    true
}

// ── Free functions for symbol extraction ──────────────────────────────

/// Extract a symbol name from a source code line at the cursor position.
/// Returns the first Rust-like identifier found on the line that is not a keyword.
pub fn extract_symbol_from_line(line: &str) -> Option<String> {
    let re = regex::Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\b").ok()?;
    for cap in re.captures_iter(line) {
        let word = cap.get(1)?.as_str();
        if !is_rust_keyword(word) && word.len() > 1 {
            return Some(word.to_string());
        }
    }
    None
}

/// Check if a word is a Rust keyword (should not be treated as a symbol).
pub fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async" | "await" | "break" | "const" | "continue" | "crate"
            | "dyn" | "else" | "enum" | "extern" | "false" | "fn" | "for"
            | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod"
            | "move" | "mut" | "pub" | "ref" | "return" | "self" | "Self"
            | "static" | "struct" | "super" | "trait" | "true" | "type"
            | "unsafe" | "use" | "where" | "while" | "yield"
    )
}

/// Extract the symbol (identifier) at a specific column in a line.
/// Returns `(symbol_text, start_col, end_col)` where cols are 0-indexed character offsets.
pub fn extract_symbol_at_column(line: &str, col: usize) -> Option<(String, usize, usize)> {
    if col >= line.len() {
        return None;
    }
    // Check that the character at `col` is part of an identifier.
    let ch = line.as_bytes().get(col).copied()?;
    if !(ch.is_ascii_alphanumeric() || ch == b'_') {
        return None;
    }
    // Walk backwards to find start of identifier.
    let start = line[..col]
        .bytes()
        .rev()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let start_col = col - start;
    // Walk forwards to find end of identifier.
    let end = line[col..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let end_col = col + end;
    let word = &line[start_col..end_col];
    if word.len() <= 1 || is_rust_keyword(word) {
        return None;
    }
    // Must start with letter or underscore.
    if !word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    Some((word.to_string(), start_col, end_col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_symbol_at_column_basic() {
        let line = "    let foo = AppState::new();";
        // Click on 'A' of AppState at col 14
        let result = extract_symbol_at_column(line, 14);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_middle() {
        let line = "    let foo = AppState::new();";
        // Click on 'S' of AppState at col 17
        let result = extract_symbol_at_column(line, 17);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_on_keyword() {
        let line = "    let foo = bar;";
        // Click on 'l' of let at col 4
        let result = extract_symbol_at_column(line, 4);
        assert_eq!(result, None); // "let" is a keyword
    }

    #[test]
    fn test_extract_symbol_at_column_on_space() {
        let line = "fn main() {}";
        let result = extract_symbol_at_column(line, 2);
        assert_eq!(result, None); // space
    }

    #[test]
    fn test_extract_symbol_at_column_out_of_bounds() {
        let line = "short";
        let result = extract_symbol_at_column(line, 100);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_single_char() {
        let line = "x + y";
        // Single char identifiers are filtered out
        let result = extract_symbol_at_column(line, 0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_underscore_prefix() {
        let line = "    _handler.call();";
        let result = extract_symbol_at_column(line, 5);
        assert_eq!(result, Some(("_handler".to_string(), 4, 12)));
    }
}
