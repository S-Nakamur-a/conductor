//! App state and focus management.
//!
//! This module defines the top-level application state, the unified panel
//! layout focus model, and transitions between panels.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::config;
use crate::diff_state::{DiffState, DiffViewMode};
use crate::git_engine;
use crate::pty_manager;
use crate::review_state::ReviewState;
use crate::review_store::{self, Author, CommentKind, ReviewStore};
use crate::theme::Theme;
use crate::viewer_state::ViewerState;

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
    /// Confirming unsync / reset (y/n).
    ConfirmingUnsync,
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
    /// PTY session manager.
    pub pty_manager: pty_manager::PtyManager,
    /// Whether a worktree creation dialog is showing.
    pub worktree_input_mode: WorktreeInputMode,
    /// Text buffer for worktree name input.
    pub worktree_input_buffer: String,
    /// Status message (flash message) shown in the status bar.
    pub status_message: Option<StatusMessage>,
    /// Files detected as changed by Claude Code output analysis.
    pub cc_changed_files: Vec<String>,
    /// Worktree paths whose Claude Code sessions are waiting for user input.
    pub cc_waiting_worktrees: HashSet<PathBuf>,
    /// Whether the session history viewer is active.
    pub history_active: bool,
    /// Session history records loaded from the database.
    pub history_records: Vec<review_store::SessionHistory>,
    /// Index of the selected history record.
    pub history_selected: usize,
    /// Search query for session history.
    pub history_search_query: String,
    /// Whether the history search input is active.
    pub history_search_active: bool,
    /// Whether the cherry-pick picker UI is active.
    pub cherry_pick_active: bool,
    /// Source branch for cherry-pick (selected from another worktree).
    pub cherry_pick_source_branch: String,
    /// List of commits from the source branch.
    pub cherry_pick_commits: Vec<git_engine::CommitInfo>,
    /// Index of the selected commit in the picker.
    pub cherry_pick_selected: usize,
    /// List of known repository paths (including the current one).
    pub repo_list: Vec<std::path::PathBuf>,
    /// Index of the currently active repository in repo_list.
    pub repo_list_index: usize,
    /// Whether the repo selector overlay is active.
    pub repo_selector_active: bool,
    /// Selected index in the repo selector.
    pub repo_selector_selected: usize,
    /// Whether the "open repository" path input is active.
    pub open_repo_active: bool,
    /// Text buffer for the "open repository" path input.
    pub open_repo_buffer: String,
    /// Last known terminal content area size (rows, cols) for Claude PTY.
    pub terminal_size_claude: (u16, u16),
    /// Last known terminal content area size (rows, cols) for Shell PTY.
    pub terminal_size_shell: (u16, u16),
    /// Set to `true` when a full terminal clear + redraw is needed.
    pub needs_clear: bool,
    /// Index of the active Claude Code session for the current worktree (into pty_manager.sessions).
    pub active_claude_session: Option<usize>,
    /// Index of the active Shell session for the current worktree (into pty_manager.sessions).
    pub active_shell_session: Option<usize>,

    // ── Create worktree 2-step flow ─────────────────────────────
    /// Branch name entered in step 1, held while step 2 (base branch) is active.
    pub worktree_pending_branch: String,
    /// Full list of branches available as base for worktree creation.
    pub base_branch_list: Vec<String>,
    /// Currently selected index in the base branch picker.
    pub base_branch_selected: usize,
    /// Filter string for narrowing the base branch list.
    pub base_branch_filter: String,

    // ── Switch (remote branch checkout) ─────────────────────────
    /// Whether the switch-branch overlay is active.
    pub switch_branch_active: bool,
    /// Full list of remote branches.
    pub switch_branch_list: Vec<String>,
    /// Currently selected index in the switch list.
    pub switch_branch_selected: usize,
    /// Filter string for narrowing the switch branch list.
    pub switch_branch_filter: String,

    // ── Sync (merge branch for testing) ─────────────────────────
    /// Whether the sync overlay is active.
    pub sync_active: bool,
    /// List of local branch names available for sync.
    pub sync_branches: Vec<String>,
    /// Currently selected index in the sync list.
    pub sync_selected: usize,

    // ── Prune ───────────────────────────────────────────────────
    /// Whether the prune overlay is active.
    pub prune_active: bool,
    /// List of stale worktree names found.
    pub prune_stale: Vec<String>,

    // ── Delete flow (branch deletion after worktree removal) ────
    /// Branch name pending deletion after worktree was removed.
    pub worktree_pending_delete_branch: String,

    // ── Resume Claude session overlay ─────────────────────────
    /// Whether the resume-session picker overlay is active.
    pub resume_session_active: bool,
    /// List of resumable Claude Code sessions.
    pub resume_sessions: Vec<crate::claude_sessions::ResumableSession>,
    /// Currently selected index in the resume session list.
    pub resume_session_selected: usize,
    /// Filter string for narrowing the resume session list.
    pub resume_session_filter: String,
    /// Whether to show sessions from all projects (true) or only current repo (false).
    pub resume_session_all_projects: bool,

    // ── Syntax highlighting (syntect) ──────────────────────────
    /// Shared syntect syntax definitions.
    pub syntax_set: SyntaxSet,
    /// Active syntect highlighting theme.
    pub syntect_theme: syntect::highlighting::Theme,

    // ── Help overlay ─────────────────────────────────────────────
    /// Whether the help overlay is visible.
    pub help_active: bool,
    /// Which panel's help to display (captured at the moment `?` was pressed).
    pub help_context: Focus,

    /// Whether the terminal column is manually expanded (via the [<=>] button).
    pub terminal_expanded: bool,

    // ── Command palette overlay ─────────────────────────────────
    /// Whether the command palette is open.
    pub command_palette_active: bool,
    /// Filter string for narrowing the command list.
    pub command_palette_filter: String,
    /// Currently selected index in the filtered command list.
    pub command_palette_selected: usize,

    /// Frame counter for UI animations (e.g. waiting-state pulse).
    pub ui_tick: u64,

    /// Notification bar badge positions: (start_col, end_col, branch_name).
    /// Populated during rendering for click-to-jump.
    pub notification_bar_badges: Vec<(u16, u16, String)>,

    /// Scrollback offset for the Claude Code terminal (0 = live view at bottom).
    pub terminal_scroll_claude: usize,
    /// Scrollback offset for the Shell terminal (0 = live view at bottom).
    pub terminal_scroll_shell: usize,

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

    // ── Background fetch for switch-branch overlay ──────────────
    /// Receiver for branch lists fetched in the background.
    pub bg_branch_rx: Option<mpsc::Receiver<Vec<String>>>,
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
        let syntax_set = SyntaxSet::load_defaults_newlines();
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

        let theme = Theme::from_name(&config.viewer.theme);

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

        let mut app = Self {
            focus: Focus::Worktree,
            repo_path,
            main_repo_name,
            should_quit: false,
            selected_worktree: 0,
            worktrees: Vec::new(),
            config,
            theme,
            viewer_state: ViewerState::default(),
            diff_state,
            review_store,
            review_state: ReviewState::new(),
            pty_manager: pty_manager::PtyManager::new(),
            worktree_input_mode: WorktreeInputMode::Normal,
            worktree_input_buffer: String::new(),
            status_message: None,
            cc_changed_files: Vec::new(),
            cc_waiting_worktrees: HashSet::new(),
            history_active: false,
            history_records: Vec::new(),
            history_selected: 0,
            history_search_query: String::new(),
            history_search_active: false,
            cherry_pick_active: false,
            cherry_pick_source_branch: String::new(),
            cherry_pick_commits: Vec::new(),
            cherry_pick_selected: 0,
            repo_list,
            repo_list_index: 0,
            repo_selector_active: false,
            repo_selector_selected: 0,
            open_repo_active: false,
            open_repo_buffer: String::new(),
            terminal_size_claude: (24, 80),
            terminal_size_shell: (6, 80),
            needs_clear: false,
            active_claude_session: None,
            active_shell_session: None,
            worktree_pending_branch: String::new(),
            base_branch_list: Vec::new(),
            base_branch_selected: 0,
            base_branch_filter: String::new(),
            switch_branch_active: false,
            switch_branch_list: Vec::new(),
            switch_branch_selected: 0,
            switch_branch_filter: String::new(),
            sync_active: false,
            sync_branches: Vec::new(),
            sync_selected: 0,
            prune_active: false,
            prune_stale: Vec::new(),
            worktree_pending_delete_branch: String::new(),
            resume_session_active: false,
            resume_sessions: Vec::new(),
            resume_session_selected: 0,
            resume_session_filter: String::new(),
            resume_session_all_projects: false,
            syntax_set,
            syntect_theme,
            help_active: false,
            help_context: Focus::Worktree,
            terminal_expanded: false,
            command_palette_active: false,
            command_palette_filter: String::new(),
            command_palette_selected: 0,
            ui_tick: 0,
            notification_bar_badges: Vec::new(),
            terminal_scroll_claude: 0,
            terminal_scroll_shell: 0,
            stats_session_id,
            today_stats,
            worktree_heads: HashMap::new(),
            ccusage_info: None,
            bg_branch_rx: None,
        };
        app.refresh_worktrees();
        app.refresh_reviews();
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
        self.active_claude_session = None;
        self.active_shell_session = None;

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
                self.active_claude_session = None;
                self.active_shell_session = None;

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
            Ok(engine) => match engine.list_worktrees() {
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
            },
            Err(e) => {
                log::warn!("failed to open git repository: {e}");
            }
        }
    }

    /// Reload the viewer file tree for the currently selected worktree.
    ///
    /// Preserves the currently open file and scroll position so that
    /// file-watcher refreshes don't disrupt the user's view.
    pub fn refresh_viewer(&mut self) {
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let path = wt.path.clone();
            self.viewer_state.load_file_tree(&path);
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
            self.diff_state.load_diff(&path, &base_branch, word_diff);
        }
    }

    /// Set focus to a panel, lazily loading data when first needed.
    pub fn set_focus(&mut self, focus: Focus) {
        match focus {
            Focus::Explorer | Focus::Viewer => {
                if self.viewer_state.file_tree.is_empty() {
                    self.refresh_viewer();
                }
                if self.diff_state.committed_files.is_empty() && self.diff_state.uncommitted_files.is_empty() {
                    self.refresh_diff();
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
        match self.focus {
            Focus::Worktree => "Alt+1-5: jump | Tab: next | q: quit | j/k: nav | w/W: new/del | s: switch | y/Y: sync/unsync | P: prune",
            Focus::Explorer => "Alt+1-5: jump | Tab: next panel | j/k: navigate | Enter: open file | h/l: collapse/expand | d: diff list",
            Focus::Viewer => "Alt+1-5: jump | Tab: next panel | Esc: back to explorer | j/k: scroll | /: search | c: comment",
            Focus::TerminalClaude => "Alt+1-5: jump | Ctrl+n: new CC | Ctrl+p: resume CC | keys → PTY",
            Focus::TerminalShell => "Alt+1-5: jump | Ctrl+t: new shell | keys → PTY",
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
            CommandId::ToggleTerminalExpand => {
                self.terminal_expanded = !self.terminal_expanded;
            }
            CommandId::CreateWorktree => {
                self.worktree_input_mode = WorktreeInputMode::CreatingWorktree;
                self.worktree_input_buffer.clear();
                self.set_status_info("New branch name (Enter to continue, Esc to cancel):".to_string());
            }
            CommandId::DeleteWorktree => {
                if let Some(wt) = self.worktrees.get(self.selected_worktree) {
                    if wt.is_main {
                        self.set_status("Cannot delete the main worktree.".to_string(), StatusLevel::Warning);
                    } else {
                        let branch = wt.branch.clone();
                        self.worktree_input_mode = WorktreeInputMode::ConfirmingDelete;
                        self.set_status_info(format!("Delete worktree '{branch}'? (y/n)"));
                    }
                }
            }
            CommandId::SwitchBranch => {
                self.set_status_info("Loading branches...".to_string());
                self.load_switch_branches();
                if !self.switch_branch_list.is_empty() {
                    self.switch_branch_active = true;
                    self.status_message = None;
                }
            }
            CommandId::SyncBranch => {
                self.load_sync_branches();
                if self.sync_branches.is_empty() {
                    self.set_status_info("No other worktree branches to sync.".to_string());
                } else {
                    self.sync_active = true;
                }
            }
            CommandId::PruneWorktrees => {
                match crate::git_engine::GitEngine::open(&self.repo_path) {
                    Ok(engine) => match engine.find_stale_worktrees() {
                        Ok(stale) => {
                            if stale.is_empty() {
                                self.set_status_info("No stale worktrees found.".to_string());
                            } else {
                                self.prune_stale = stale;
                                self.prune_active = true;
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
                    self.cherry_pick_source_branch = branch;
                    self.load_cherry_pick_commits();
                    self.cherry_pick_active = true;
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
                self.resume_session_active = true;
                self.load_resume_sessions();
            }
            CommandId::RefreshDiff => self.refresh_diff(),
            CommandId::SearchInFile => {
                self.viewer_state.search_active = true;
                self.viewer_state.search_query.clear();
                self.set_focus(Focus::Viewer);
            }
            CommandId::ToggleHelp => {
                self.help_context = self.focus;
                self.help_active = true;
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
                self.history_active = true;
                self.load_session_history();
            }
            CommandId::OpenRepo => {
                self.open_repo_active = true;
                self.open_repo_buffer = self.repo_path.display().to_string();
            }
            CommandId::SwitchRepo => {
                if self.repo_list.len() > 1 {
                    self.repo_selector_active = true;
                    self.repo_selector_selected = self.repo_list_index;
                }
            }
            CommandId::Quit => self.should_quit = true,
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
        self.focus = match self.focus {
            Focus::Worktree => Focus::Explorer,
            Focus::Explorer => Focus::Viewer,
            Focus::Viewer => Focus::TerminalClaude,
            Focus::TerminalClaude => Focus::TerminalShell,
            Focus::TerminalShell => Focus::Worktree,
        };
    }

    /// Cycle focus backward.
    pub fn cycle_focus_backward(&mut self) {
        self.focus = match self.focus {
            Focus::Worktree => Focus::TerminalShell,
            Focus::Explorer => Focus::Worktree,
            Focus::Viewer => Focus::Explorer,
            Focus::TerminalClaude => Focus::Viewer,
            Focus::TerminalShell => Focus::TerminalClaude,
        };
    }

    // ── Terminal / PTY helpers ────────────────────────────────────────

    /// Spawn a new Claude Code PTY session for the currently selected worktree.
    pub fn spawn_claude_code(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let cc_count = self
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::ClaudeCode)
            .count();
        let label = format!("CC:{}", cc_count + 1);
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal_size_claude;
        let idx = self.pty_manager.spawn_session(
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
        self.pty_manager.activate_session(idx);
        self.active_claude_session = Some(idx);
        Ok(idx)
    }

    /// Spawn a new interactive shell PTY session for the currently selected worktree.
    pub fn spawn_shell(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let sh_count = self
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::Shell)
            .count();
        let label = format!("SH:{}", sh_count + 1);
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal_size_shell;
        let idx = self.pty_manager.spawn_session(
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
        self.pty_manager.activate_session(idx);
        self.active_shell_session = Some(idx);
        Ok(idx)
    }

    /// Close (kill + remove) a terminal session by its global index.
    ///
    /// Adjusts `active_claude_session` and `active_shell_session` indices
    /// and falls back to the next available session for the current worktree.
    pub fn close_terminal_session(&mut self, global_idx: usize) {
        // Kill and remove the session.
        let _ = self.pty_manager.kill_session(global_idx);
        self.pty_manager.remove_session(global_idx);

        // Adjust active session indices.
        for a in [&mut self.active_claude_session, &mut self.active_shell_session]
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
        if self.active_claude_session == Some(usize::MAX) {
            self.active_claude_session = self
                .current_worktree_claude_sessions()
                .first()
                .map(|(idx, _)| *idx);
        }
        if self.active_shell_session == Some(usize::MAX) {
            self.active_shell_session = self
                .current_worktree_shell_sessions()
                .first()
                .map(|(idx, _)| *idx);
        }
    }

    /// Remove PTY sessions whose child processes have exited.
    ///
    /// Iterates in reverse to preserve indices of earlier sessions while
    /// removing later ones. Adjusts `active_claude_session` and
    /// `active_shell_session` indices after removal.
    pub fn cleanup_dead_sessions(&mut self) {
        let count = self.pty_manager.session_count();
        let mut removed_any = false;

        // Walk backwards so removals don't shift indices we haven't checked yet.
        for idx in (0..count).rev() {
            if !self.pty_manager.is_session_alive(idx) {
                log::info!("removing dead PTY session at index {idx}");
                self.pty_manager.remove_session(idx);
                removed_any = true;

                // Adjust active session indices.
                for a in [&mut self.active_claude_session, &mut self.active_shell_session].into_iter().flatten() {
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
            if self.active_claude_session == Some(usize::MAX) {
                self.active_claude_session = None;
            }
            if self.active_shell_session == Some(usize::MAX) {
                self.active_shell_session = None;
            }
        }
    }

    /// Load resumable Claude Code sessions from Claude's history.
    pub fn load_resume_sessions(&mut self) {
        let filter = if self.resume_session_all_projects {
            None
        } else {
            Some(self.repo_path.as_path())
        };
        match crate::claude_sessions::load_resumable_sessions(filter) {
            Ok(sessions) => {
                self.resume_sessions = sessions;
                self.resume_session_selected = 0;
                self.resume_session_filter.clear();
            }
            Err(e) => {
                log::warn!("failed to load resumable sessions: {e}");
                self.resume_sessions.clear();
                self.set_status(format!("Error loading sessions: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Return the filtered list of resume sessions based on the current filter string.
    pub fn filtered_resume_sessions(&self) -> Vec<(usize, &crate::claude_sessions::ResumableSession)> {
        if self.resume_session_filter.is_empty() {
            self.resume_sessions.iter().enumerate().collect()
        } else {
            let filter_lower = self.resume_session_filter.to_lowercase();
            self.resume_sessions
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
        let (rows, cols) = self.terminal_size_claude;
        let idx = self.pty_manager.spawn_session(
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
        self.pty_manager.activate_session(idx);
        self.active_claude_session = Some(idx);
        Ok(idx)
    }

    /// Return `(index_in_pty_manager, &PtySession)` pairs for Claude Code sessions
    /// belonging to the currently selected worktree.
    pub fn current_worktree_claude_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.pty_manager
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
        self.pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell)
            .collect()
    }

    /// Update the terminal content area size for Claude PTY sessions and resize them.
    pub fn update_claude_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal_size_claude = (rows, cols);
        let wt_path = self.selected_worktree_path();
        let count = self.pty_manager.session_count();
        for idx in 0..count {
            let s = &self.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == pty_manager::SessionKind::ClaudeCode {
                self.pty_manager.resize_session(idx, rows, cols);
            }
        }
    }

    /// Update the terminal content area size for Shell PTY sessions and resize them.
    pub fn update_shell_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal_size_shell = (rows, cols);
        let wt_path = self.selected_worktree_path();
        let count = self.pty_manager.session_count();
        for idx in 0..count {
            let s = &self.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell {
                self.pty_manager.resize_session(idx, rows, cols);
            }
        }
    }

    // ── Claude Code output analysis ────────────────────────────────────

    /// Scan all Claude Code sessions for file-change patterns in their PTY
    /// output.
    pub fn check_cc_output(&mut self) {
        let session_count = self.pty_manager.session_count();
        let mut found_new = false;

        for idx in 0..session_count {
            let new_files = self.pty_manager.detect_changed_files(idx);
            if !new_files.is_empty() {
                found_new = true;
                for f in new_files {
                    if !self.cc_changed_files.contains(&f) {
                        self.cc_changed_files.push(f);
                    }
                }
            }
        }

        if found_new {
            self.refresh_diff();
        }
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
        let signal_dir = self.repo_path.join(".conductor").join("cc-waiting");
        if let Ok(entries) = std::fs::read_dir(&signal_dir) {
            for entry in entries.flatten() {
                let filename = entry.file_name().to_string_lossy().to_string();
                let path_str = filename.replace("__", "/");
                for wt in &self.worktrees {
                    if wt.path.to_string_lossy() == path_str {
                        new_waiting.insert(wt.path.clone());
                    }
                }
            }
        }

        // Source 2: PTY pattern match fallback (for [Y/n] prompts).
        let session_count = self.pty_manager.session_count();
        for idx in 0..session_count {
            let session = &self.pty_manager.sessions()[idx];
            if session.kind != pty_manager::SessionKind::ClaudeCode {
                continue;
            }
            if self.pty_manager.is_waiting_for_input(idx) {
                new_waiting.insert(session.working_dir.clone());
            }
        }

        // Detect worktrees that newly entered waiting state.
        let current_wt_path = self.selected_worktree_path();
        let is_terminal_focused = matches!(self.focus, Focus::TerminalClaude);

        for wt_path in &new_waiting {
            if !self.cc_waiting_worktrees.contains(wt_path) {
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

        self.cc_waiting_worktrees = new_waiting;
    }

    /// Remove the hook signal file for a given session and clear its
    /// waiting state. Called when user sends input to a CC terminal.
    pub fn clear_cc_waiting_signal(&mut self, session_idx: usize) {
        let session = match self.pty_manager.sessions().get(session_idx) {
            Some(s) => s,
            None => return,
        };
        if session.kind != pty_manager::SessionKind::ClaudeCode {
            return;
        }
        let signal_dir = self.repo_path.join(".conductor").join("cc-waiting");
        let sanitized = session.working_dir.display().to_string().replace('/', "__");
        let _ = std::fs::remove_file(signal_dir.join(&sanitized));
        let working_dir = session.working_dir.clone();
        self.cc_waiting_worktrees.remove(&working_dir);
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
                    self.history_records = records;
                    self.history_selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to load session history: {e}");
                    self.history_records.clear();
                }
            }
        }
    }

    pub fn search_session_history(&mut self) {
        if let Some(store) = &self.review_store {
            let query = self.history_search_query.clone();
            let result = if query.is_empty() {
                store.list_session_history(50)
            } else {
                store.search_session_history(&query)
            };
            match result {
                Ok(records) => {
                    self.history_records = records;
                    self.history_selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to search session history: {e}");
                }
            }
        }
    }

    pub fn save_current_session_history(&mut self) {
        // Try the active Claude session first, then Shell.
        let active_idx = self.active_claude_session
            .or(self.active_shell_session);
        let active_idx = match active_idx {
            Some(idx) => idx,
            None => {
                self.set_status("No active PTY session to save.".to_string(), StatusLevel::Warning);
                return;
            }
        };

        let sessions = self.pty_manager.sessions();
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
        let output = self.pty_manager.get_output(active_idx).join("\n");

        if let Some(store) = &self.review_store {
            match store.save_session_history(&session_id, &worktree, &label, kind, &output) {
                Ok(()) => {
                    self.status_message = Some(StatusMessage::new("Session history saved.".to_string(), StatusLevel::Success, self.ui_tick));
                    if self.history_active {
                        match store.list_session_history(50) {
                            Ok(records) => {
                                self.history_records = records;
                                self.history_selected = 0;
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

    /// Create a worktree from a base ref (2-step flow).
    pub fn create_worktree_from_base(&mut self, branch_name: &str, base_ref: &str) {
        let base = if base_ref.is_empty() { "origin/main" } else { base_ref };
        let wt_dir = self.config.general.worktree_dir.clone();
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.create_worktree_from_base(branch_name, base, wt_dir.as_deref()) {
                    Ok(path) => {
                        self.record_stat("branches_created");
                        self.refresh_worktrees();
                        self.select_worktree_by_path(&path);
                        self.set_status(format!(
                            "Created worktree: {} (from {})", path.display(), base
                        ), StatusLevel::Success);
                    }
                    Err(e) => {
                        self.set_status(format!("Error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Create a worktree from a remote branch.
    pub fn create_worktree_from_remote(&mut self, remote_branch: &str) {
        let wt_dir = self.config.general.worktree_dir.clone();
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.create_worktree_from_remote(remote_branch, wt_dir.as_deref()) {
                    Ok(path) => {
                        self.record_stat("branches_created");
                        self.refresh_worktrees();
                        self.select_worktree_by_path(&path);
                        self.set_status(format!(
                            "Created tracking worktree: {}", path.display()
                        ), StatusLevel::Success);
                    }
                    Err(e) => {
                        self.set_status(format!("Error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
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

    /// Execute sync merge on the current worktree.
    pub fn execute_sync(&mut self, source_branch: &str) {
        let wt_path = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("No worktree selected.".to_string(), StatusLevel::Error);
                return;
            }
        };
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.sync_merge(&wt_path, source_branch) {
                    Ok(msg) => {
                        self.set_status(msg, StatusLevel::Success);
                        self.refresh_worktrees();
                    }
                    Err(e) => {
                        self.set_status(format!("Sync error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Execute unsync (reset --hard HEAD) on the current worktree.
    pub fn execute_unsync(&mut self) {
        let wt_path = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("No worktree selected.".to_string(), StatusLevel::Error);
                return;
            }
        };
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.unsync_reset(&wt_path) {
                    Ok(msg) => {
                        self.set_status(msg, StatusLevel::Success);
                        self.refresh_worktrees();
                    }
                    Err(e) => {
                        self.set_status(format!("Unsync error: {e}"), StatusLevel::Error);
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
                for name in &self.prune_stale {
                    match engine.prune_stale_worktree(name) {
                        Ok(()) => pruned += 1,
                        Err(e) => {
                            log::warn!("failed to prune worktree '{name}': {e}");
                        }
                    }
                }
                self.set_status(format!("Pruned {pruned} stale worktree(s)."), StatusLevel::Success);
                self.prune_stale.clear();
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
                        self.switch_branch_list = branches;
                        self.switch_branch_selected = 0;
                        self.switch_branch_filter.clear();
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.switch_branch_list.clear();
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
        let (tx, rx) = mpsc::channel();
        self.bg_branch_rx = Some(rx);
        std::thread::spawn(move || {
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
        let rx = match self.bg_branch_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        match rx.try_recv() {
            Ok(branches) => {
                // Preserve the user's current filter/selection as best we can.
                let prev_selected_name = self.filtered_switch_branches()
                    .get(self.switch_branch_selected)
                    .map(|(_, name)| (*name).clone());
                self.switch_branch_list = branches;
                // Try to restore selection by name.
                if let Some(name) = prev_selected_name {
                    if let Some(pos) = self.filtered_switch_branches()
                        .iter()
                        .position(|(_, b)| **b == name)
                    {
                        self.switch_branch_selected = pos;
                    }
                }
                self.bg_branch_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => { /* still fetching */ }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.bg_branch_rx = None;
            }
        }
    }

    /// Return the filtered list of switch branches based on the current filter.
    pub fn filtered_switch_branches(&self) -> Vec<(usize, &String)> {
        if self.switch_branch_filter.is_empty() {
            self.switch_branch_list.iter().enumerate().collect()
        } else {
            let filter_lower = self.switch_branch_filter.to_lowercase();
            self.switch_branch_list
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
                        self.base_branch_list = branches;
                        self.base_branch_selected = 0;
                        self.base_branch_filter.clear();
                        // Pre-select origin/<main_branch> if it exists.
                        let default_base = format!("origin/{}", self.config.general.main_branch);
                        if let Some(pos) = self.base_branch_list.iter().position(|b| b == &default_base) {
                            self.base_branch_selected = pos;
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.base_branch_list.clear();
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
        if self.base_branch_filter.is_empty() {
            self.base_branch_list.iter().enumerate().collect()
        } else {
            let filter_lower = self.base_branch_filter.to_lowercase();
            self.base_branch_list
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// Load sync branch candidates (other local worktree branches).
    pub fn load_sync_branches(&mut self) {
        let current_branch = self.selected_worktree_branch();
        self.sync_branches = self.worktrees
            .iter()
            .filter(|w| w.branch != current_branch)
            .map(|w| w.branch.clone())
            .collect();
        self.sync_selected = 0;
    }

    pub fn delete_selected_worktree(&mut self) {
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
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.remove_worktree(&wt_path) {
                    Ok(()) => {
                        self.set_status(format!("Deleted worktree: {branch}"), StatusLevel::Success);
                        self.refresh_worktrees();
                    }
                    Err(e) => {
                        self.set_status(format!("Error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    // ── Cherry-pick helpers ────────────────────────────────────────────

    pub fn load_cherry_pick_commits(&mut self) {
        let branch = self.cherry_pick_source_branch.clone();
        if branch.is_empty() {
            self.cherry_pick_commits.clear();
            return;
        }
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_branch_commits(&branch, 20) {
                    Ok(commits) => {
                        self.cherry_pick_commits = commits;
                        self.cherry_pick_selected = 0;
                    }
                    Err(e) => {
                        log::warn!("failed to list commits for branch '{branch}': {e}");
                        self.cherry_pick_commits.clear();
                    }
                }
            }
            Err(e) => {
                log::warn!("failed to open git repository for cherry-pick: {e}");
                self.cherry_pick_commits.clear();
            }
        }
    }

    pub fn execute_cherry_pick(&mut self) {
        let commit = match self.cherry_pick_commits.get(self.cherry_pick_selected) {
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

        // Update active sessions to match the new worktree.
        let wt_name = self.selected_worktree_branch();
        let claude_sessions = self.current_worktree_claude_sessions();
        self.active_claude_session = claude_sessions.first().map(|(idx, _)| *idx);
        let shell_sessions = self.current_worktree_shell_sessions();
        self.active_shell_session = shell_sessions.first().map(|(idx, _)| *idx);

        // Activate the PTY sessions.
        if let Some(idx) = self.active_claude_session {
            self.pty_manager.activate_session(idx);
        }
        if let Some(idx) = self.active_shell_session {
            self.pty_manager.activate_session(idx);
        }

        self.terminal_scroll_claude = 0;
        self.terminal_scroll_shell = 0;

        self.set_status(format!("Switched to worktree: {wt_name}"), StatusLevel::Success);
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

    /// Return `(worktree_name, working_dir)` for the currently selected worktree.
    fn selected_worktree_info(&self) -> (String, PathBuf) {
        self.worktrees
            .get(self.selected_worktree)
            .map(|w| (w.branch.clone(), w.path.clone()))
            .unwrap_or_else(|| ("default".to_string(), self.repo_path.clone()))
    }
}
