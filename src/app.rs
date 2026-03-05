//! App state and focus management.
//!
//! This module defines the top-level application state, the unified panel
//! layout focus model, and transitions between panels.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::background::BackgroundOp;

use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::config;
use crate::diff_state::{DiffState, DiffViewMode};
use crate::git_engine;
use crate::grep_search::GrepProgress;
use crate::keymap::KeyMap;
use crate::overlay::{
    CherryPickOverlay, CommandPaletteOverlay, GrabOverlay, GrepSearchOverlay, HelpOverlay,
    HistoryOverlay, OpenRepoOverlay, PruneOverlay, RepoSelectorOverlay, ResumeSessionOverlay,
    SwitchBranchOverlay,
};
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

/// Run the LLM generation for smart worktree (branch name + prompt) via `claude --print`.
fn run_smart_generation(desc: &str) -> Result<SmartGenResult, String> {
    let system_prompt = r#"You are a helper that generates a git branch name and a Claude Code prompt from a task description.
Output ONLY a JSON object with two fields:
- "branch": a kebab-case branch name in English, 3-5 words, prefixed with "feature/", "fix/", or "refactor/" as appropriate.
- "prompt": a detailed, actionable prompt for Claude Code to implement the task. Write the prompt in the same language as the input description.
No markdown fences, no explanation, just the JSON object."#;

    let mut child = std::process::Command::new("claude")
        .args(["--print", "-p", system_prompt])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn claude: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(desc.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for claude: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("claude exited with {}: {}", output.status, stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_str = stdout
        .trim()
        .strip_prefix("```json")
        .or_else(|| stdout.trim().strip_prefix("```"))
        .unwrap_or(stdout.trim());
    let json_str = json_str
        .strip_suffix("```")
        .unwrap_or(json_str)
        .trim();
    serde_json::from_str::<SmartGenResult>(json_str)
        .map_err(|e| format!("JSON parse error: {e}\nRaw output: {stdout}"))
}

/// Info about a grabbed branch (branch checkout swap with main).
#[derive(Debug, Clone)]
pub struct GrabbedBranch {
    /// The original branch name (e.g., "feature-x").
    pub branch: String,
    /// Path of the worktree that originally had this branch.
    pub source_worktree: PathBuf,
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

/// Top-level application state shared across all UI panels.
pub struct App {
    /// Current panel focus.
    pub focus: Focus,
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
    /// Session history overlay state.
    pub history: HistoryOverlay,
    /// Cherry-pick overlay state.
    pub cherry_pick: CherryPickOverlay,
    /// List of known repository paths (including the current one).
    pub repo_list: Vec<std::path::PathBuf>,
    /// Index of the currently active repository in repo_list.
    pub repo_list_index: usize,
    /// Repository selector overlay state.
    pub repo_selector: RepoSelectorOverlay,
    /// Open-repository path input overlay state.
    pub open_repo: OpenRepoOverlay,


    // ── Switch (remote branch checkout) ─────────────────────────
    /// Switch-branch overlay state.
    pub switch_branch: SwitchBranchOverlay,

    // ── Grab (checkout branch on main) ─────────────────────────
    /// Grab branch overlay state.
    pub grab: GrabOverlay,

    // ── Prune ───────────────────────────────────────────────────
    /// Prune overlay state.
    pub prune: PruneOverlay,


    // ── Resume Claude session overlay ─────────────────────────
    /// Resume-session overlay state.
    pub resume_session: ResumeSessionOverlay,

    // ── Syntax highlighting (syntect) ──────────────────────────
    /// Shared syntect syntax definitions.
    pub syntax_set: SyntaxSet,
    /// Active syntect highlighting theme.
    pub syntect_theme: syntect::highlighting::Theme,

    // ── Help overlay ─────────────────────────────────────────────
    /// Help overlay state.
    pub help: HelpOverlay,

    /// Which panel is currently expanded to 100% (via the [<=>] button).
    /// `None` means no panel is expanded (default layout).
    pub expanded_panel: Option<Focus>,

    // ── Command palette overlay ─────────────────────────────────
    /// Command palette overlay state.
    pub command_palette: CommandPaletteOverlay,

    // ── Grep (full-text search) overlay ─────────────────────────
    /// Grep search overlay state.
    pub grep_search: GrepSearchOverlay,

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


    // ── Auto-resume Claude sessions ─────────────────────────────
    /// Whether auto-resume should run on the next frame (one-shot).
    pub pending_auto_resume: bool,
}

/// Aggregated token usage and cost from ccusage.
#[derive(Debug, Clone)]
pub struct CcusageInfo {
    pub total_tokens: u64,
    pub total_cost: f64,
}

impl App {
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
            focus: Focus::Worktree,
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
            history: HistoryOverlay::default(),
            cherry_pick: CherryPickOverlay::default(),
            repo_list,
            repo_list_index: 0,
            repo_selector: RepoSelectorOverlay::default(),
            open_repo: OpenRepoOverlay::default(),
            switch_branch: SwitchBranchOverlay::default(),
            grab: GrabOverlay::default(),
            prune: PruneOverlay::default(),
            resume_session: ResumeSessionOverlay::default(),
            syntax_set,
            syntect_theme,
            help: HelpOverlay::default(),
            expanded_panel: None,
            command_palette: CommandPaletteOverlay::default(),
            grep_search: GrepSearchOverlay::default(),
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
            pending_auto_resume: auto_resume,
        };
        app.refresh_worktrees();
        app.refresh_reviews();

        // Restore grab state from $git_common_dir/wt-grab if it exists.
        if let Ok(engine) = git_engine::GitEngine::open(&app.repo_path) {
            match engine.load_grab_state() {
                Ok(Some((branch, source_worktree, _stash_branch))) => {
                    app.worktree_mgr.grabbed_branch = Some(GrabbedBranch {
                        branch,
                        source_worktree,
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
    pub fn refresh_worktrees(&mut self) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_worktrees() {
                    Ok(worktrees) => {
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
                    self.worktree_mgr.local_branches = branches;
                }
            }
            Err(e) => {
                log::warn!("failed to open git repository: {e}");
            }
        }
        self.rebuild_worktree_list_rows();
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
                if self.viewer_state.file_tree.is_empty() {
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
                Focus::Worktree => "Opt+1-5: jump | Tab: next | q: quit | j/k: nav | w/W: new/del | s: switch | g: grab | G: ungrab | P: prune",
                Focus::Explorer => "Opt+1-5: jump | Tab: next panel | j/k: navigate | Enter: open file | h/l: collapse/expand | d: diff list",
                Focus::Viewer => "Opt+1-5: jump | Tab: next panel | Esc: back to explorer | j/k: scroll | /: search | c: comment",
                Focus::TerminalClaude => "Opt+1-5: jump | Ctrl+n: new CC | Ctrl+p: palette | Ctrl+w: worktree | keys → PTY",
                Focus::TerminalShell => "Opt+1-5: jump | Ctrl+t: new shell | keys → PTY",
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.focus {
                Focus::Worktree => "Alt+1-5: jump | Tab: next | q: quit | j/k: nav | w/W: new/del | s: switch | g: grab | G: ungrab | P: prune",
                Focus::Explorer => "Alt+1-5: jump | Tab: next panel | j/k: navigate | Enter: open file | h/l: collapse/expand | d: diff list",
                Focus::Viewer => "Alt+1-5: jump | Tab: next panel | Esc: back to explorer | j/k: scroll | /: search | c: comment",
                Focus::TerminalClaude => "Alt+1-5: jump | Ctrl+n: new CC | Ctrl+p: palette | Ctrl+w: worktree | keys → PTY",
                Focus::TerminalShell => "Alt+1-5: jump | Ctrl+t: new shell | keys → PTY",
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
            CommandId::TogglePanelExpand => {
                if self.expanded_panel == Some(self.focus) {
                    self.expanded_panel = None;
                } else {
                    self.expanded_panel = Some(self.focus);
                }
            }
            CommandId::CreateWorktree => {
                self.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
                self.worktree_mgr.input_buffer.clear();
                self.set_status_info("New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string());
            }
            CommandId::DeleteWorktree => {
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
            CommandId::SwitchBranch => {
                self.set_status_info("Loading branches...".to_string());
                self.load_switch_branches();
                if !self.switch_branch.branches.is_empty() {
                    self.switch_branch.active = true;
                    self.status_message = None;
                }
            }
            CommandId::GrabBranch => {
                if self.worktree_mgr.grabbed_branch.is_some() {
                    self.set_status("Already grabbing a branch. Ungrab first (Y).".to_string(), StatusLevel::Warning);
                } else {
                    self.load_grab_branches();
                    if self.grab.branches.is_empty() {
                        self.set_status_info("No non-main worktrees to grab.".to_string());
                    } else {
                        self.grab.active = true;
                    }
                }
            }
            CommandId::PruneWorktrees => {
                match crate::git_engine::GitEngine::open(&self.repo_path) {
                    Ok(engine) => match engine.find_stale_worktrees() {
                        Ok(stale) => {
                            if stale.is_empty() {
                                self.set_status_info("No stale worktrees found.".to_string());
                            } else {
                                self.prune.stale = stale;
                                self.prune.active = true;
                            }
                        }
                        Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
                    },
                    Err(e) => self.set_status(format!("Error: {e}"), StatusLevel::Error),
                }
            }
            CommandId::MergeToMain => {
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
            CommandId::RefreshWorktrees => self.refresh_worktrees(),
            CommandId::ResetMainToOrigin => {
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
            CommandId::CherryPick => {
                let current_branch = self.selected_worktree_branch();
                let source = self.worktrees.iter()
                    .find(|w| w.branch != current_branch)
                    .map(|w| w.branch.clone());
                if let Some(branch) = source {
                    self.cherry_pick.source_branch = branch;
                    self.load_cherry_pick_commits();
                    self.cherry_pick.active = true;
                } else {
                    self.set_status_info("No other worktree branches available.".to_string());
                }
            }
            CommandId::NewClaudeCode => {
                if let Err(e) = self.spawn_claude_code() {
                    self.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
                }
                self.set_focus(Focus::TerminalClaude);
            }
            CommandId::NewShell => {
                if let Err(e) = self.spawn_shell() {
                    self.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
                }
                self.set_focus(Focus::TerminalShell);
            }
            CommandId::ResumeClaudeSession => {
                self.resume_session.active = true;
                self.load_resume_sessions();
            }
            CommandId::RefreshDiff => self.refresh_diff(),
            CommandId::SearchInFile => {
                self.viewer_state.search_active = true;
                self.viewer_state.search_query.clear();
                self.set_focus(Focus::Viewer);
            }
            CommandId::ToggleHelp => {
                self.help.context = self.focus;
                self.help.active = true;
            }
            CommandId::ShowReviewComments => {
                self.viewer_state.explorer_show_comments = true;
                self.viewer_state.explorer_focus_on_diff_list = true;
                self.set_focus(Focus::Explorer);
            }
            CommandId::ShowReviewTemplates => {
                self.review_state.template_picker_active = true;
            }
            CommandId::SessionHistory => {
                self.history.active = true;
                self.load_session_history();
            }
            CommandId::OpenRepo => {
                self.open_repo.active = true;
                self.open_repo.buffer.set_text(&self.repo_path.display().to_string());
            }
            CommandId::SwitchRepo => {
                if self.repo_list.len() > 1 {
                    self.repo_selector.active = true;
                    self.repo_selector.selected = self.repo_list_index;
                }
            }
            CommandId::UngrabBranch => {
                if self.worktree_mgr.grabbed_branch.is_none() {
                    self.set_status("Not grabbing — nothing to ungrab.".to_string(), StatusLevel::Warning);
                } else {
                    self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingUngrab;
                    self.set_status("Ungrab? Main will return to main branch. (y/n)".to_string(), StatusLevel::Warning);
                }
            }
            CommandId::ShowDiffList => {
                self.viewer_state.explorer_show_comments = false;
                self.viewer_state.explorer_focus_on_diff_list = true;
                self.set_focus(Focus::Explorer);
            }
            CommandId::ShowCommentList => {
                self.viewer_state.explorer_show_comments = true;
                self.viewer_state.explorer_focus_on_diff_list = true;
                self.set_focus(Focus::Explorer);
            }
            CommandId::AddReviewComment => {
                if let Some(file_path) = self.viewer_state.current_file.clone() {
                    let location = if let Some((start, end)) = self.viewer_state.selected_range() {
                        if start == end {
                            format!("{file_path}:{start} ")
                        } else {
                            format!("{file_path}:{start}-{end} ")
                        }
                    } else {
                        let line = self.viewer_state.file_scroll + 1;
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
            CommandId::ViewCommentDetail => {
                // Try viewer context first (current line), then comment list context.
                if self.viewer_state.current_file.is_some() {
                    let cursor_line = if let Some((start, _)) = self.viewer_state.selected_range() {
                        start
                    } else {
                        self.viewer_state.file_scroll + 1
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
            CommandId::DeleteComment => {
                if self.viewer_state.explorer_show_comments
                    && self.viewer_state.explorer_focus_on_diff_list
                    && !self.review_state.comment_list_rows.is_empty()
                {
                    self.delete_selected_review_comment();
                } else {
                    self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
                }
            }
            CommandId::ToggleCommentResolve => {
                if self.viewer_state.explorer_show_comments
                    && self.viewer_state.explorer_focus_on_diff_list
                    && !self.review_state.comment_list_rows.is_empty()
                {
                    self.toggle_selected_review_status();
                } else {
                    self.set_status("No comment selected.".to_string(), StatusLevel::Warning);
                }
            }
            CommandId::EditComment => {
                let comment_idx = self
                    .review_state
                    .selected_comment_idx(self.viewer_state.comment_list_selected);
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
            CommandId::ReplyToComment => {
                let comment_idx = self
                    .review_state
                    .selected_comment_idx(self.viewer_state.comment_list_selected);
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
            CommandId::SaveSessionHistory => {
                self.save_current_session_history();
            }
            CommandId::OpenPullRequest => {
                self.open_pr_in_browser();
            }
            CommandId::UpdateAndRestart => {
                if self.update_info.is_some() {
                    self.start_update_confirm();
                } else {
                    self.set_status("No update available.".to_string(), StatusLevel::Info);
                }
            }
            CommandId::SearchFullText => {
                self.grep_search.active = true;
                self.grep_search.query.clear();
                self.grep_search.results.clear();
                self.grep_search.selected = 0;
                self.grep_search.scroll = 0;
                self.grep_search.running = false;
                self.grep_search.bg_op.clear();
                self.grep_search.bg_op_phase2.clear();
                self.grep_search.debounce_deadline = None;
                self.grep_search.phase1_active = false;
            }
            CommandId::Quit => self.should_quit = true,
        }
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

        self.update_state = UpdateState::InProgress;
        self.update_progress_message = "Preparing update...".to_string();

        self.update_op.start(move |tx| {
            perform_update(&tx, &version, &tarball_url);
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

    // ── Terminal / PTY helpers ────────────────────────────────────────

    /// Spawn a new Claude Code PTY session for the currently selected worktree.
    pub fn spawn_claude_code(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let cc_count = self
            .terminal.pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::ClaudeCode)
            .count();
        let label = format!("CC:{}", cc_count + 1);
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::ClaudeCode,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            None,
            &self.repo_path,
        )?;
        self.terminal.pty_manager.activate_session(idx);
        self.terminal.active_claude_session = Some(idx);
        self.rebuild_worktree_list_rows();
        Ok(idx)
    }

    /// Spawn a new interactive shell PTY session for the currently selected worktree.
    pub fn spawn_shell(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let sh_count = self
            .terminal.pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::Shell)
            .count();
        let label = format!("SH:{}", sh_count + 1);
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_shell;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::Shell,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            None,
            &self.repo_path,
        )?;
        self.terminal.pty_manager.activate_session(idx);
        self.terminal.active_shell_session = Some(idx);
        Ok(idx)
    }

    /// Close (kill + remove) a terminal session by its global index.
    ///
    /// Adjusts `active_claude_session` and `active_shell_session` indices
    /// and falls back to the next available session for the current worktree.
    pub fn close_terminal_session(&mut self, global_idx: usize) {
        // Kill and remove the session.
        let _ = self.terminal.pty_manager.kill_session(global_idx);
        self.terminal.pty_manager.remove_session(global_idx);

        // Adjust active session indices.
        for a in [&mut self.terminal.active_claude_session, &mut self.terminal.active_shell_session]
            .into_iter()
            .flatten()
        {
            if *a == global_idx {
                *a = usize::MAX; // mark for clear
            } else if *a > global_idx {
                *a -= 1;
            }
        }

        // Clear invalidated indices and fall back to next available session.
        if self.terminal.active_claude_session == Some(usize::MAX) {
            self.terminal.active_claude_session = self
                .current_worktree_claude_sessions()
                .first()
                .map(|(idx, _)| *idx);
        }
        if self.terminal.active_shell_session == Some(usize::MAX) {
            self.terminal.active_shell_session = self
                .current_worktree_shell_sessions()
                .first()
                .map(|(idx, _)| *idx);
        }
        self.rebuild_worktree_list_rows();
    }

    /// Remove PTY sessions whose child processes have exited.
    ///
    /// Iterates in reverse to preserve indices of earlier sessions while
    /// removing later ones. Adjusts `active_claude_session` and
    /// `active_shell_session` indices after removal.
    pub fn cleanup_dead_sessions(&mut self) {
        let count = self.terminal.pty_manager.session_count();
        let mut removed_any = false;

        // Walk backwards so removals don't shift indices we haven't checked yet.
        for idx in (0..count).rev() {
            if !self.terminal.pty_manager.is_session_alive(idx) {
                log::info!("removing dead PTY session at index {idx}");
                self.terminal.pty_manager.remove_session(idx);
                removed_any = true;

                // Adjust active session indices.
                for a in [&mut self.terminal.active_claude_session, &mut self.terminal.active_shell_session].into_iter().flatten() {
                    if *a == idx {
                        *a = usize::MAX; // mark for clear
                    } else if *a > idx {
                        *a -= 1;
                    }
                }
            }
        }

        if removed_any {
            // Clear any indices that were pointing at removed sessions.
            if self.terminal.active_claude_session == Some(usize::MAX) {
                self.terminal.active_claude_session = None;
            }
            if self.terminal.active_shell_session == Some(usize::MAX) {
                self.terminal.active_shell_session = None;
            }
        }
    }

    /// Load resumable Claude Code sessions from Claude's history.
    pub fn load_resume_sessions(&mut self) {
        let filter = if self.resume_session.all_projects {
            None
        } else {
            Some(self.repo_path.as_path())
        };
        match crate::claude_sessions::load_resumable_sessions(filter) {
            Ok(sessions) => {
                self.resume_session.sessions = sessions;
                self.resume_session.selected = 0;
                self.resume_session.filter.clear();
            }
            Err(e) => {
                log::warn!("failed to load resumable sessions: {e}");
                self.resume_session.sessions.clear();
                self.set_status(format!("Error loading sessions: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Return the filtered list of resume sessions based on the current filter string.
    pub fn filtered_resume_sessions(&self) -> Vec<(usize, &crate::claude_sessions::ResumableSession)> {
        if self.resume_session.filter.is_empty() {
            self.resume_session.sessions.iter().enumerate().collect()
        } else {
            let filter_lower = self.resume_session.filter.to_lowercase();
            self.resume_session.sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.display.to_lowercase().contains(&filter_lower)
                        || s.session_id.to_lowercase().contains(&filter_lower)
                        || s.project_name.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Resume a Claude Code session by its session ID.
    pub fn resume_claude_session(&mut self, session_id: &str, display: &str) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let label: String = display.chars().take(40).collect();
        let label = if label.is_empty() {
            format!("Resume:{}", &session_id[..8.min(session_id.len())])
        } else {
            label
        };
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::ClaudeCode,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            Some(session_id),
            &self.repo_path,
        )?;
        self.terminal.pty_manager.activate_session(idx);
        self.terminal.active_claude_session = Some(idx);
        Ok(idx)
    }

    /// Automatically resume Claude Code sessions for all worktrees that had a
    /// previous session. Called once after the first frame render.
    pub fn perform_auto_resume(&mut self) {
        if !self.pending_auto_resume {
            return;
        }
        self.pending_auto_resume = false;

        let paths: Vec<PathBuf> = self.worktrees.iter().map(|w| w.path.clone()).collect();
        if paths.is_empty() {
            return;
        }

        let sessions = match crate::claude_sessions::find_latest_sessions_for_paths(&paths) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("auto-resume: failed to find sessions: {e}");
                return;
            }
        };

        if sessions.is_empty() {
            return;
        }

        let selected_wt_path = self.selected_worktree_path();
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let repo_path = self.repo_path.clone();
        let mut resumed_count = 0;

        for wt in &self.worktrees.clone() {
            let canonical = std::fs::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());
            let session = match sessions.get(&canonical) {
                Some(s) => s,
                None => continue,
            };

            let label: String = session.display.chars().take(40).collect();
            let label = if label.is_empty() {
                format!("Resume:{}", &session.session_id[..8.min(session.session_id.len())])
            } else {
                label
            };

            match self.terminal.pty_manager.spawn_session(
                pty_manager::SessionKind::ClaudeCode,
                &wt.branch,
                &label,
                &shell,
                &wt.path,
                rows,
                cols,
                Some(&session.session_id),
                &repo_path,
            ) {
                Ok(idx) => {
                    resumed_count += 1;
                    // Only activate + set active_claude_session for the currently selected worktree.
                    if wt.path == selected_wt_path {
                        self.terminal.pty_manager.activate_session(idx);
                        self.terminal.active_claude_session = Some(idx);
                    }
                }
                Err(e) => {
                    log::warn!("auto-resume: failed to spawn session for {}: {e}", wt.branch);
                }
            }
        }

        if resumed_count > 0 {
            self.set_status(
                format!("Auto-resumed {resumed_count} Claude session(s)"),
                StatusLevel::Success,
            );
        }
    }

    /// Return `(index_in_pty_manager, &PtySession)` pairs for Claude Code sessions
    /// belonging to the currently selected worktree.
    pub fn current_worktree_claude_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal.pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == pty_manager::SessionKind::ClaudeCode)
            .collect()
    }

    /// Return `(index_in_pty_manager, &PtySession)` pairs for Shell sessions
    /// belonging to the currently selected worktree.
    pub fn current_worktree_shell_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal.pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell)
            .collect()
    }

    /// Update the terminal content area size for Claude PTY sessions and resize them.
    pub fn update_claude_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_claude = (rows, cols);
        let wt_path = self.selected_worktree_path();
        let count = self.terminal.pty_manager.session_count();
        for idx in 0..count {
            let s = &self.terminal.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == pty_manager::SessionKind::ClaudeCode {
                self.terminal.pty_manager.resize_session(idx, rows, cols);
            }
        }
    }

    /// Update the terminal content area size for Shell PTY sessions and resize them.
    pub fn update_shell_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_shell = (rows, cols);
        let wt_path = self.selected_worktree_path();
        let count = self.terminal.pty_manager.session_count();
        for idx in 0..count {
            let s = &self.terminal.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell {
                self.terminal.pty_manager.resize_session(idx, rows, cols);
            }
        }
    }

    // ── Lightweight change-detection polling ─────────────────────────────

    /// Check whether the diff and viewer panels need refreshing by comparing
    /// the current worktree's HEAD oid and status counts against the last
    /// known values.  Only triggers the expensive `refresh_diff()` and
    /// `refresh_viewer()` when an actual change is detected.
    ///
    /// Called after `refresh_worktrees()` in the polling loop, which already
    /// fetches HEAD oids and status counts as a side effect.
    pub fn check_diff_viewer_staleness(&mut self) {
        let wt = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt,
            None => return,
        };

        let current_head = self.worktree_heads.get(&wt.branch).cloned();
        let current_status = (wt.added, wt.modified, wt.deleted);

        let head_changed = self.last_poll_head_oid.as_ref() != current_head.as_ref();
        let status_changed = self.last_poll_status != Some(current_status);

        if head_changed || status_changed {
            log::debug!(
                "Change detected for worktree '{}': head_changed={}, status_changed={}",
                wt.branch, head_changed, status_changed,
            );
            self.refresh_diff();
            self.refresh_viewer();
        }

        self.last_poll_head_oid = current_head;
        self.last_poll_status = Some(current_status);
    }

    // ── Claude Code input-waiting detection ────────────────────────────

    /// Scan all Claude Code sessions and update `cc_waiting_worktrees`.
    ///
    /// Uses two sources:
    /// 1. Hook signal files in `.conductor/cc-waiting/` (high reliability).
    /// 2. PTY pattern matching fallback (for `[Y/n]` prompts).
    ///
    /// If a worktree newly enters the waiting state and the user is not
    /// currently focused on that worktree's terminal, a status message is
    /// shown as a notification.
    pub fn check_cc_waiting_state(&mut self) {
        let mut new_waiting: HashSet<PathBuf> = HashSet::new();

        // Source 1: Hook signal files (high reliability).
        // Signal files are written by the plugin hook to the main repo's
        // `.conductor/cc-waiting/` directory.  Resolve via git so we look
        // in the right place even when Conductor was launched from a linked
        // worktree.
        let signal_dir = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone())
            .join(".conductor")
            .join("cc-waiting");
        if let Ok(entries) = std::fs::read_dir(&signal_dir) {
            for entry in entries.flatten() {
                let filename = entry.file_name().to_string_lossy().to_string();
                let signal_path: PathBuf = PathBuf::from(filename.replace("__", "/"));
                // Normalize both sides (strip trailing slashes) to ensure
                // comparison succeeds regardless of how paths were serialized.
                let signal_normalized: PathBuf = signal_path.components().collect();
                for wt in &self.worktrees {
                    let wt_normalized: PathBuf = wt.path.components().collect();
                    if wt_normalized == signal_normalized {
                        new_waiting.insert(wt.path.clone());
                    }
                }
            }
        }

        // Source 2: PTY pattern match fallback (for [Y/n] prompts).
        let session_count = self.terminal.pty_manager.session_count();
        for idx in 0..session_count {
            let session = &self.terminal.pty_manager.sessions()[idx];
            if session.kind != pty_manager::SessionKind::ClaudeCode {
                continue;
            }
            if self.terminal.pty_manager.is_waiting_for_input(idx) {
                new_waiting.insert(session.working_dir.clone());
            }
        }

        // Ignore waiting state for worktrees that have no CC session open.
        // Signal files may persist after a session has exited; without this
        // filter the notification bar would animate for a non-existent panel.
        new_waiting.retain(|wt_path| {
            self.terminal.pty_manager.sessions().iter().any(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == *wt_path
            })
        });

        // Detect worktrees that newly entered waiting state.
        let current_wt_path = self.selected_worktree_path();
        let is_terminal_focused = matches!(self.focus, Focus::TerminalClaude);

        // When the user is focused on a CC terminal, treat the waiting state
        // as acknowledged — remove it so the notification bar and worktree
        // animation are fully cleared (not just pulse-suppressed).
        if is_terminal_focused && new_waiting.remove(&current_wt_path) {
            // Record ack so the notification is not re-triggered by the
            // PTY pattern-match source until new output arrives.
            if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode
                    && s.working_dir == current_wt_path
            }) {
                let t = *session
                    .last_output_time
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                self.terminal.cc_waiting_ack_time.insert(current_wt_path.clone(), t);
            }
        }

        // Suppress re-triggering for worktrees the user already acknowledged
        // if the PTY has not produced any new output since that acknowledgment.
        let mut ack_expired: Vec<PathBuf> = Vec::new();
        new_waiting.retain(|wt_path| {
            if let Some(&ack_time) = self.terminal.cc_waiting_ack_time.get(wt_path) {
                if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                    s.kind == pty_manager::SessionKind::ClaudeCode
                        && s.working_dir == *wt_path
                }) {
                    let current = *session
                        .last_output_time
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if current == ack_time {
                        return false; // no new output — suppress
                    }
                }
                // New output arrived or session gone — ack is stale.
                ack_expired.push(wt_path.clone());
            }
            true
        });
        for p in ack_expired {
            self.terminal.cc_waiting_ack_time.remove(&p);
        }

        for wt_path in &new_waiting {
            if !self.terminal.cc_waiting_worktrees.contains(wt_path) {
                // Resolve display name from worktree list.
                let display_name = self.worktrees.iter()
                    .find(|w| &w.path == wt_path)
                    .map(|w| w.branch.clone())
                    .unwrap_or_else(|| "?".to_string());
                // Newly waiting — notify if user is not focused on that terminal.
                let skip_notify = is_terminal_focused && *wt_path == current_wt_path;
                if !skip_notify {
                    self.set_status(format!("CC waiting for input: {display_name}"), StatusLevel::Info);
                    if self.config.notification.cc_waiting {
                        let msg = format!("CC waiting for input: {display_name}");
                        std::thread::spawn(move || {
                            let _ = std::process::Command::new("terminal-notifier")
                                .args(["-title", "Conductor", "-message", &msg])
                                .output();
                        });
                    }
                }
            }
        }

        self.terminal.cc_waiting_worktrees = new_waiting;
    }

    /// Remove the hook signal file for a given session and clear its
    /// waiting state. Called when user sends input to a CC terminal.
    pub fn clear_cc_waiting_signal(&mut self, session_idx: usize) {
        let session = match self.terminal.pty_manager.sessions().get(session_idx) {
            Some(s) => s,
            None => return,
        };
        if session.kind != pty_manager::SessionKind::ClaudeCode {
            return;
        }
        // Record the PTY output timestamp so that the periodic scan does not
        // re-trigger the notification until new output actually arrives.
        let last_output = *session
            .last_output_time
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let working_dir = session.working_dir.clone();
        self.terminal.cc_waiting_ack_time.insert(working_dir.clone(), last_output);

        let signal_dir = git_engine::GitEngine::open(&self.repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo_path.clone())
            .join(".conductor")
            .join("cc-waiting");
        // Normalize the path (strip trailing slash) to match the shell's $PWD encoding.
        let normalized: PathBuf = session.working_dir.components().collect();
        let sanitized = normalized.display().to_string().replace('/', "__");
        let _ = std::fs::remove_file(signal_dir.join(&sanitized));
        self.terminal.cc_waiting_worktrees.remove(&working_dir);
    }

    // ── Review helpers ────────────────────────────────────────────────

    /// Reload review comments from the database for the currently selected worktree.
    pub fn refresh_reviews(&mut self) {
        if let Some(store) = &self.review_store {
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // Rebuild per-file cache for the currently viewed file.
            if let Some(file_path) = self.viewer_state.current_file.clone() {
                self.review_state.build_file_comment_cache(&file_path);
            }
        }
    }

    /// Add a new review comment for the current worktree and refresh the
    /// comment list.
    pub fn add_review_comment(
        &mut self,
        file_path: &str,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: &str,
        author: Author,
    ) {
        let branch = self
            .worktrees
            .get(self.selected_worktree)
            .map(|w| w.branch.clone());

        if let Some(store) = &self.review_store {
            let wt = self.selected_worktree_branch();
            match store.add_review(
                &wt,
                file_path,
                line_start,
                line_end,
                kind,
                body,
                "HEAD",
                author,
                branch.as_deref(),
            ) {
                Ok(_) => {
                    self.review_state.status_message =
                        Some("Comment added.".to_string());
                    self.record_stat("reviews_created");
                }
                Err(e) => {
                    log::warn!("failed to add review comment: {e}");
                    self.review_state.status_message =
                        Some(format!("Error: {e}"));
                }
            }
            self.review_state.load_comments(store, &wt);
            // Rebuild per-file cache for the commented file.
            self.review_state.build_file_comment_cache(file_path);
        }
    }

    /// Update the body of the currently selected review comment.
    pub fn update_selected_review_body(&mut self, new_body: &str) {
        let id = self
            .review_state
            .selected_comment()
            .map(|c| c.id.clone());

        if let (Some(store), Some(id)) = (&self.review_store, id) {
            match store.update_review_body(&id, new_body) {
                Ok(()) => {
                    self.review_state.status_message =
                        Some("Comment updated.".to_string());
                }
                Err(e) => {
                    log::warn!("failed to update review body: {e}");
                    self.review_state.status_message =
                        Some(format!("Error: {e}"));
                }
            }
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
        }
    }

    /// Delete the currently selected review comment (from explorer comment list).
    pub fn delete_selected_review_comment(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.comment_list_selected);
        let id = comment_idx
            .and_then(|idx| self.review_state.comments.get(idx))
            .map(|c| c.id.clone());

        if let (Some(store), Some(id)) = (&self.review_store, id) {
            match store.delete_review(&id) {
                Ok(()) => {
                    self.status_message = Some(StatusMessage::new("Comment deleted.".to_string(), StatusLevel::Success, self.ui_tick));
                }
                Err(e) => {
                    log::warn!("failed to delete review comment: {e}");
                    self.status_message = Some(StatusMessage::new(format!("Error: {e}"), StatusLevel::Error, self.ui_tick));
                }
            }
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // Clamp selection to valid range after deletion (using virtual row count).
            let row_count = self.review_state.comment_list_rows.len();
            if row_count == 0 {
                self.viewer_state.comment_list_selected = 0;
            } else if self.viewer_state.comment_list_selected >= row_count {
                self.viewer_state.comment_list_selected = row_count - 1;
            }
        }
    }

    /// Toggle the status of the currently selected review comment (Pending ↔ Resolved).
    pub fn toggle_selected_review_status(&mut self) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.comment_list_selected);
        let id_and_status = comment_idx
            .and_then(|idx| self.review_state.comments.get(idx))
            .map(|c| (c.id.clone(), c.status));

        if let (Some(store), Some((id, current_status))) = (&self.review_store, id_and_status) {
            use crate::review_store::CommentStatus;
            let new_status = match current_status {
                CommentStatus::Pending => CommentStatus::Resolved,
                CommentStatus::Resolved => CommentStatus::Pending,
            };
            match store.update_review_status(&id, new_status) {
                Ok(()) => {
                    let label = new_status.as_str();
                    self.status_message = Some(StatusMessage::new(format!("Comment marked as {label}."), StatusLevel::Success, self.ui_tick));
                }
                Err(e) => {
                    log::warn!("failed to update review status: {e}");
                    self.status_message = Some(StatusMessage::new(format!("Error: {e}"), StatusLevel::Error, self.ui_tick));
                }
            }
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
        }
    }

    /// Add a reply to the currently selected comment (from explorer comment list).
    pub fn add_reply_to_selected_comment(&mut self, body: &str) {
        let comment_idx = self
            .review_state
            .selected_comment_idx(self.viewer_state.comment_list_selected);
        let review_id = comment_idx
            .and_then(|idx| self.review_state.comments.get(idx))
            .map(|c| c.id.clone());

        if let (Some(store), Some(review_id)) = (&self.review_store, review_id) {
            match store.add_reply(&review_id, body, Author::User) {
                Ok(()) => {
                    self.status_message = Some(StatusMessage::new("Reply added.".to_string(), StatusLevel::Success, self.ui_tick));
                }
                Err(e) => {
                    log::warn!("failed to add reply: {e}");
                    self.status_message = Some(StatusMessage::new(format!("Error: {e}"), StatusLevel::Error, self.ui_tick));
                }
            }
            // Invalidate cached replies and reload.
            self.review_state.cached_replies.remove(&review_id);
            let wt = self.selected_worktree_branch();
            self.review_state.load_comments(store, &wt);
            // Reload replies for this comment if it was expanded.
            if self.review_state.expanded_comments.contains(&review_id) {
                if let Ok(replies) = store.get_replies(&review_id) {
                    self.review_state.cached_replies.insert(review_id, replies);
                    self.review_state.rebuild_comment_list_rows();
                }
            }
        }
    }

    /// Toggle expansion of the comment thread at the current visual selection.
    ///
    /// Only acts on `CommentListRow::Comment` rows that have replies.
    /// On expand: loads replies from DB, caches them, and rebuilds row list.
    /// On collapse: removes from expanded set and rebuilds.
    pub fn toggle_comment_expansion(&mut self) {
        use crate::review_state::CommentListRow;

        let visual = self.viewer_state.comment_list_selected;
        let row = self.review_state.comment_list_rows.get(visual).cloned();

        let Some(CommentListRow::Comment { comment_idx }) = row else {
            return;
        };

        let Some(comment) = self.review_state.comments.get(comment_idx) else {
            return;
        };

        let reply_count = self
            .review_state
            .reply_counts
            .get(&comment.id)
            .copied()
            .unwrap_or(0);
        if reply_count == 0 {
            return;
        }

        let comment_id = comment.id.clone();

        if self.review_state.expanded_comments.contains(&comment_id) {
            // Collapse.
            self.review_state.expanded_comments.remove(&comment_id);
            self.review_state.rebuild_comment_list_rows();
            // Clamp selection.
            let row_count = self.review_state.comment_list_rows.len();
            if row_count > 0 && self.viewer_state.comment_list_selected >= row_count {
                self.viewer_state.comment_list_selected = row_count - 1;
            }
        } else {
            // Expand — load replies from DB if not cached.
            if !self.review_state.cached_replies.contains_key(&comment_id) {
                if let Some(store) = &self.review_store {
                    match store.get_replies(&comment_id) {
                        Ok(replies) => {
                            self.review_state
                                .cached_replies
                                .insert(comment_id.clone(), replies);
                        }
                        Err(e) => {
                            log::warn!("failed to load replies: {e}");
                            self.set_status(format!("Error loading replies: {e}"), StatusLevel::Error);
                            return;
                        }
                    }
                }
            }
            self.review_state.expanded_comments.insert(comment_id);
            self.review_state.rebuild_comment_list_rows();
        }
    }

    // ── Template helpers ─────────────────────────────────────────

    pub fn delete_review_template(&mut self, id: &str) {
        if let Some(store) = &self.review_store {
            match store.delete_template(id) {
                Ok(()) => {
                    self.review_state.status_message = Some("Template deleted.".to_string());
                }
                Err(e) => {
                    self.review_state.status_message = Some(format!("Error: {e}"));
                }
            }
            self.review_state.load_templates(store);
        }
    }

    // ── Session history helpers ─────────────────────────────────

    pub fn load_session_history(&mut self) {
        if let Some(store) = &self.review_store {
            match store.list_session_history(50) {
                Ok(records) => {
                    self.history.records = records;
                    self.history.selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to load session history: {e}");
                    self.history.records.clear();
                }
            }
        }
    }

    pub fn search_session_history(&mut self) {
        if let Some(store) = &self.review_store {
            let query = self.history.search_query.text().to_string();
            let result = if query.is_empty() {
                store.list_session_history(50)
            } else {
                store.search_session_history(&query)
            };
            match result {
                Ok(records) => {
                    self.history.records = records;
                    self.history.selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to search session history: {e}");
                }
            }
        }
    }

    pub fn save_current_session_history(&mut self) {
        // Try the active Claude session first, then Shell.
        let active_idx = self.terminal.active_claude_session
            .or(self.terminal.active_shell_session);
        let active_idx = match active_idx {
            Some(idx) => idx,
            None => {
                self.set_status("No active PTY session to save.".to_string(), StatusLevel::Warning);
                return;
            }
        };

        let sessions = self.terminal.pty_manager.sessions();
        let session = match sessions.get(active_idx) {
            Some(s) => s,
            None => {
                self.set_status("Session not found.".to_string(), StatusLevel::Error);
                return;
            }
        };

        let session_id = session.id.clone();
        let worktree = session.worktree.clone();
        let label = session.label.clone();
        let kind = match session.kind {
            pty_manager::SessionKind::ClaudeCode => "claude_code",
            pty_manager::SessionKind::Shell => "shell",
        };
        let output = self.terminal.pty_manager.get_output(active_idx).join("\n");

        if let Some(store) = &self.review_store {
            match store.save_session_history(&session_id, &worktree, &label, kind, &output) {
                Ok(()) => {
                    self.status_message = Some(StatusMessage::new("Session history saved.".to_string(), StatusLevel::Success, self.ui_tick));
                    if self.history.active {
                        match store.list_session_history(50) {
                            Ok(records) => {
                                self.history.records = records;
                                self.history.selected = 0;
                            }
                            Err(e) => {
                                log::warn!("failed to reload session history: {e}");
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("failed to save session history: {e}");
                    self.status_message = Some(StatusMessage::new(format!("Error saving history: {e}"), StatusLevel::Error, self.ui_tick));
                }
            }
        }
    }

    // ── Worktree create / delete helpers ──────────────────────────

    /// Select a worktree by its path and trigger UI updates.
    fn select_worktree_by_path(&mut self, path: &std::path::Path) {
        if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
            self.selected_worktree = idx;
            self.on_worktree_changed();
        }
    }

    /// Create a worktree from a base ref (2-step flow) — runs in a background thread.
    pub fn create_worktree_from_base(&mut self, branch_name: &str, base_ref: &str) {
        let base = if base_ref.is_empty() { "origin/main" } else { base_ref };

        let pending = PendingWorktree {
            branch: branch_name.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: base.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            delete_branch_after: false,
            description: String::new(),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(format!("Creating worktree '{branch_name}'..."), StatusLevel::Info);

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();
        let branch = branch_name.to_string();
        let base_owned = base.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.create_worktree_from_base(&branch, &base_owned, wt_dir.as_deref()));
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed { error: format!("{e}"), pending },
            };
            let _ = tx.send(msg);
        });
    }

    /// Create a worktree from a remote branch — runs in a background thread.
    pub fn create_worktree_from_remote(&mut self, remote_branch: &str) {
        let local_branch = remote_branch
            .strip_prefix("origin/")
            .unwrap_or(remote_branch);

        let pending = PendingWorktree {
            branch: local_branch.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: remote_branch.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            delete_branch_after: false,
            description: String::new(),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(format!("Creating worktree '{local_branch}'..."), StatusLevel::Info);

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();
        let remote = remote_branch.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.create_worktree_from_remote(&remote, wt_dir.as_deref()));
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed { error: format!("{e}"), pending },
            };
            let _ = tx.send(msg);
        });
    }

    /// Delete a branch (optionally force).
    pub fn delete_branch(&mut self, name: &str, force: bool) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.delete_branch(name, force) {
                    Ok(()) => {
                        let mode = if force { "force-deleted" } else { "deleted" };
                        self.set_status(format!("Branch {mode}: {name}"), StatusLevel::Success);
                    }
                    Err(e) => {
                        self.set_status(format!("Branch delete error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Execute grab: checkout main to the selected worktree's branch.
    pub fn execute_grab(&mut self, branch_name: &str) {
        // Pre-check: already grabbing another branch
        if let Some(ref grabbed) = self.worktree_mgr.grabbed_branch {
            self.set_status(
                format!(
                    "Already grabbed: {}. Ungrab first (Y).",
                    grabbed.branch
                ),
                StatusLevel::Warning,
            );
            return;
        }

        let main_path = match self.worktrees.iter().find(|w| w.is_main) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("Main worktree not found.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let source_path = match self.worktrees.iter().find(|w| w.branch == branch_name) {
            Some(w) => w.path.clone(),
            None => {
                self.set_status(
                    format!("Worktree for '{branch_name}' not found."),
                    StatusLevel::Error,
                );
                return;
            }
        };
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.grab_branch(&main_path, &source_path, branch_name) {
                    Ok(()) => {
                        self.worktree_mgr.grabbed_branch = Some(GrabbedBranch {
                            branch: branch_name.to_string(),
                            source_worktree: source_path,
                        });
                        self.set_status(
                            format!("Grabbed '{branch_name}' — main is now on this branch. Press Y to ungrab."),
                            StatusLevel::Success,
                        );
                        self.refresh_worktrees();
                    }
                    Err(e) => {
                        self.set_status(format!("Grab error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Execute ungrab: return main to main branch, restore worktree to original branch.
    pub fn execute_ungrab(&mut self) {
        let grabbed = match self.worktree_mgr.grabbed_branch.clone() {
            Some(g) => g,
            None => {
                self.set_status("Not grabbing any branch.".to_string(), StatusLevel::Warning);
                return;
            }
        };
        let main_path = match self.worktrees.iter().find(|w| w.is_main) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("Main worktree not found.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let main_branch = self.config.general.main_branch.clone();
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.ungrab_branch(
                    &main_path,
                    &grabbed.source_worktree,
                    &grabbed.branch,
                    &main_branch,
                ) {
                    Ok(()) => {
                        let branch = grabbed.branch.clone();
                        self.worktree_mgr.grabbed_branch = None;
                        self.set_status(
                            format!("Ungrabbed '{branch}' — main restored."),
                            StatusLevel::Success,
                        );
                        self.refresh_worktrees();
                    }
                    Err(e) => {
                        self.set_status(format!("Ungrab error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Prune all stale worktrees.
    pub fn execute_prune(&mut self) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                let mut pruned = 0;
                for name in &self.prune.stale {
                    match engine.prune_stale_worktree(name) {
                        Ok(()) => pruned += 1,
                        Err(e) => {
                            log::warn!("failed to prune worktree '{name}': {e}");
                        }
                    }
                }
                self.set_status(format!("Pruned {pruned} stale worktree(s)."), StatusLevel::Success);
                self.prune.stale.clear();
                self.refresh_worktrees();
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Load remote branches for the switch overlay.
    ///
    /// Immediately populates the list from cached refs, then kicks off a
    /// background fetch. When the fetch completes, `poll_bg_branches()`
    /// picks up the refreshed list so the overlay updates without blocking.
    pub fn load_switch_branches(&mut self) {
        // Show cached refs instantly.
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_remote_branches() {
                    Ok(branches) => {
                        self.switch_branch.branches = branches;
                        self.switch_branch.selected = 0;
                        self.switch_branch.filter.clear();
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.switch_branch.branches.clear();
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
                return;
            }
        }

        // Fetch in background and send updated branch list back.
        let repo_path = self.repo_path.clone();
        self.bg_branch_op.start(move |tx| {
            let engine = match git_engine::GitEngine::open(&repo_path) {
                Ok(e) => e,
                Err(err) => {
                    log::warn!("bg fetch: failed to open repo: {err}");
                    return;
                }
            };
            if let Err(e) = engine.fetch_origin() {
                log::warn!("bg fetch failed: {e}");
            }
            match engine.list_remote_branches() {
                Ok(branches) => { let _ = tx.send(branches); }
                Err(e) => { log::warn!("bg list_remote_branches failed: {e}"); }
            }
        });
    }

    /// Check whether the background fetch has finished and update the
    /// switch-branch list if new data is available. Non-blocking.
    pub fn poll_bg_branches(&mut self) {
        if let Some(branches) = self.bg_branch_op.poll() {
            // Preserve the user's current filter/selection as best we can.
            let prev_selected_name = self.filtered_switch_branches()
                .get(self.switch_branch.selected)
                .map(|(_, name)| (*name).clone());
            self.switch_branch.branches = branches;
            // Try to restore selection by name.
            if let Some(name) = prev_selected_name {
                if let Some(pos) = self.filtered_switch_branches()
                    .iter()
                    .position(|(_, b)| **b == name)
                {
                    self.switch_branch.selected = pos;
                }
            }
            self.bg_branch_op.clear();
        }
    }

    // ── Pull worktree (fetch + fast-forward) ──────────────────────────

    /// Start a background pull (fetch + fast-forward) for the selected worktree.
    pub fn start_pull_worktree(&mut self) {
        if self.bg_pull_op.is_running() {
            self.set_status("A pull is already in progress.".to_string(), StatusLevel::Warning);
            return;
        }

        let wt = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt,
            None => return,
        };

        let branch = wt.branch.clone();
        let wt_path = wt.path.clone();
        let repo_path = self.repo_path.clone();

        self.set_status(format!("Pulling '{branch}'..."), StatusLevel::Info);

        self.bg_pull_op.start(move |tx| {
            let result = (|| -> Result<String, String> {
                let engine = git_engine::GitEngine::open(&repo_path)
                    .map_err(|e| format!("Failed to open repo: {e}"))?;
                engine.pull_worktree(&wt_path)
                    .map_err(|e| format!("{e}"))
            })();
            let _ = tx.send(result);
        });
    }

    /// Poll the background pull channel. Non-blocking.
    pub fn poll_bg_pull(&mut self) {
        if let Some(result) = self.bg_pull_op.poll() {
            match result {
                Ok(msg) => {
                    let level = if msg.contains("up-to-date") {
                        StatusLevel::Info
                    } else if msg.contains("fast-forward") {
                        StatusLevel::Success
                    } else {
                        StatusLevel::Warning
                    };
                    self.set_status(msg, level);
                    self.refresh_worktrees();
                }
                Err(err) => {
                    self.set_status(format!("Pull failed: {err}"), StatusLevel::Error);
                }
            }
        }
    }

    // ── Async worktree operations ──────────────────────────────────────

    /// Poll for completed background worktree create/delete results.
    pub fn poll_worktree_ops(&mut self) {
        let rx = match self.worktree_mgr.bg_worktree_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        let mut results = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(result) => results.push(result),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worktree_mgr.bg_worktree_rx = None;
                    self.worktree_mgr.bg_worktree_tx = None;
                    break;
                }
            }
        }
        for result in results {
            self.handle_worktree_op_result(result);
        }
    }

    fn handle_worktree_op_result(&mut self, result: WorktreeOpResult) {
        match result {
            WorktreeOpResult::Created { path, pending } => {
                // Remove from pending list (matches both Creating and SmartCreating).
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });

                self.record_stat("branches_created");
                if let Some(store) = &self.review_store {
                    let _ = store.save_worktree_base_branch(&pending.branch, &pending.base_ref);
                }
                self.refresh_worktrees();
                self.select_worktree_by_path(&path);
                self.set_status(
                    format!("Created worktree: {} (from {})", path.display(), pending.base_ref),
                    StatusLevel::Success,
                );

                // Smart Worktree: auto-spawn Claude Code and pre-type prompt.
                if pending.auto_spawn {
                    match self.spawn_claude_code() {
                        Ok(idx) => {
                            if !pending.smart_prompt.is_empty() {
                                let _ = self.terminal.pty_manager.write_to_session(idx, pending.smart_prompt.as_bytes());
                            }
                            self.set_focus(Focus::TerminalClaude);
                        }
                        Err(e) => {
                            log::warn!("Failed to auto-spawn Claude Code: {e}");
                        }
                    }
                }
            }
            WorktreeOpResult::CreateFailed { error, pending } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::Deleted { ref branch } => {
                let delete_branch_after = self.worktree_mgr.pending_worktrees.iter().any(|p| {
                    p.op == PendingWorktreeOp::Deleting && p.branch == *branch && p.delete_branch_after
                });
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch)
                });
                self.refresh_worktrees();
                self.set_status(format!("Deleted worktree: {branch}"), StatusLevel::Success);

                if delete_branch_after {
                    self.delete_branch(branch, true);
                }
            }
            WorktreeOpResult::DeleteFailed { error, ref branch } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch)
                });
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::Skipped { ref branch, ref reason } => {
                self.worktree_mgr.pending_worktrees.retain(|p| p.branch != *branch);
                self.worktree_mgr.skip_reason = Some(reason.clone());
            }
            WorktreeOpResult::SmartBranchResolved { ref description, ref branch, ref prompt } => {
                // Update the pending entry: set branch name and prompt.
                for p in &mut self.worktree_mgr.pending_worktrees {
                    if p.op == PendingWorktreeOp::SmartCreating && p.description == *description {
                        p.branch = branch.clone();
                        p.smart_prompt = prompt.clone();
                        break;
                    }
                }
                self.set_status(
                    format!("Smart worktree: creating '{branch}'..."),
                    StatusLevel::Info,
                );
            }
            WorktreeOpResult::SmartFailed { ref description, ref error } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::SmartCreating && p.description == *description)
                });
                log::warn!("Smart worktree failed: {error}");
                self.set_status(
                    format!("Smart worktree failed: {error}"),
                    StatusLevel::Error,
                );
            }
        }
    }

    // ── Smart Worktree generation ──────────────────────────────────────

    /// Run LLM generation + worktree creation asynchronously in a single background thread.
    pub fn start_smart_worktree_async(&mut self, description: &str) {
        let desc = description.to_string();
        let main_branch = self.config.general.main_branch.clone();
        let base_ref = format!("origin/{main_branch}");
        let repo_path = self.repo_path.clone();
        let wt_dir = self.config.general.worktree_dir.clone();

        // Add pending entry with empty branch (will be updated when LLM resolves).
        let pending = PendingWorktree {
            branch: String::new(),
            op: PendingWorktreeOp::SmartCreating,
            base_ref: base_ref.clone(),
            worktree_path: None,
            auto_spawn: true,
            smart_prompt: String::new(),
            delete_branch_after: false,
            description: desc.clone(),
        };
        self.worktree_mgr.pending_worktrees.push(pending);
        self.set_status("Smart worktree: generating...".to_string(), StatusLevel::Info);

        let tx = self.worktree_op_sender();

        std::thread::spawn(move || {
            // Phase 1: LLM generation.
            let gen_result = match run_smart_generation(&desc) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(WorktreeOpResult::SmartFailed {
                        description: desc,
                        error: e,
                    });
                    return;
                }
            };

            if gen_result.branch.is_empty() {
                let _ = tx.send(WorktreeOpResult::SmartFailed {
                    description: desc,
                    error: "LLM returned empty branch name".to_string(),
                });
                return;
            }

            let branch = gen_result.branch.clone();
            let prompt = gen_result.prompt.clone();

            // Report branch resolved (for UI update).
            let _ = tx.send(WorktreeOpResult::SmartBranchResolved {
                description: desc.clone(),
                branch: branch.clone(),
                prompt: prompt.clone(),
            });

            // Phase 2: Create worktree.
            let pending = PendingWorktree {
                branch: branch.clone(),
                op: PendingWorktreeOp::SmartCreating,
                base_ref: base_ref.clone(),
                worktree_path: None,
                auto_spawn: true,
                smart_prompt: prompt,
                delete_branch_after: false,
                description: desc,
            };
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.create_worktree_from_base(&branch, &base_ref, wt_dir.as_deref()));
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed { error: format!("{e}"), pending },
            };
            let _ = tx.send(msg);
        });
    }

    /// Schedule an incremental grep search with debounce (200ms).
    ///
    /// Called on every keystroke that modifies the query. Sets a deadline;
    /// `check_grep_debounce()` fires the actual search when the deadline passes.
    pub fn schedule_grep_search(&mut self) {
        let query = self.grep_search.query.text().to_string();
        if query.is_empty() {
            // Clear everything immediately.
            self.grep_search.results.clear();
            self.grep_search.selected = 0;
            self.grep_search.scroll = 0;
            self.grep_search.running = false;
            self.grep_search.bg_op.clear();
            self.grep_search.bg_op_phase2.clear();
            self.grep_search.debounce_deadline = None;
            self.grep_search.phase1_active = false;
            return;
        }
        self.grep_search.debounce_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
    }

    /// Check if the debounce deadline has passed; if so, start the search.
    /// Returns `true` if a search was started (caller should trigger redraw).
    pub fn check_grep_debounce(&mut self) -> bool {
        if let Some(deadline) = self.grep_search.debounce_deadline {
            if std::time::Instant::now() >= deadline {
                self.grep_search.debounce_deadline = None;
                self.start_incremental_grep_search();
                return true;
            }
        }
        false
    }

    /// Start an incremental grep search.
    ///
    /// For short queries (≤3 chars), uses 2-phase search:
    ///   phase1 — search only recently modified files (fast)
    ///   phase2 — full search (runs in parallel, replaces phase1 results)
    /// For longer queries, runs only a full search.
    fn start_incremental_grep_search(&mut self) {
        let query = self.grep_search.query.text().to_string();
        if query.is_empty() {
            return;
        }

        let wt_path = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => return,
        };

        // Cancel any previous search.
        self.grep_search.bg_op.clear();
        self.grep_search.bg_op_phase2.clear();

        // Reset results.
        self.grep_search.results.clear();
        self.grep_search.selected = 0;
        self.grep_search.scroll = 0;
        self.grep_search.running = true;

        let regex_mode = self.grep_search.regex_mode;
        let case_sensitive = self.grep_search.case_sensitive;

        if query.chars().count() <= 3 {
            // 2-phase search for short queries.
            self.grep_search.phase1_active = true;

            // Get recently modified files (synchronous, fast).
            let recent_files = crate::git_engine::recently_modified_files(&wt_path, 200)
                .unwrap_or_default();

            // Phase1: search only recent files.
            if !recent_files.is_empty() {
                let wt1 = wt_path.clone();
                let q1 = query.clone();
                let files1 = recent_files;
                self.grep_search.bg_op.start(move |tx| {
                    crate::grep_search::run_search_files(&wt1, &q1, regex_mode, case_sensitive, files1, tx);
                });
            }

            // Phase2: full search (runs in parallel).
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.grep_search.bg_op_phase2.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        } else {
            // Single-phase full search for longer queries.
            self.grep_search.phase1_active = false;
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.grep_search.bg_op.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        }
    }

    /// Poll for background grep search results.
    pub fn poll_grep_search(&mut self) {
        // Poll phase1 / single-phase bg_op.
        let messages = self.grep_search.bg_op.poll_all();
        for msg in messages {
            match msg {
                GrepProgress::Results(batch) => {
                    self.grep_search.results.extend(batch);
                }
                GrepProgress::Done(total) => {
                    // If phase1 completed but phase2 is still running, keep running = true.
                    if !self.grep_search.phase1_active || !self.grep_search.bg_op_phase2.is_running() {
                        self.grep_search.running = false;
                        self.grep_search.bg_op.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    } else {
                        self.grep_search.bg_op.clear();
                    }
                }
                GrepProgress::Error(msg) => {
                    self.grep_search.running = false;
                    self.grep_search.bg_op.clear();
                    self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                    return;
                }
            }
        }

        // Poll phase2 bg_op.
        if self.grep_search.phase1_active {
            let messages2 = self.grep_search.bg_op_phase2.poll_all();
            let mut got_phase2_results = false;
            for msg in messages2 {
                match msg {
                    GrepProgress::Results(batch) => {
                        if !got_phase2_results {
                            // Replace phase1 results with phase2 results.
                            self.grep_search.results.clear();
                            self.grep_search.selected = 0;
                            self.grep_search.scroll = 0;
                            self.grep_search.phase1_active = false;
                            got_phase2_results = true;
                        }
                        self.grep_search.results.extend(batch);
                    }
                    GrepProgress::Done(total) => {
                        if !got_phase2_results {
                            // Phase2 done with no results — clear phase1 results too
                            // only if phase1 also had no results; otherwise keep phase1.
                            self.grep_search.phase1_active = false;
                        }
                        self.grep_search.running = false;
                        self.grep_search.bg_op_phase2.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    }
                    GrepProgress::Error(msg) => {
                        self.grep_search.phase1_active = false;
                        self.grep_search.running = false;
                        self.grep_search.bg_op_phase2.clear();
                        self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                        return;
                    }
                }
            }
        }
    }

    /// Return the filtered list of switch branches based on the current filter.
    pub fn filtered_switch_branches(&self) -> Vec<(usize, &String)> {
        if self.switch_branch.filter.is_empty() {
            self.switch_branch.branches.iter().enumerate().collect()
        } else {
            let filter_lower = self.switch_branch.filter.to_lowercase();
            self.switch_branch.branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// Load branches available as base for worktree creation.
    /// Lists remote branches and pre-selects `origin/<main_branch>`.
    pub fn load_base_branches(&mut self) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_remote_branches() {
                    Ok(branches) => {
                        self.worktree_mgr.base_branch_list = branches;
                        self.worktree_mgr.base_branch_selected = 0;
                        self.worktree_mgr.base_branch_filter.clear();
                        // Pre-select origin/<main_branch> if it exists.
                        let default_base = format!("origin/{}", self.config.general.main_branch);
                        if let Some(pos) = self.worktree_mgr.base_branch_list.iter().position(|b| b == &default_base) {
                            self.worktree_mgr.base_branch_selected = pos;
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.worktree_mgr.base_branch_list.clear();
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Return the filtered list of base branches based on the current filter.
    pub fn filtered_base_branches(&self) -> Vec<(usize, &String)> {
        if self.worktree_mgr.base_branch_filter.is_empty() {
            self.worktree_mgr.base_branch_list.iter().enumerate().collect()
        } else {
            let filter_lower = self.worktree_mgr.base_branch_filter.to_lowercase();
            self.worktree_mgr.base_branch_list
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// Load grab branch candidates (non-main worktree branches).
    pub fn load_grab_branches(&mut self) {
        self.grab.branches = self.worktrees
            .iter()
            .filter(|w| !w.is_main)
            .map(|w| w.branch.clone())
            .collect();
        self.grab.selected = 0;
    }

    pub fn delete_selected_worktree(&mut self, delete_branch_after: bool) {
        let wt = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt,
            None => return,
        };

        if wt.is_main {
            self.set_status("Cannot delete the main worktree.".to_string(), StatusLevel::Error);
            return;
        }

        let wt_path = wt.path.clone();
        let branch = wt.branch.clone();

        // Kill all PTY sessions (Claude Code + Shell) associated with this worktree
        // before removing the worktree directory. Walk backwards so removals don't
        // shift indices we haven't processed yet.
        let session_indices: Vec<usize> = self
            .terminal.pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path)
            .map(|(idx, _)| idx)
            .collect();
        for &idx in session_indices.iter().rev() {
            log::info!(
                "killing PTY session {idx} for deleted worktree '{branch}'"
            );
            self.close_terminal_session(idx);
        }

        // Add pending entry and run git removal in a background thread.
        let pending = PendingWorktree {
            branch: branch.clone(),
            op: PendingWorktreeOp::Deleting,
            base_ref: String::new(),
            worktree_path: Some(wt_path.clone()),
            auto_spawn: false,
            smart_prompt: String::new(),
            delete_branch_after,
            description: String::new(),
        };
        self.worktree_mgr.pending_worktrees.push(pending);
        self.set_status(format!("Deleting worktree '{branch}'..."), StatusLevel::Info);

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.remove_worktree(&wt_path));
            let msg = match result {
                Ok(()) => WorktreeOpResult::Deleted { branch },
                Err(e) => WorktreeOpResult::DeleteFailed { error: format!("{e}"), branch },
            };
            let _ = tx.send(msg);
        });
    }

    // ── Cherry-pick helpers ────────────────────────────────────────────

    pub fn load_cherry_pick_commits(&mut self) {
        let branch = self.cherry_pick.source_branch.clone();
        if branch.is_empty() {
            self.cherry_pick.commits.clear();
            return;
        }
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_branch_commits(&branch, 20) {
                    Ok(commits) => {
                        self.cherry_pick.commits = commits;
                        self.cherry_pick.selected = 0;
                    }
                    Err(e) => {
                        log::warn!("failed to list commits for branch '{branch}': {e}");
                        self.cherry_pick.commits.clear();
                    }
                }
            }
            Err(e) => {
                log::warn!("failed to open git repository for cherry-pick: {e}");
                self.cherry_pick.commits.clear();
            }
        }
    }

    pub fn execute_cherry_pick(&mut self) {
        let commit = match self.cherry_pick.commits.get(self.cherry_pick.selected) {
            Some(c) => c.clone(),
            None => {
                self.set_status("No commit selected.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let wt_path = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("No worktree selected.".to_string(), StatusLevel::Error);
                return;
            }
        };

        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.cherry_pick_to_worktree(&wt_path, &commit.oid) {
                    Ok(msg) => {
                        self.set_status(msg, StatusLevel::Success);
                        self.refresh_worktrees();
                    }
                    Err(e) => {
                        self.set_status(format!("Cherry-pick error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Called when the selected worktree changes — refreshes viewer, diff, sessions.
    pub fn on_worktree_changed(&mut self) {
        self.viewer_state = ViewerState::default();
        self.refresh_viewer();
        self.refresh_diff();
        self.refresh_reviews();

        // Snapshot baseline so the next poll cycle doesn't trigger a redundant refresh.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            self.last_poll_head_oid = self.worktree_heads.get(&wt.branch).cloned();
            self.last_poll_status = Some((wt.added, wt.modified, wt.deleted));
        }

        // Update active sessions to match the new worktree.
        let wt_name = self.selected_worktree_branch();
        let claude_sessions = self.current_worktree_claude_sessions();
        self.terminal.active_claude_session = claude_sessions.first().map(|(idx, _)| *idx);
        let shell_sessions = self.current_worktree_shell_sessions();
        self.terminal.active_shell_session = shell_sessions.first().map(|(idx, _)| *idx);

        // Activate the PTY sessions.
        if let Some(idx) = self.terminal.active_claude_session {
            self.terminal.pty_manager.activate_session(idx);
        }
        if let Some(idx) = self.terminal.active_shell_session {
            self.terminal.pty_manager.activate_session(idx);
        }

        self.terminal.scroll_claude = 0;
        self.terminal.scroll_shell = 0;
        self.terminal.cache_claude = Default::default();
        self.terminal.cache_shell = Default::default();

        self.compute_branch_details();
        self.set_status(format!("Switched to worktree: {wt_name}"), StatusLevel::Success);
    }

    // ── Branch details (worktree detail panel) ───────────────────

    /// Check whether the `gh` CLI is available on this system.
    fn check_gh_available() -> bool {
        std::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Get (or lazily create) a sender for worktree operation results.
    fn worktree_op_sender(&mut self) -> mpsc::Sender<WorktreeOpResult> {
        if self.worktree_mgr.bg_worktree_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.worktree_mgr.bg_worktree_tx = Some(tx);
            self.worktree_mgr.bg_worktree_rx = Some(rx);
        }
        self.worktree_mgr.bg_worktree_tx.as_ref().unwrap().clone()
    }

    /// Check if a worktree at the given path is pending deletion.
    pub fn is_worktree_pending_delete(&self, path: &Path) -> bool {
        self.worktree_mgr.pending_worktrees.iter().any(|p| {
            p.op == PendingWorktreeOp::Deleting && p.worktree_path.as_deref() == Some(path)
        })
    }

    /// Compute branch lineage and start PR URL lookup for the selected worktree.
    pub fn compute_branch_details(&mut self) {
        let Some(wt) = self.worktrees.get(self.selected_worktree) else {
            self.branch_details = Default::default();
            return;
        };
        let branch = wt.branch.clone();
        let is_main = wt.is_main;

        // Collect active worktree branch names as candidates.
        let worktree_branches: Vec<String> = self
            .worktrees
            .iter()
            .filter(|w| !w.is_main && w.branch != branch)
            .map(|w| w.branch.clone())
            .collect();

        let mut details = git_engine::BranchDetails::default();

        if !is_main {
            // Parent branch: check DB first, fall back to reflog/merge-base heuristic.
            if let Some(store) = &self.review_store {
                if let Ok(Some(base)) = store.get_worktree_base_branch(&branch) {
                    details.initial_branch = Some(base);
                }
            }
            if details.initial_branch.is_none() {
                if let Ok(engine) = git_engine::GitEngine::open(&self.repo_path) {
                    details.initial_branch = engine.detect_parent_branch(
                        &branch,
                        &self.config.general.main_branch,
                        &worktree_branches,
                    );
                }
            }
        }

        // Derived (fork) branches: check DB first, fall back to git heuristic.
        let mut db_children = Vec::new();
        if let Some(store) = &self.review_store {
            if let Ok(children) = store.get_worktree_children(&branch) {
                db_children = children;
            }
        }

        // Filter DB children to only those that are currently active worktree branches.
        let active_branches: std::collections::HashSet<&str> =
            self.worktrees.iter().map(|w| w.branch.as_str()).collect();

        let filtered_children: Vec<String> = db_children
            .into_iter()
            .filter(|c| active_branches.contains(c.as_str()))
            .collect();

        if !filtered_children.is_empty() {
            details.derived_branches = filtered_children;
        } else if let Ok(engine) = git_engine::GitEngine::open(&self.repo_path) {
            let main = &self.config.general.main_branch;
            if let Ok(derived) =
                engine.find_derived_branches(&branch, main, &worktree_branches)
            {
                details.derived_branches = derived;
            }
        }

        // PR URL (non-main only).
        if !is_main && self.gh_available {
            details.pr_loading = true;
            self.start_pr_url_lookup(&branch);
        }

        self.branch_details = details;
    }

    /// Spawn a background thread to look up the PR URL via `gh pr view`.
    fn start_pr_url_lookup(&mut self, branch: &str) {
        let branch = branch.to_string();
        let repo_path = self.repo_path.clone();

        self.bg_pr_url_op.start(move |tx| {
            let result = std::process::Command::new("gh")
                .args(["pr", "view", "--head", &branch, "--json", "url", "-q", ".url"])
                .current_dir(&repo_path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if url.is_empty() { None } else { Some(url) }
                    } else {
                        None
                    }
                });
            let _ = tx.send(result);
        });
    }

    /// Poll the background PR URL lookup for a result.
    pub fn poll_pr_url(&mut self) {
        if let Some(result) = self.bg_pr_url_op.poll() {
            self.branch_details.pr_url = result;
            self.branch_details.pr_loading = false;
        }
    }

    // ── Open PR in browser ───────────────────────────────────────

    /// Open the pull-request page for the selected worktree's branch in the
    /// default web browser.
    pub fn open_pr_in_browser(&mut self) {
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status("No worktree selected.".to_string(), StatusLevel::Warning);
            return;
        }

        match crate::git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => match engine.pr_url_for_branch(&branch) {
                Some(url) => {
                    log::info!("Opening PR URL: {url}");
                    if let Err(e) = open::that(&url) {
                        self.set_status(format!("Failed to open browser: {e}"), StatusLevel::Error);
                    } else {
                        self.set_status(format!("Opened PR for '{branch}'"), StatusLevel::Success);
                    }
                }
                None => {
                    self.set_status("Could not determine remote URL.".to_string(), StatusLevel::Error);
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    // ── Public accessor helpers ─────────────────────────────────────

    /// Return the branch name used as the worktree identifier.
    pub fn selected_worktree_branch(&self) -> String {
        self.worktrees
            .get(self.selected_worktree)
            .map(|w| w.branch.clone())
            .unwrap_or_default()
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
fn perform_update(tx: &mpsc::Sender<UpdateProgress>, version: &str, tarball_url: &str) {
    use std::process::Command;

    let tmpdir = std::env::temp_dir().join(format!("conductor-update-{version}"));
    let _ = std::fs::remove_dir_all(&tmpdir);
    if std::fs::create_dir_all(&tmpdir).is_err() {
        let _ = tx.send(UpdateProgress::Error("Failed to create temp directory".to_string()));
        return;
    }

    // Resolve tarball URL — if empty, re-fetch from API.
    let url = if tarball_url.is_empty() {
        let _ = tx.send(UpdateProgress::Status("Fetching release info...".to_string()));
        match crate::update_checker::check_for_update() {
            Some(info) if !info.tarball_url.is_empty() => info.tarball_url,
            _ => {
                let _ = tx.send(UpdateProgress::Error("Could not find tarball URL".to_string()));
                let _ = std::fs::remove_dir_all(&tmpdir);
                return;
            }
        }
    } else {
        tarball_url.to_string()
    };

    // Download.
    let _ = tx.send(UpdateProgress::Status(format!("Downloading v{version}...")));
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
            let _ = std::fs::remove_dir_all(&tmpdir);
            return;
        }
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let _ = tx.send(UpdateProgress::Error(format!("Download failed: {stderr}")));
            let _ = std::fs::remove_dir_all(&tmpdir);
            return;
        }
        _ => {}
    }

    // Extract.
    let _ = tx.send(UpdateProgress::Status("Extracting...".to_string()));
    let extract = Command::new("tar")
        .args(["xzf", "source.tar.gz"])
        .current_dir(&tmpdir)
        .output();
    match extract {
        Err(e) => {
            let _ = tx.send(UpdateProgress::Error(format!("tar not found: {e}")));
            let _ = std::fs::remove_dir_all(&tmpdir);
            return;
        }
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let _ = tx.send(UpdateProgress::Error(format!("Extraction failed: {stderr}")));
            let _ = std::fs::remove_dir_all(&tmpdir);
            return;
        }
        _ => {}
    }

    // Find the extracted directory (GitHub tarballs extract to owner-repo-hash/).
    let src_dir = match std::fs::read_dir(&tmpdir) {
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
                    let _ = tx.send(UpdateProgress::Error("No source directory found in tarball".to_string()));
                    let _ = std::fs::remove_dir_all(&tmpdir);
                    return;
                }
            }
        }
        Err(e) => {
            let _ = tx.send(UpdateProgress::Error(format!("Failed to read temp dir: {e}")));
            let _ = std::fs::remove_dir_all(&tmpdir);
            return;
        }
    };

    // Build & install.
    let _ = tx.send(UpdateProgress::Status(format!("Building v{version}... (this may take a while)")));
    let build = Command::new("make")
        .arg("install")
        .current_dir(&src_dir)
        .output();
    match build {
        Err(e) => {
            let _ = tx.send(UpdateProgress::Error(format!("make not found: {e}")));
            let _ = std::fs::remove_dir_all(&tmpdir);
            return;
        }
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let msg = if stderr.len() > 200 {
                format!("Build failed: ...{}", &stderr[stderr.len() - 200..])
            } else {
                format!("Build failed: {stderr}")
            };
            let _ = tx.send(UpdateProgress::Error(msg));
            let _ = std::fs::remove_dir_all(&tmpdir);
            return;
        }
        _ => {}
    }

    // Clean up.
    let _ = std::fs::remove_dir_all(&tmpdir);

    let _ = tx.send(UpdateProgress::Done(format!("v{version} installed successfully! Restarting...")));
}
