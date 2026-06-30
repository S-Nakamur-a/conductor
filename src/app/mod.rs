//! App state and focus management.
//!
//! This module defines the top-level application state, the unified panel
//! layout focus model, and transitions between panels.

mod review;
mod terminal;
mod worktree;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};

use crate::background::BackgroundOp;

use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::config;
use crate::diff_state::{DiffState, DiffViewMode};
use crate::git_engine;
use crate::grep_search::GrepProgress;
use crate::jump_history::JumpHistory;
use crate::keymap::KeyMap;
use crate::overlay::{ActiveOverlay, OverlayManager};
use crate::overlay::{ReferencesOverlay, SymbolActionOverlay, SymbolHintOverlay};
use crate::pty_manager;
use crate::review_state::ReviewState;
use crate::review_store::{self, Author, CommentKind, ReviewStore};
use crate::symbol_index::SymbolIndex;
use crate::terminal_state::TerminalState;
use crate::theme::Theme;
use crate::viewer::ViewerState;
use crate::worktree_ops::WorktreeManager;

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
        Self {
            text,
            level,
            created_at_tick: tick,
        }
    }

    /// Return the icon prefix for this message level.
    pub fn icon(&self) -> &'static str {
        match self.level {
            StatusLevel::Success => "\u{2713} ", // ✓
            StatusLevel::Error => "\u{2717} ",   // ✗
            StatusLevel::Warning => "\u{26A1} ", // ⚡
            StatusLevel::Info => "\u{2139} ",    // ℹ
        }
    }
}

impl From<String> for StatusMessage {
    fn from(text: String) -> Self {
        Self {
            text,
            level: StatusLevel::Info,
            created_at_tick: 0,
        }
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
    /// The embedded editor panel (vim/emacs in a PTY) occupying the merged
    /// Explorer+Viewer region. Only reachable while [`App::editor`] is `Some`.
    Editor,
}

impl Focus {
    /// The base keymap context for this panel. Both terminals share the
    /// `Terminal` context (sub-modes like diff/comment lists are tracked
    /// separately by the panels themselves).
    pub fn key_context(self) -> crate::keymap::KeyContext {
        use crate::keymap::KeyContext;
        match self {
            Focus::Worktree => KeyContext::Worktree,
            Focus::Explorer => KeyContext::Explorer,
            Focus::Viewer => KeyContext::Viewer,
            Focus::TerminalClaude | Focus::TerminalShell => KeyContext::Terminal,
            Focus::Editor => KeyContext::Editor,
        }
    }

    /// Whether this panel hosts a PTY whose inner program (Claude Code, shell,
    /// or an editor) should receive raw keystrokes. The event dispatcher routes
    /// these panels through the PTY-forwarding path; the keymap only steals back
    /// the chords that [fire in the terminal](crate::keymap::Action).
    pub fn is_pty(self) -> bool {
        matches!(
            self,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        )
    }
}

/// Direction of a tmux-style pane resize, relative to the focused panel.
///
/// Semantics mirror tmux `resize-pane -L/-R/-U/-D`: the focused panel grows
/// toward the given direction by moving the divider it shares with the neighbor
/// on that side. When the panel has no neighbor on that side (it sits against
/// the edge), the opposite divider moves instead, so the panel shrinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeDir {
    Left,
    Right,
    Up,
    Down,
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
    /// Confirming a hard reset of main to origin (y/n) — discards local commits.
    ConfirmingReset,
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
    /// Session name for Claude Code `--name` flag (generated by LLM for smart worktrees).
    pub session_name: Option<String>,
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
    Created {
        path: PathBuf,
        pending: PendingWorktree,
    },
    CreateFailed {
        error: String,
        pending: PendingWorktree,
    },
    Deleted {
        branch: String,
    },
    DeleteFailed {
        error: String,
        branch: String,
    },
    Skipped {
        branch: String,
        reason: String,
    },
    /// Smart worktree: LLM resolved a branch name (for UI update).
    SmartBranchResolved {
        description: String,
        branch: String,
        prompt: String,
        session_name: Option<String>,
    },
    /// Smart worktree: entire operation failed.
    SmartFailed {
        description: String,
        error: String,
    },
}

/// Result from the smart worktree LLM generation.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SmartGenResult {
    pub branch: String,
    pub prompt: String,
    #[serde(default)]
    pub session_name: Option<String>,
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

/// A pending view restore: open this file and scroll to this line once the
/// file tree for the current worktree is loaded. Used to restore where the
/// user was after a restart (or when switching back to a worktree).
#[derive(Debug, Clone)]
pub struct PendingViewRestore {
    /// Worktree-relative path of the file to re-open.
    pub file: String,
    /// Top visible line (0-based) to scroll to.
    pub scroll: usize,
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
    pub const VIEWER: u8 = 0b0000_0100;
    pub const TERMINAL: u8 = 0b0000_1000;
    pub const ALL: u8 = 0b0000_1111;

    pub fn mark(&mut self, bits: u8) {
        self.0 |= bits;
    }
    pub fn mark_all(&mut self) {
        self.0 = Self::ALL;
    }
    #[allow(dead_code)]
    pub fn is_dirty(&self, bits: u8) -> bool {
        self.0 & bits != 0
    }
    pub fn any(&self) -> bool {
        self.0 != 0
    }
    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

/// Background operations driven by the 60fps event loop, grouped so that `App`
/// does not carry a separate flat field per async task. Each op is polled from
/// the main loop (or worktree-switch handlers) and writes its result back into
/// the relevant `App` state.
#[derive(Default)]
pub struct BackgroundOps {
    /// Background update check (latest release lookup).
    pub update_check: BackgroundOp<Option<crate::update_checker::UpdateInfo>>,
    /// Background ccusage (token/cost) fetch.
    pub ccusage: BackgroundOp<CcusageInfo>,
    /// Background branch list fetch (switch-branch overlay).
    pub branch: BackgroundOp<Vec<String>>,
    /// Background pull operation.
    pub pull: BackgroundOp<Result<String, String>>,
    /// Background `gh pr view` lookup.
    pub pr_url: BackgroundOp<Option<String>>,
    /// Background diff computation (worktree switch).
    pub diff: BackgroundOp<BgDiffResult>,
    /// Background file tree walk (worktree switch).
    pub file_tree: BackgroundOp<Vec<crate::viewer::FileTreeEntry>>,
    /// Background branch details computation (worktree switch).
    pub branch_details: BackgroundOp<git_engine::BranchDetails>,
    /// Background symbol index build.
    pub symbol_index: BackgroundOp<Result<usize, String>>,
}

/// State for an active embedded editor panel (vim/emacs in a PTY).
///
/// Transient: created when the user opens a file in `$EDITOR` and dropped when
/// the editor process exits. Owns the render cache so the editor panel renders
/// independently of the Claude/Shell terminal caches.
pub struct EditorPanel {
    /// Index of the editor's PTY session in the `PtyManager` session list.
    /// Kept in sync (shifted/cleared) when other sessions are removed.
    pub session_idx: usize,
    /// Absolute path of the file being edited — used for the reload-on-exit and
    /// the panel title.
    pub path: PathBuf,
    /// Cached PTY render output for the editor panel (mirrors the Claude/Shell
    /// caches in `TerminalState`).
    pub cache: crate::ui::common::PtyRenderCache,
    /// Set when the PTY reader thread produced new output to re-render.
    pub dirty: bool,
}

/// Top-level application state shared across all UI panels.
pub struct App {
    /// Tracks which panels need re-rendering.
    pub dirty: DirtyPanels,
    /// Current panel focus.
    pub focus: Focus,
    /// Panel that had focus immediately before the current one — drives the
    /// border-color glide when focus moves (see `animated_border_color`).
    pub focus_prev: Focus,
    /// When focus last changed, for timing the border transition.
    pub focus_changed_at: std::time::Instant,
    /// All overlay popup states (switch-branch, grab, prune, help, etc.).
    pub overlays: OverlayManager,
    /// Working directory of the repository being inspected.
    pub repo_path: PathBuf,
    /// Display name of the main repository (directory name of the main worktree).
    pub main_repo_name: String,
    /// Whether the application should quit on the next tick.
    pub should_quit: bool,
    /// The embedded editor panel, when active. `Some` ⟺ an editor PTY is running
    /// and occupying the merged Explorer+Viewer region; `None` is the normal
    /// (no-editor) layout. Set by [`App::open_in_editor`] and torn down by
    /// [`App::exit_editor`] (the only two methods that pair this field with
    /// `Focus::Editor`, keeping the invariant local).
    pub editor: Option<EditorPanel>,
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
    /// Active theme name — the canonical key used to resolve `theme`.
    /// Kept in sync by `set_theme`; used to find the current selection in the
    /// theme picker and to build the config layer when persisting.
    pub theme_name: String,
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
    /// Per-id cache of rendered Markdown (comment/reply bodies), so the inline
    /// thread box doesn't re-parse/highlight every frame.
    pub markdown_cache: crate::ui::markdown::MarkdownCache,

    /// Which panel is currently expanded to 100% (via the [<=>] button).
    /// `None` means no panel is expanded (default layout).
    pub expanded_panel: Option<Focus>,

    /// Runtime height percentage for the Claude Code area within the terminal
    /// column (the Shell gets the remainder). Seeded from
    /// `config.layout.terminal_split_pct` at startup, adjusted live by a
    /// tmux-style pane resize (Ctrl+Alt+Up/Down with a terminal focused), and
    /// persisted back to the config file so the ratio survives a restart.
    pub terminal_split_pct: u16,

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

    // ── View state restore (persist where the user was) ─────────
    /// Branch of the worktree whose viewer state is currently loaded in
    /// memory. Tracks the "owner" of `viewer_state` so it can be persisted
    /// before we switch away. `None` until the first worktree is loaded.
    pub current_view_branch: Option<String>,
    /// A saved file+scroll to restore once the file tree is available
    /// (one-shot; consumed by [`App::consume_pending_view_restore`]).
    pub pending_view_restore: Option<PendingViewRestore>,

    /// Cached layout rectangles (recomputed when frame size or expansion state changes).
    pub layout_cache: crate::ui::layout::LayoutCache,
    /// Clickable regions of the worktree bar, recorded during render and read by
    /// the mouse handler (worktree select / delete / add).
    pub wtbar_hits: Vec<crate::ui::worktree_bar::WtbarHit>,
    /// Index of the first worktree chip shown in the bar (horizontal scroll
    /// position). Adjusted by wheel/arrow paging and re-clamped each render.
    pub wtbar_scroll: usize,
    /// When set, the next bar render pans `wtbar_scroll` just enough to keep the
    /// selected worktree's chip visible. Set when the selection changes so a
    /// jump always reveals its chip, without disturbing free scrolling otherwise.
    pub wtbar_reveal_selected: bool,

    // ── Code navigation (symbol index + jump history) ───────────
    pub symbol_index: SymbolIndex,
    pub jump_history: JumpHistory,
    pub references_overlay: ReferencesOverlay,
    pub symbol_hint_overlay: SymbolHintOverlay,
    pub symbol_action_overlay: SymbolActionOverlay,

    // ── Background operations (polled by the event loop) ─────────
    pub bg: BackgroundOps,

    // ── New worktree badge ──────────────────────────────────────
    /// Paths of worktrees recently created (for badge display). Cleared on selection.
    pub new_worktree_paths: HashSet<PathBuf>,

    // ── Panel number overlay (Alt+/ toggle) ─────────────────────
    /// When true, panel number badges are rendered over each panel.
    /// Toggled by Alt+/ and auto-dismissed after 2 seconds.
    pub show_panel_number_overlay: bool,
    /// Instant when the overlay was activated (for auto-dismiss timer).
    pub panel_overlay_since: Option<std::time::Instant>,

    // ── Party mode (hidden easter egg) ───────────────────────────
    /// When true, the UI goes full party: the focused panel's border
    /// glows in a flowing rainbow, syntax tokens turn rainbow, the title
    /// bar shimmers, and confetti drifts across the screen. Toggled from
    /// the command palette; not persisted (session-only secret).
    pub party_mode: bool,

    // ── Rich mode (terminal graphics tiers) ──────────────────────
    /// Resolved rich-mode tier, decided once at startup from config +
    /// terminal capability detection (see `term_caps`). Tier A drives the
    /// gradient border/title effects; Tier B additionally enables
    /// graphics-protocol image previews.
    pub rich_tier: crate::term_caps::RichTier,
    /// Graphics-protocol picker for Tier B image rendering.
    /// `Some` only when `rich_tier` is `TierB`.
    pub rich_picker: Option<ratatui_image::picker::Picker>,
    /// The tier detection resolved at startup, kept so the runtime toggle
    /// can restore it after switching rich mode off.
    pub rich_tier_available: crate::term_caps::RichTier,
    /// Wall-clock anchor for rich-mode animations. Phases derive from
    /// elapsed time (not `ui_tick`) so animation speed is independent of
    /// the redraw rate, which varies from ~2fps (idle pulses) to ~60fps
    /// (active input).
    pub rich_epoch: std::time::Instant,

    // ── Reflow transcript view ───────────────────────────────────────────
    /// State for the read-only, word-wrapped session-log viewer that
    /// overlays the Claude PTY panel during infinite-scrollback mode.
    pub reflow: ReflowView,
}

/// Active entry animation for the reflow transcript view.
///
/// Holds the `Instant` the animation started so that `reflow_view::render` can
/// compute progress via elapsed time without any frame-counter dependency.
/// Only the *entry* transition is animated — it masks the initial
/// `build_lines` latency. Leaving the view swaps back to the live PTY
/// immediately (no exit animation) so returning to the prompt feels instant.
pub struct Sweep {
    pub start: std::time::Instant,
}

/// State for the reflow transcript view.
///
/// When `active` is `true`, this view overlays the Claude PTY panel and renders
/// the Claude Code session log as a scrollable, word-wrapped Markdown display.
/// Width changes trigger a full re-render of `cached_lines`; otherwise the
/// cached lines are reused each frame.
#[derive(Default)]
pub struct ReflowView {
    /// Whether the reflow view is currently overlaying the Claude PTY panel.
    pub active: bool,
    /// Parsed and normalised log entries from the session file.
    ///
    /// Wrapped in `Rc` so that `build_lines` can cheaply clone the handle
    /// (refcount increment only) and release its borrow on `self` before
    /// calling `cache.render` — avoiding a deep copy of all entry strings on
    /// every resize.
    pub entries: std::rc::Rc<Vec<crate::claude_log::LogEntry>>,
    /// Vertical scroll offset — number of rendered lines from the top to skip.
    pub scroll: usize,
    /// Total number of lines in `cached_lines` (kept in sync after each render).
    pub total_lines: usize,
    /// Panel inner width at the last render — used to detect size changes for reflow.
    pub last_width: u16,
    /// When `true`, the next render pins scroll to the bottom (most recent turn).
    pub pending_bottom: bool,
    /// Pre-rendered, width-reflowed lines; rebuilt only when `last_width` changes.
    pub cached_lines: Vec<ratatui::text::Line<'static>>,
    /// Inner panel height at the last render — used for page-scroll sizing.
    pub last_inner_height: u16,
    /// Per-session Markdown render cache.
    ///
    /// Kept separate from `App::markdown_cache` so it does not pollute the
    /// shared cache with reflow keys and is automatically invalidated when a new
    /// session is opened (the whole `ReflowView` is replaced by `open_reflow`).
    pub cache: crate::ui::markdown::MarkdownCache,
    /// In-progress entry/exit sweep animation, or `None` when idle.
    ///
    /// `Option<Sweep>` defaults to `None` — `Sweep` itself does not need
    /// `Default` because `Option<T>: Default` is always `None` without a
    /// `T: Default` bound.
    pub sweep: Option<Sweep>,
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

impl App {
    /// Returns `true` when the panel number overlay should be rendered.
    /// Activated by Alt+/ and auto-dismisses after 2 seconds.
    pub fn show_panel_overlay(&self) -> bool {
        if !self.show_panel_number_overlay {
            return false;
        }
        // Auto-dismiss after 2 seconds.
        if let Some(since) = self.panel_overlay_since
            && since.elapsed() >= std::time::Duration::from_secs(2)
        {
            return false;
        }
        true
    }

    /// Toggle the panel number overlay on/off. When turning on, starts
    /// the auto-dismiss timer.
    pub fn toggle_panel_overlay(&mut self) {
        if self.show_panel_overlay() {
            self.show_panel_number_overlay = false;
            self.panel_overlay_since = None;
        } else {
            self.show_panel_number_overlay = true;
            self.panel_overlay_since = Some(std::time::Instant::now());
        }
    }

    // ── Reflow transcript view ────────────────────────────────────────────

    /// Enter the reflow transcript view for the active Claude panel's session.
    ///
    /// Resolves the session backing the currently displayed Claude panel (via
    /// its pinned `claude_session_id`), loads and parses that `.jsonl` file, and
    /// activates the overlay. Falls back to the selected worktree's most recent
    /// session only when the panel has no tracked id. If no session is found or
    /// the log is empty, a status flash is shown and the view stays inactive.
    pub fn open_reflow(&mut self) {
        // Prefer the session backing the *currently displayed* Claude panel. A
        // worktree can host several Claude panels (CC:1, CC:2, …), so resolving
        // "the worktree's latest session" would open whichever log was written
        // most recently regardless of which panel is on screen — that is the
        // cross-panel scroll bleed this view used to suffer from. The pinned
        // per-session id (see `PtySession::claude_session_id`) ties the
        // transcript to the panel the user is actually looking at.
        let resolved = self
            .terminal
            .active_claude_session
            .and_then(|idx| self.terminal.pty_manager.claude_session_ref(idx));

        // Working dir of the selected worktree, used for the mtime fallback.
        let working_dir = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status(
                    "No worktree selected for transcript view".to_string(),
                    StatusLevel::Warning,
                );
                return;
            }
        };

        // 1. Prefer the active panel's pinned session id. When a worktree hosts
        //    several Claude panels (CC:1, CC:2, …) this ties the transcript to
        //    the panel actually on screen, avoiding cross-panel scroll bleed.
        let mut entries = resolved
            .and_then(|(wd, sid)| crate::claude_sessions::session_jsonl_path(&wd, &sid))
            .map(|path| crate::claude_log::load_session(&path))
            .unwrap_or_default();

        // 2. Fall back to the most-recently-written log in this worktree's
        //    project dir when the pinned session is missing or empty. This is
        //    what catches a manual in-app `/resume`: it switches the live
        //    session id away from the conductor-launched (pinned) one, so the
        //    pinned file is stale/empty while the real transcript is whatever
        //    Claude is now appending to (= freshest mtime). Empty/aux logs
        //    (e.g. one-shot security-review runs sharing the dir) are skipped.
        if entries.is_empty() {
            entries = crate::claude_sessions::session_logs_by_mtime(&working_dir)
                .iter()
                .map(|path| crate::claude_log::load_session(path))
                .find(|e| !e.is_empty())
                .unwrap_or_default();
        }

        if entries.is_empty() {
            self.set_status(
                "Session log is empty or unreadable".to_string(),
                StatusLevel::Info,
            );
            return;
        }

        // TODO(background): load_session is currently synchronous; for very
        // large files (5MB+) this may block the UI for a frame or two.
        // A future version should spawn the parse on a background thread and
        // show a "Loading…" placeholder while the entries arrive.
        self.reflow = ReflowView {
            active: true,
            entries: std::rc::Rc::new(entries),
            scroll: 0,
            total_lines: 0,
            last_width: 0, // Forces a full line rebuild on first render.
            pending_bottom: true,
            cached_lines: Vec::new(),
            last_inner_height: 0,
            cache: crate::ui::markdown::MarkdownCache::new(),
            // Start the entry transition: the border glides from the accent to
            // its complement over TRANSITION_DURATION_MS, masking the initial
            // build_lines latency.
            sweep: Some(Sweep {
                start: std::time::Instant::now(),
            }),
        };
    }

    /// Leave the reflow transcript view and return to the live PTY display.
    pub fn close_reflow(&mut self) {
        self.reflow.active = false;
        self.reflow.sweep = None;
        // Reset Claude scrollback so the live tail is shown immediately.
        self.terminal.scroll_claude = 0;
        // Force a fresh PTY snapshot on the next frame. While the reflow view
        // was up the PTY panel rendered nothing, so `cache_claude` holds the
        // pre-scrollback frame. If no new output happens to arrive right after
        // closing (e.g. Claude is idle at its prompt), the stale cache would
        // otherwise persist and the input box would not reappear. Clearing the
        // cache and marking it dirty rebuilds the live tail immediately.
        self.terminal.cache_claude = Default::default();
        self.terminal.dirty_claude = true;
    }

    /// Leave the reflow transcript view, returning to the live PTY immediately.
    ///
    /// Kept as a distinct entry point from `close_reflow` for the keybind/scroll
    /// call sites, but there is no exit animation: the content swaps back to the
    /// live tail on the same frame so returning to the prompt feels instant.
    pub fn request_close_reflow(&mut self) {
        self.close_reflow();
    }

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
        let theme_name = resolve_theme_name(&config);
        let theme = Theme::from_name(&theme_name);
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
            selected_worktree: 0,
            worktrees: Vec::new(),
            config,
            keymap,
            theme,
            theme_name,
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
            markdown_cache: crate::ui::markdown::MarkdownCache::new(),
            expanded_panel: None,
            terminal_split_pct: config_terminal_split_pct,
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
            symbol_index: SymbolIndex::new(PathBuf::new()),
            jump_history: JumpHistory::new(),
            references_overlay: ReferencesOverlay::default(),
            symbol_hint_overlay: SymbolHintOverlay::default(),
            symbol_action_overlay: SymbolActionOverlay::default(),
            bg: BackgroundOps::default(),
            new_worktree_paths: HashSet::new(),
            show_panel_number_overlay: false,
            panel_overlay_since: None,
            party_mode: false,
            rich_tier: crate::term_caps::RichTier::Off,
            rich_picker: None,
            rich_tier_available: crate::term_caps::RichTier::Off,
            rich_epoch: std::time::Instant::now(),
            reflow: ReflowView::default(),
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

    /// Switch to a different repository by index in `repo_list`.
    pub fn switch_repo(&mut self, index: usize) {
        if index >= self.repo_list.len() {
            return;
        }
        // Persist the outgoing repo's view before swapping the store.
        self.persist_view_state();
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
        self.diff_state =
            DiffState::new(&self.config.general.main_branch, self.diff_state.view_mode);
        // Restore the new repo's last selected worktree + open file/scroll.
        self.restore_selected_worktree_and_view();
        self.refresh_reviews();
        self.terminal.active_claude_session = None;
        self.terminal.active_shell_session = None;

        self.set_status(
            format!("Switched to repository: {}", self.main_repo_name),
            StatusLevel::Success,
        );
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
            self.set_status(
                format!("Not a directory: {}", canonical.display()),
                StatusLevel::Error,
            );
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
                self.diff_state =
                    DiffState::new(&self.config.general.main_branch, self.diff_state.view_mode);
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
                self.set_status(
                    format!("Opened repository: {repo_name}"),
                    StatusLevel::Success,
                );
            }
            Err(e) => {
                self.set_status(
                    format!("Not a git repository: {} ({e})", canonical.display()),
                    StatusLevel::Error,
                );
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
        // Remember which branch is selected *before* we replace the list, so we
        // can pin the selection to it by identity afterwards (the list order can
        // shift when worktrees are added/removed).
        let prev_selected_branch = self.selected_worktree_branch();
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
                        // Preserve the selection by *branch identity*, not list
                        // position: indices shift when worktrees are added or
                        // removed. Re-finding the branch keeps the selection
                        // pinned to the same worktree instead of silently sliding
                        // onto a neighbour. Only when the branch is gone (its
                        // worktree was removed) do we fall back to clamping.
                        if let Some(idx) = reselect_worktree_index(
                            &self.worktrees,
                            &prev_selected_branch,
                            self.selected_worktree,
                        ) {
                            self.selected_worktree = idx;
                        }
                        // Detect commits by HEAD oid changes.
                        for wt in &self.worktrees {
                            if let Ok(wt_engine) = git_engine::GitEngine::open(&wt.path)
                                && let Ok(head_oid) = wt_engine.head_oid_string()
                            {
                                if let Some(old) = self.worktree_heads.get(&wt.branch)
                                    && old != &head_oid
                                {
                                    self.record_stat("commits_made");
                                    changed = true;
                                }
                                self.worktree_heads.insert(wt.branch.clone(), head_oid);
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
        // If the selected worktree's branch changed out from under us (its
        // worktree was removed, so the selection fell back to another branch —
        // often the main worktree), reload the review state. Otherwise the
        // previous branch's change summary and comments linger and get shown
        // against the wrong branch (e.g. a merged PR's summary on `main`).
        if self.selected_worktree_branch() != prev_selected_branch {
            self.refresh_reviews();
        }
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

    /// Open the file currently shown in the Viewer in an embedded editor panel
    /// (`$VISUAL` / `$EDITOR` in a PTY occupying the merged Explorer+Viewer
    /// region). Resolves the viewer's relative `current_file` against the
    /// selected worktree; if no file is open, flashes a hint instead. A no-op if
    /// an editor is already open.
    pub fn open_in_editor(&mut self) {
        if self.editor.is_some() {
            return;
        }
        // A grabbed worktree's terminals are locked (its sessions run on main),
        // and §1c would freeze an editor opened here. Refuse rather than trap the
        // user in an undrivable editor.
        if self.is_selected_worktree_grabbed() {
            self.set_status(
                "Cannot edit while this worktree is grabbed".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let Some(path) =
            editor_target(self.viewer_state.content.current_file.as_deref(), &working_dir)
        else {
            self.set_status("No file open to edit".to_string(), StatusLevel::Warning);
            return;
        };

        let argv = resolve_editor_command(
            std::env::var("VISUAL").ok().as_deref(),
            std::env::var("EDITOR").ok().as_deref(),
            "vi",
        );
        // `resolve_editor_command` never returns an empty vec.
        let (program, args) = argv.split_first().expect("editor command is non-empty");

        let (rows, cols) = self.editor_pty_size();
        match self.terminal.pty_manager.spawn_editor_session(
            &worktree_name,
            "editor",
            &working_dir,
            rows,
            cols,
            program,
            args,
            &path,
        ) {
            Ok(idx) => {
                self.terminal.pty_manager.activate_session(idx);
                let fname = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.editor = Some(EditorPanel {
                    session_idx: idx,
                    path,
                    cache: Default::default(),
                    dirty: true,
                });
                self.set_focus(Focus::Editor);
                // Repaint from scratch so the editor's alternate screen draws
                // cleanly over the panels it replaces.
                self.terminal.needs_clear = true;
                self.dirty.mark_all();
                self.set_status(
                    format!("Editing {fname} — Ctrl+Esc: Claude · :q: close · ctrl+alt+z: zoom"),
                    StatusLevel::Info,
                );
            }
            Err(e) => {
                self.set_status(format!("Failed to launch editor: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Tear down the embedded editor panel: kill/remove its PTY session, restore
    /// focus to the Viewer, and reload the just-edited file so the change is
    /// visible immediately (mirrors the debounced file-watcher refresh pair).
    pub fn exit_editor(&mut self) {
        let Some(path) = self.take_down_editor() else {
            return;
        };
        // Reload the just-edited file immediately (mirror the file-watcher pair).
        self.refresh_viewer();
        self.refresh_diff();
        self.dirty.mark_all();
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.set_status(format!("Edited {fname}"), StatusLevel::Success);
    }

    /// Tear down the editor PTY and drop the panel, returning the edited path
    /// (or `None` if no editor was open). Shared core of [`Self::exit_editor`]
    /// (which adds reload + status) and worktree switching (which discards the
    /// editor silently because the surrounding context is being reloaded anyway).
    fn take_down_editor(&mut self) -> Option<PathBuf> {
        let panel = self.editor.take()?;
        // Remove the editor PTY (kill is harmless if the child already exited),
        // adjusting other session indices.
        self.close_terminal_session(panel.session_idx);
        // Move focus off the (now-gone) editor only if it was focused — the usual
        // `:q` flow. If the user had stepped over to Claude and the editor exited
        // from under them, leave their focus put; just drop any stale "editor
        // maximized" state. Assigned directly (not via `set_focus`) so callers
        // control any reload.
        if self.focus == Focus::Editor {
            self.focus = Focus::Viewer;
        }
        if self.expanded_panel == Some(Focus::Editor) {
            self.expanded_panel = None;
        }
        self.terminal.needs_clear = true;
        Some(panel.path)
    }

    /// Discard the editor panel when the worktree it belongs to is being left.
    /// No reload/flash — the caller ([`on_worktree_changed`]) reloads the new
    /// worktree's view regardless.
    pub fn discard_editor_on_worktree_change(&mut self) {
        self.take_down_editor();
    }

    /// If an embedded editor is open and its process has exited (e.g. `:q`),
    /// tear it down and restore the normal layout. Returns `true` if it closed.
    /// Called every main-loop iteration so the panel disappears promptly rather
    /// than waiting on the slow dead-session cleanup timer.
    pub fn poll_editor_exit(&mut self) -> bool {
        let Some(idx) = self.editor.as_ref().map(|e| e.session_idx) else {
            return false;
        };
        if self.terminal.pty_manager.is_session_alive(idx) {
            return false;
        }
        self.exit_editor();
        true
    }

    /// Compute the editor PTY's content size (rows, cols) from the cached
    /// layout: the editor occupies the merged Explorer+Viewer region, minus the
    /// title row and borders (which collapse when the panel is maximized).
    fn editor_pty_size(&self) -> (u16, u16) {
        let cols = &self.layout_cache.columns;
        let region_w = cols[1].width.saturating_add(cols[2].width);
        let region_h = cols[1].height;
        let expanded = self.expanded_panel == Some(Focus::Editor);
        editor_content_size(region_w, region_h, expanded)
    }

    /// Reload the viewer file tree for the currently selected worktree.
    ///
    /// Preserves the currently open file and scroll position so that
    /// file-watcher refreshes don't disrupt the user's view.
    ///
    /// Returns `true` when the file tree's visible entries changed. Uses
    /// [`Self::selected_worktree_path`], which falls back to `repo_path` when
    /// there is no worktree, so the Explorer still shows the current folder's
    /// contents in a plain (non-git) directory.
    pub fn refresh_viewer(&mut self) -> bool {
        let path = self.selected_worktree_path();
        let tab_width = self.config.viewer.tab_width;
        let changed = self.viewer_state.load_file_tree(&path, tab_width);
        // Startup restore: this is the lazy (synchronous) tree-load path
        // (e.g. first time the viewer is focused), so re-open any pending
        // file here. The async worktree-switch path does this in
        // `poll_worktree_switch_ops`.
        self.consume_pending_view_restore();
        self.rehighlight_viewer();
        changed
    }

    /// Restore the previously selected worktree and seed its saved view
    /// (open file + scroll) for the current repo. Safe to call when nothing
    /// was persisted — it just leaves the defaults in place.
    ///
    /// Used at startup and when switching repos. The worktree list is already
    /// populated synchronously by [`App::refresh_worktrees`], so the selection
    /// is restored without a frame of flicker. The file itself is restored
    /// lazily once its tree loads (see [`App::consume_pending_view_restore`]).
    pub fn restore_selected_worktree_and_view(&mut self) {
        // Restore which worktree was selected (fall back to current on miss).
        let saved_branch = self
            .review_store
            .as_ref()
            .and_then(|s| s.get_selected_worktree().ok().flatten());
        if let Some(branch) = saved_branch
            && let Some(idx) = self.worktrees.iter().position(|w| w.branch == branch)
        {
            self.selected_worktree = idx;
        }

        // Point the worktree-list cursor at the restored worktree.
        self.rebuild_worktree_list_rows();
        let sel = self.selected_worktree;
        if let Some(pos) = self
            .worktree_list_rows
            .iter()
            .position(|r| matches!(r, WorktreeListRow::Worktree(i) if *i == sel))
        {
            self.worktree_list_selected = pos;
        }

        // Track the loaded worktree and seed its saved file/scroll.
        let branch = self.selected_worktree_branch();
        self.pending_view_restore = None;
        if branch.is_empty() {
            self.current_view_branch = None;
            return;
        }
        self.current_view_branch = Some(branch.clone());
        if let Some(store) = &self.review_store
            && let Ok(Some((Some(file), line))) = store.get_view_state(&branch)
        {
            self.pending_view_restore = Some(PendingViewRestore {
                file,
                scroll: line.max(0) as usize,
            });
        }
    }

    /// Persist the in-memory view (open file + scroll) for `branch`.
    ///
    /// If a restore is still pending (the user never opened the viewer for this
    /// worktree this session), the unconsumed pending value is written back
    /// unchanged so we don't clobber the saved state with an empty view.
    fn save_view_for(&self, branch: &str) {
        let Some(store) = &self.review_store else {
            return;
        };
        let (file, line) = match &self.pending_view_restore {
            Some(r) => (Some(r.file.clone()), r.scroll as i64),
            None => (
                self.viewer_state.content.current_file.clone(),
                self.viewer_state.content.file_scroll as i64,
            ),
        };
        let _ = store.save_view_state(branch, file.as_deref(), line);
    }

    /// Save the current worktree's view and selection. Called before exit /
    /// restart and before switching repos.
    pub fn persist_view_state(&self) {
        if let Some(branch) = &self.current_view_branch {
            self.save_view_for(branch);
            if let Some(store) = &self.review_store {
                let _ = store.set_selected_worktree(branch);
            }
        }
    }

    /// Consume a one-shot [`PendingViewRestore`]: open the saved file and
    /// scroll to the saved line. No-op if nothing is pending or the file no
    /// longer exists. The scroll target is clamped to the file length so a
    /// shrunken file doesn't leave a blank viewer.
    pub fn consume_pending_view_restore(&mut self) {
        let Some(restore) = self.pending_view_restore.take() else {
            return;
        };
        let wt_path = self.selected_worktree_path();
        if !wt_path.join(&restore.file).is_file() {
            return;
        }
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(&wt_path, &restore.file, tab_width);
        let max = self.viewer_state.content.file_content.len().saturating_sub(1);
        self.viewer_state.content.file_scroll = restore.scroll.min(max);
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
            self.diff_state
                .load_diff(&path, &base_branch, word_diff, tab_width);
            self.viewer_state.invalidate_diff_annotations();
        }
    }

    /// Set focus to a panel, lazily loading data when first needed.
    pub fn set_focus(&mut self, mut focus: Focus) {
        // While the embedded editor occupies the merged Explorer+Viewer region,
        // those two panels are hidden — any request to focus them lands on the
        // editor instead. Centralizing the redirect here keeps every focus path
        // (Tab cycle, alt+digit, click, palette) honoring the invariant without
        // each needing to know about the editor.
        if self.editor.is_some() && matches!(focus, Focus::Explorer | Focus::Viewer) {
            focus = Focus::Editor;
        }

        // The worktree column became a monitor strip + switcher modal, so
        // "focus the worktree" now opens that modal and leaves focus where it
        // was. This is the single chokepoint every worktree trigger funnels
        // through (Tab no longer reaches Worktree, super+1/`w`/palette/click all
        // call set_focus(Worktree)).
        if focus == Focus::Worktree {
            self.overlays.active = crate::overlay::ActiveOverlay::WorktreeSwitcher;
            return;
        }

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
        // Note: we deliberately do NOT close the reflow transcript here on a
        // plain focus change. Both the key handler (`event`) and the renderer
        // (`ui::terminal_claude`) gate reflow on `focus == TerminalClaude`, so
        // while another panel is focused the transcript neither captures keys
        // nor renders (the Claude panel falls back to the live PTY). Tearing it
        // down here would also reset the scroll offset, snapping the user back to
        // the live tail when they merely glanced at another panel. Reflow is
        // still closed on the transitions where the transcript becomes stale —
        // session switch (`switch_claude_session`) and worktree change
        // (`on_worktree_changed`) — and by Esc/F4 in the reflow key handler.

        match focus {
            Focus::Explorer | Focus::Viewer => {
                if self.viewer_state.tree.file_tree.is_empty() {
                    self.refresh_viewer();
                }
                if self.diff_state.committed_files.is_empty()
                    && self.diff_state.uncommitted_files.is_empty()
                {
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
        // A panel's transient search prompt is modal to that panel; moving focus
        // away must release key capture. Otherwise the search box keeps eating
        // keystrokes after focus moves (e.g. `/` in the viewer, then Tab to
        // Claude — input should go to Claude). The query/matches persist so n/N
        // still work when you return.
        if focus != Focus::Viewer {
            self.viewer_state.search.search_active = false;
        }
        if matches!(
            focus,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        ) {
            self.viewer_state.filename_search.filename_search_active = false;
        }
        // Record the change so the gaining/losing panels can glide their border
        // color (only on an actual change, so a re-focus doesn't restart it).
        if self.focus != focus {
            self.focus_prev = self.focus;
            self.focus_changed_at = std::time::Instant::now();
        }
        self.focus = focus;
    }

    /// Border color for `panel`, eased across focus changes: the panel gaining
    /// focus glides `border_unfocused → border_focused`, the one losing it
    /// glides back, over `anim::FOCUS_MS`. Everything else rests on the static
    /// unfocused color. This is what makes panel switches feel smooth instead of
    /// snapping, using the theme's RGB colors and `Theme::lerp`.
    pub fn animated_border_color(&self, panel: Focus) -> ratatui::style::Color {
        let t = crate::anim::eased_progress(self.focus_changed_at.elapsed(), crate::anim::FOCUS_MS);
        if self.focus == panel {
            if t >= 1.0 {
                self.theme.border_focused
            } else {
                crate::theme::Theme::lerp(self.theme.border_unfocused, self.theme.border_focused, t)
            }
        } else if self.focus_prev == panel && t < 1.0 {
            crate::theme::Theme::lerp(self.theme.border_focused, self.theme.border_unfocused, t)
        } else {
            self.theme.border_unfocused
        }
    }

    /// Whether a UI transition (currently the focus-border glide) is still in
    /// flight. The main loop uses this to keep redrawing at the active frame
    /// rate so the transition actually animates instead of stalling at the idle
    /// tick rate.
    pub fn has_active_transition(&self) -> bool {
        self.focus_changed_at.elapsed() < std::time::Duration::from_millis(crate::anim::FOCUS_MS)
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

    /// Switch the active UI theme at runtime.
    ///
    /// When `persist` is `true`, the selection is written to the config file
    /// (`~/.config/conductor/config.toml`) so it survives restarts. A write
    /// failure is non-fatal: it is logged and surfaced as a warning flash.
    pub fn set_theme(&mut self, name: &str, persist: bool) {
        self.theme = Theme::from_name(name);
        self.theme_name = name.to_string();
        self.config.ui.theme = Some(name.to_string());
        if persist
            && let Err(e) = crate::config::persist_ui_theme(name)
        {
            log::warn!("failed to persist theme '{name}': {e}");
            self.set_status(
                format!("Theme saved in session but could not write config: {e}"),
                StatusLevel::Warning,
            );
        }
    }

    // ── Live config reload ─────────────────────────────────────────

    /// Apply appearance (live-reloadable) fields from `new` to the running app.
    ///
    /// Only the fields classified as LIVE are copied; restart-required fields
    /// (shell, scrollback limits, API settings, etc.) are intentionally left
    /// untouched so that `refresh_diff`, which reads `config.general.main_branch`
    /// on every call, never sees a stale or transitional value.
    ///
    /// ## LIVE fields applied here
    /// - `ui.theme` / `viewer.theme` → theme + theme_name + syntect rebuild
    /// - `viewer.syntax_theme_file`  → syntect rebuild (same path as theme)
    /// - `viewer.tab_width`          → config copy + refresh_viewer + refresh_diff
    /// - `diff.word_diff`            → config copy + refresh_diff
    /// - `diff.default_view`         → diff_state.view_mode + refresh_diff
    /// - `general.decoration`        → config copy (drawn directly each frame)
    /// - `layout.*`                  → config copy; LayoutCache auto-invalidates
    ///
    /// `viewer.word_wrap` is copied into config via `adopt_appearance` but is not
    /// in `AppearanceSnapshot` and has no rendering effect until the render path
    /// is implemented.
    pub fn apply_appearance(&mut self, new: &config::Config) {
        // ── UI / syntax theme ──────────────────────────────────────
        let new_theme_name = resolve_theme_name(new);
        if new_theme_name != self.theme_name {
            self.theme = Theme::from_name(&new_theme_name);
            self.theme_name = new_theme_name;
        }

        // Rebuild syntect theme when either the viewer theme or the custom
        // theme file changes (the two are bundled into a single re-construction
        // so there is never a half-updated state).
        let ts = ThemeSet::load_defaults();
        self.syntect_theme = config::syntect_theme_for(&new.viewer, &ts);

        // Clear the Markdown cache so code blocks inside review comments pick
        // up the new syntect theme. The cache fingerprints the UI colour palette
        // only; a syntax-only change would otherwise leave stale highlighted spans.
        self.markdown_cache.clear();

        // Force a full rebuild of the reflow transcript on the next render so
        // that Markdown spans pick up the new theme colours and syntect palette.
        // Setting last_width=0 makes build_lines run on the next frame regardless
        // of whether the panel width changed.
        self.reflow.last_width = 0;
        self.reflow.cache.clear();

        // ── Diff view mode ──────────────────────────────────────────
        // Apply view_mode directly. `diff_state.view_mode` is written only in
        // `DiffState::new` and here — there is no runtime interactive toggle —
        // so overwriting it is safe.
        self.diff_state.view_mode = crate::diff_state::DiffViewMode::from(new.diff.default_view);

        // Copy all live config fields (no-op for restart-required fields).
        // LayoutCache keyed on layout proportions detects changes automatically
        // and recomputes on the next frame; no explicit invalidation needed.
        self.config.adopt_appearance(new);

        // The Claude/Shell split is a runtime field seeded from config; resync it
        // so an external edit to layout.terminal_split_pct takes effect live. Our
        // own resize-driven writes never reach here — they leave the appearance
        // snapshot unchanged, so reload_appearance_config short-circuits first.
        self.terminal_split_pct = self
            .config
            .layout
            .terminal_split_pct
            .clamp(Self::TERMINAL_SPLIT_MIN, Self::TERMINAL_SPLIT_MAX);

        // Refresh the viewer file tree + diff to pick up tab_width / word_diff.
        // refresh_viewer calls rehighlight_viewer unconditionally, so the new
        // syntect theme is applied to the open file as part of this call.
        self.refresh_viewer();
        self.refresh_diff();

        // Trigger a full redraw.
        self.dirty.mark_all();
    }

    /// Reload the config file and apply any appearance changes.
    ///
    /// 1. Guards against the config file being absent (e.g., a remove event from
    ///    a delete-then-write atomic save): skips loading to avoid `Config::load()`
    ///    writing a default file and clobbering the user's in-progress edits.
    /// 2. Loads `~/.config/conductor/config.toml`; on parse error, flashes an
    ///    error message and returns without modifying the running config.
    /// 3. Computes whether appearance fields and/or restart-required fields changed.
    ///    True no-op (neither changed) → returns silently, which is also the guard
    ///    that absorbs the self-write loop from the in-app theme picker.
    /// 4. If restart-required fields changed, flashes a warning.
    /// 5. If appearance fields changed, calls `apply_appearance` and (when no
    ///    restart warning was issued) flashes an info confirmation.
    pub fn reload_appearance_config(&mut self) {
        // Guard: skip if the file was just deleted (remove event from an atomic
        // editor save). Config::load() on a missing file would write defaults and
        // return Config::default(), clobbering the user's work.
        if !config::config_file_path().exists() {
            return;
        }

        let new = match config::Config::load() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("config reload: failed to parse config file: {e}");
                self.set_status(
                    format!("Config error — kept previous settings: {e}"),
                    StatusLevel::Error,
                );
                return;
            }
        };

        let appearance_changed = new.appearance_snapshot() != self.config.appearance_snapshot();
        let restart_changed = config::has_restart_changes(&self.config, &new);

        // True no-op: nothing changed. This absorbs the FS event from the in-app
        // theme picker (ui.theme is appearance-only, so both flags are false when
        // the picker persists a theme that the running config already reflects).
        if !appearance_changed && !restart_changed {
            return;
        }

        if restart_changed {
            self.set_status(
                String::from("Config updated — some changes require a restart to take effect"),
                StatusLevel::Warning,
            );
        }

        if appearance_changed {
            self.apply_appearance(&new);
            if !restart_changed {
                self.set_status_info(String::from("Config reloaded"));
            }
        }
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
            d.file_path == *cur_file && (d.line as isize - cursor_line as isize).unsigned_abs() <= 2
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
        let scroll = target_0
            .saturating_sub(source_screen_row)
            .min(total.saturating_sub(1));
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
                self.viewer_state
                    .open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state
                    .reveal_file_in_tree(&loc.file_path, &wt_path);
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
                self.viewer_state
                    .open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state
                    .reveal_file_in_tree(&loc.file_path, &wt_path);
            }
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
        }
    }

    /// Start building the symbol index in the background.
    pub fn start_symbol_index_build(&mut self) {
        let index = self.symbol_index.clone();
        self.bg.symbol_index.start(move |tx| {
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

        self.viewer_state
            .open_file(&wt_path, relative_path, tab_width);
        self.viewer_state
            .reveal_file_in_tree(relative_path, &wt_path);

        if let Some(ln) = line {
            let max = self
                .viewer_state
                .content
                .file_content
                .len()
                .saturating_sub(1);
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
            CommandId::NextWorktree => self.select_next_worktree(),
            CommandId::PrevWorktree => self.select_prev_worktree(),
            CommandId::TogglePanelExpand => self.cmd_toggle_panel_expand(),
            CommandId::ResizePaneLeft => self.resize_focused_pane(ResizeDir::Left),
            CommandId::ResizePaneRight => self.resize_focused_pane(ResizeDir::Right),
            CommandId::ResizePaneUp => self.resize_focused_pane(ResizeDir::Up),
            CommandId::ResizePaneDown => self.resize_focused_pane(ResizeDir::Down),
            CommandId::CreateWorktree => self.cmd_create_worktree(),
            CommandId::DeleteWorktree => self.cmd_delete_worktree(),
            CommandId::SwitchBranch => self.cmd_switch_branch(),
            CommandId::GrabBranch => self.cmd_grab_branch(),
            CommandId::PruneWorktrees => self.cmd_prune_worktrees(),
            CommandId::MergeToMain => self.cmd_merge_to_main(),
            CommandId::RefreshWorktrees => {
                let _ = self.refresh_worktrees();
            }
            CommandId::ResetMainToOrigin => self.cmd_reset_main_to_origin(),
            CommandId::CherryPick => self.cmd_cherry_pick(),
            CommandId::PullWorktree => self.start_pull_worktree(),
            CommandId::NewClaudeCode => self.cmd_new_claude_code(),
            CommandId::NewShell => self.cmd_new_shell(),
            CommandId::ResumeClaudeSession => self.cmd_resume_claude_session(),
            CommandId::RefreshDiff => self.refresh_diff(),
            CommandId::SearchInFile => self.cmd_search_in_file(),
            CommandId::ToggleHelp => self.cmd_toggle_help(),
            CommandId::ShowReviewComments => self.cmd_show_review_comments(),
            CommandId::ShowReviewTemplates => {
                self.review_state.template_picker_active = true;
            }
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
            CommandId::TogglePartyMode => self.cmd_toggle_party_mode(),
            CommandId::ToggleRichMode => self.cmd_toggle_rich_mode(),
            CommandId::Quit => self.should_quit = true,
            CommandId::SwitchTheme => self.cmd_open_theme_picker(),
        }
    }

    /// Open the theme picker overlay.
    ///
    /// Captures `theme_name` as the revert target so Esc can restore the theme
    /// that was active when the picker opened (even after live-preview moves).
    pub fn cmd_open_theme_picker(&mut self) {
        let themes: Vec<String> = crate::theme::Theme::all_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let selected = themes
            .iter()
            .position(|t| t == &self.theme_name)
            .unwrap_or(0);
        self.overlays.theme_picker = crate::overlay::ThemePickerOverlay {
            themes,
            selected,
            original: self.theme_name.clone(),
        };
        self.overlays.active = ActiveOverlay::ThemePicker;
    }

    // ── Command palette handler methods ──────────────────────────────

    /// Toggle the hidden party theme mode (rainbow borders, flashy syntax,
    /// confetti). A flash message confirms the new state; the whole UI is
    /// re-rendered so the effect appears/disappears immediately.
    fn cmd_toggle_party_mode(&mut self) {
        self.party_mode = !self.party_mode;
        if self.party_mode {
            self.set_status("🎉 Party mode ON! 🎉".to_string(), StatusLevel::Success);
        } else {
            self.set_status_info("Party mode off.".to_string());
        }
        self.dirty.mark_all();
    }

    /// Toggle rich mode between off and the tier detected at startup. On
    /// terminals where detection found nothing, toggling on falls back to
    /// Tier A (same behaviour as `[rich] mode = "force"`).
    fn cmd_toggle_rich_mode(&mut self) {
        use crate::term_caps::RichTier;
        if self.rich_tier.is_rich() {
            self.rich_tier = RichTier::Off;
            self.set_status_info("Rich mode off.".to_string());
        } else {
            self.rich_tier = if self.rich_tier_available.is_rich() {
                self.rich_tier_available
            } else {
                RichTier::TierA
            };
            self.set_status("✨ Rich mode ON".to_string(), StatusLevel::Success);
        }
        self.dirty.mark_all();
    }

    fn cmd_toggle_panel_expand(&mut self) {
        if self.expanded_panel == Some(self.focus) {
            self.expanded_panel = None;
        } else {
            self.expanded_panel = Some(self.focus);
        }
    }

    /// Step (percentage points) each horizontal pane resize moves a column
    /// divider.
    const RESIZE_STEP_PCT: u16 = 5;
    /// Minimum width percentage for each of the three columns (Explorer, Viewer,
    /// Terminal), so a tmux-style resize can never collapse a column to nothing.
    const MIN_COL_PCT: u16 = 10;
    /// Step (percentage points) each vertical pane resize moves the Claude/Shell
    /// divider.
    const TERMINAL_SPLIT_STEP: u16 = 5;
    /// Bounds for the runtime Claude-area percentage, leaving at least this much
    /// for each of the two terminal panes so neither can vanish.
    const TERMINAL_SPLIT_MIN: u16 = 20;
    const TERMINAL_SPLIT_MAX: u16 = 80;

    /// Resize the focused panel tmux-style, growing it toward `dir`.
    ///
    /// Maps the focused panel and direction onto one of the three adjustable
    /// dividers (Explorer|Viewer, Viewer|Terminal, Claude|Shell). The focused
    /// panel grows toward `dir` by moving the divider it shares with its
    /// neighbor on that side; against an edge it moves the only divider it has,
    /// shrinking instead — mirroring `resize-pane -L/-R/-U/-D`. The middle
    /// (Viewer) column can therefore push both of its borders, so it never
    /// becomes the cramped pane that can only shrink.
    pub fn resize_focused_pane(&mut self, dir: ResizeDir) {
        let step = Self::RESIZE_STEP_PCT as i16;
        match dir {
            ResizeDir::Left | ResizeDir::Right => {
                let grow_right = matches!(dir, ResizeDir::Right);
                match self.focus {
                    // The worktree strip is full-width, not one of the three
                    // resizable columns — nothing to resize from there.
                    Focus::Worktree => {}
                    // Leftmost column: left/right ride the Explorer|Viewer divider.
                    Focus::Explorer => {
                        self.move_explorer_viewer_divider(if grow_right { step } else { -step });
                    }
                    // Middle column pushes whichever border faces `dir`.
                    Focus::Viewer => {
                        if grow_right {
                            self.move_viewer_terminal_divider(step);
                        } else {
                            self.move_explorer_viewer_divider(-step);
                        }
                    }
                    // Rightmost column: left grows it (shrinks Viewer), right shrinks it.
                    Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => {
                        self.move_viewer_terminal_divider(if grow_right { step } else { -step });
                    }
                }
            }
            ResizeDir::Up | ResizeDir::Down => {
                // Two columns have a vertical split: the terminal (Claude/Shell)
                // and the Explorer (file tree / changed files). Down grows the
                // top pane, Up shrinks it.
                let down = matches!(dir, ResizeDir::Down);
                match self.focus {
                    Focus::TerminalClaude | Focus::TerminalShell => {
                        let step = Self::TERMINAL_SPLIT_STEP as i16;
                        self.adjust_terminal_split(if down { step } else { -step });
                    }
                    Focus::Explorer => {
                        let step = Self::TERMINAL_SPLIT_STEP as i16;
                        self.adjust_explorer_split(if down { step } else { -step });
                    }
                    _ => {}
                }
            }
        }
    }

    /// Move the Explorer|Viewer divider by `delta` points (positive = rightward,
    /// enlarging Explorer and shrinking Viewer). Terminal width is conserved.
    /// Clamped so neither Explorer nor Viewer drops below [`Self::MIN_COL_PCT`].
    fn move_explorer_viewer_divider(&mut self, delta: i16) {
        let (new_e, new_v) = clamp_ev_divider(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            delta,
            Self::MIN_COL_PCT,
        );
        if new_e == self.config.layout.explorer_width_pct {
            return;
        }
        self.config.layout.explorer_width_pct = new_e;
        self.config.layout.viewer_width_pct = new_v;
        self.after_horizontal_resize();
    }

    /// Move the Viewer|Terminal divider by `delta` points (positive = rightward,
    /// enlarging Viewer and shrinking Terminal). Explorer width is unchanged.
    /// Clamped so neither Viewer nor Terminal drops below [`Self::MIN_COL_PCT`].
    fn move_viewer_terminal_divider(&mut self, delta: i16) {
        let new_v = clamp_vt_divider(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            delta,
            Self::MIN_COL_PCT,
        );
        if new_v == self.config.layout.viewer_width_pct {
            return;
        }
        self.config.layout.viewer_width_pct = new_v;
        self.after_horizontal_resize();
    }

    /// Shared tail for a column resize: redraw, flash the new split, and persist
    /// the ratios so they survive a restart.
    fn after_horizontal_resize(&mut self) {
        self.dirty.mark_all();
        let e = self.config.layout.explorer_width_pct;
        let v = self.config.layout.viewer_width_pct;
        let t = 100u16.saturating_sub(e.saturating_add(v));
        self.set_status_info(format!("Layout: Explorer {e}% / Viewer {v}% / Terminal {t}%"));
        self.persist_layout();
    }

    /// Adjust the runtime Claude-area height percentage by `delta` points,
    /// clamped so both the Claude and Shell panes keep a usable minimum. A
    /// positive `delta` enlarges the Claude pane (shrinks the Shell); negative
    /// enlarges the Shell. Flashes the resulting split and persists the ratio.
    fn adjust_terminal_split(&mut self, delta: i16) {
        let next = (self.terminal_split_pct as i16 + delta)
            .clamp(Self::TERMINAL_SPLIT_MIN as i16, Self::TERMINAL_SPLIT_MAX as i16)
            as u16;
        if next == self.terminal_split_pct {
            return;
        }
        self.terminal_split_pct = next;
        // Keep the in-memory config in sync so the appearance snapshot matches
        // what we write below — that makes the config watcher's reload a no-op
        // (it only reacts when the snapshot differs), avoiding a self-write loop.
        self.config.layout.terminal_split_pct = next;
        self.dirty.mark_all();
        self.set_status_info(format!(
            "Terminal split: Claude {next}% / Shell {}%",
            100 - next
        ));
        self.persist_layout();
    }

    /// Adjust the Explorer column's file-tree height percentage by `delta`
    /// points (positive grows the file tree, shrinking the changed-files list),
    /// clamped so both panels keep a usable minimum. Flashes and persists.
    fn adjust_explorer_split(&mut self, delta: i16) {
        let next = (self.config.layout.explorer_split_pct as i16 + delta)
            .clamp(Self::TERMINAL_SPLIT_MIN as i16, Self::TERMINAL_SPLIT_MAX as i16)
            as u16;
        if next == self.config.layout.explorer_split_pct {
            return;
        }
        self.config.layout.explorer_split_pct = next;
        self.dirty.mark_all();
        self.set_status_info(format!(
            "Explorer split: tree {next}% / changed files {}%",
            100 - next
        ));
        self.persist_layout();
    }

    /// Persist the current panel proportions to `config.toml`. Best-effort: a
    /// write failure is logged, never fatal (the in-memory layout still applies).
    fn persist_layout(&self) {
        if let Err(e) = crate::config::persist_layout_proportions(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            self.terminal_split_pct,
            self.config.layout.explorer_split_pct,
        ) {
            log::warn!("failed to persist layout proportions: {e}");
        }
    }

    fn cmd_create_worktree(&mut self) {
        self.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
        self.worktree_mgr.input_buffer.clear();
        self.set_status_info(
            "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):".to_string(),
        );
    }

    fn cmd_delete_worktree(&mut self) {
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            if wt.is_main {
                self.set_status(
                    "Cannot delete the main worktree.".to_string(),
                    StatusLevel::Warning,
                );
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
            self.set_status(
                "Already grabbing a branch. Ungrab first (G).".to_string(),
                StatusLevel::Warning,
            );
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
                self.set_status(
                    "Cannot merge main into itself.".to_string(),
                    StatusLevel::Warning,
                );
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

    /// Ask before resetting main — this discards local commits, so it must not
    /// fire on a bare keystroke (`R` sits next to `r` refresh). The actual reset
    /// runs in [`perform_reset_main_to_origin`](Self::perform_reset_main_to_origin)
    /// once confirmed. Both the `R` key and the palette enter through here.
    pub fn cmd_reset_main_to_origin(&mut self) {
        let main_branch = self.config.general.main_branch.clone();
        self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingReset;
        self.set_status_info(format!(
            "Reset '{main_branch}' to origin? Discards local commits on it. (y/n)"
        ));
    }

    /// Perform the hard reset of main to its origin tracking branch. Call only
    /// after the user confirms (see [`cmd_reset_main_to_origin`](Self::cmd_reset_main_to_origin)).
    pub fn perform_reset_main_to_origin(&mut self) {
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
        let source = self
            .worktrees
            .iter()
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
            self.set_status(
                format!("Failed to start Claude Code: {e}"),
                StatusLevel::Error,
            );
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
        self.overlays
            .open_repo
            .buffer
            .set_text(&self.repo_path.display().to_string());
    }

    fn cmd_switch_repo(&mut self) {
        if self.repo_list.len() > 1 {
            self.overlays.active = ActiveOverlay::RepoSelector;
            self.overlays.repo_selector.selected = self.repo_list_index;
        }
    }

    fn cmd_ungrab_branch(&mut self) {
        if self.worktree_mgr.grabbed_branch.is_none() {
            self.set_status(
                "Not grabbing — nothing to ungrab.".to_string(),
                StatusLevel::Warning,
            );
        } else {
            self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingUngrab;
            self.set_status(
                "Ungrab? Main will return to main branch. (y/n)".to_string(),
                StatusLevel::Warning,
            );
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

    pub fn cmd_add_review_comment(&mut self) {
        if let Some(file_path) = self.viewer_state.content.current_file.clone() {
            // Anchor the comment to the selected range (or the top visible line),
            // then open a body-only inline compose box at that line — no
            // `file:line` prefix to type, GitHub-style.
            let (start, end) = if let Some((start, end)) = self.viewer_state.selected_range() {
                (start as u32, if start == end { None } else { Some(end as u32) })
            } else {
                ((self.viewer_state.content.file_scroll + 1) as u32, None)
            };
            self.viewer_state.clear_selection();
            self.review_state.input_anchor = Some((file_path, start, end));
            self.review_state.input_buffer.clear();
            self.review_state.input_kind = crate::review_store::CommentKind::Suggest;
            self.review_state.input_mode = crate::review_state::ReviewInputMode::AddingComment;
            self.review_state.status_message = None;
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
            if let Some(comments) = self.review_state.file_comments.get(&cursor_line)
                && !comments.is_empty()
            {
                let target_id = &comments[0].id;
                if let Some(idx) = self
                    .review_state
                    .comments
                    .iter()
                    .position(|c| c.id == *target_id)
                {
                    let cid = target_id.clone();
                    if !self.review_state.cached_replies.contains_key(&cid)
                        && let Some(store) = self.review_store.as_ref()
                        && let Ok(replies) = store.get_replies(&cid)
                    {
                        self.review_state.cached_replies.insert(cid, replies);
                    }
                    self.review_state.comment_detail_idx = idx;
                    self.review_state.comment_detail_scroll = 0;
                    self.review_state.comment_detail_active = true;
                    self.set_focus(Focus::Viewer);
                    return;
                }
            }
        }
        self.set_status(
            "No comment on current line.".to_string(),
            StatusLevel::Warning,
        );
    }

    fn cmd_delete_comment(&mut self) {
        if self.viewer_state.explorer.explorer_show_comments
            && self.viewer_state.explorer.explorer_focus_on_diff_list
            && !self.review_state.comment_list_rows.is_empty()
        {
            self.request_delete_selected_review_item();
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
        self.overlays.grep_search.input_focused = true;
    }

    /// Show the update confirmation dialog.
    pub fn start_update_confirm(&mut self) {
        self.update_state = UpdateState::Confirming;
    }

    /// Kick off the background update thread.
    pub fn start_update_download(&mut self) {
        let Some(ref info) = self.update_info else {
            return;
        };
        let version = info.latest_version.clone();
        let assets = info.assets.clone();

        self.update_state = UpdateState::InProgress;
        self.update_progress_message = "Preparing update...".to_string();

        self.update_op.start(move |tx| {
            perform_update(&tx, &version, &assets);
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
        if let Some(info) = self.bg.ccusage.poll() {
            self.ccusage_info = Some(info);
        }

        // symbol index
        if let Some(result) = self.bg.symbol_index.poll() {
            match result {
                Ok(count) => {
                    log::info!("Symbol index built: {count} symbols");
                    self.set_status(
                        format!("Symbol index ready ({count} symbols)"),
                        StatusLevel::Success,
                    );
                }
                Err(msg) => {
                    log::warn!("Symbol index build failed: {msg}");
                }
            }
        }

        // update check
        if let Some(Some(info)) = self.bg.update_check.poll()
            && crate::update_checker::is_newer(
                &info.latest_version,
                crate::update_checker::current_version(),
            )
        {
            self.update_info = Some(info);
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
        // Worktree is no longer a focusable column (it became the top strip +
        // switcher modal), so it's excluded from the Tab cycle.
        // When the editor is open it stands in for Explorer+Viewer in the cycle;
        // `set_focus` redirects any Explorer/Viewer target onto it, so the only
        // explicit arm needed is leaving the editor itself.
        //
        // The Explorer column holds two independent panels — the file tree and
        // the changed-files list — so Tab visits each as its own stop (file tree
        // → changed files → Viewer), toggling the sub-focus before moving on.
        if self.editor.is_none()
            && self.focus == Focus::Explorer
            && !self.viewer_state.explorer.explorer_focus_on_diff_list
        {
            self.viewer_state.explorer.explorer_focus_on_diff_list = true;
            self.focus_changed_at = std::time::Instant::now();
            return;
        }
        let next = match self.focus {
            Focus::Worktree | Focus::TerminalShell => Focus::Explorer,
            Focus::Explorer => Focus::Viewer,
            Focus::Viewer => Focus::TerminalClaude,
            Focus::Editor => Focus::TerminalClaude,
            Focus::TerminalClaude => Focus::TerminalShell,
        };
        // Landing on the Explorer column from elsewhere always starts on the
        // file tree (the top panel).
        if next == Focus::Explorer {
            self.viewer_state.explorer.explorer_focus_on_diff_list = false;
        }
        self.set_focus(next);
    }

    /// Cycle focus backward.
    pub fn cycle_focus_backward(&mut self) {
        // Mirror of the forward cycle: stepping back through the Explorer column
        // visits changed files then the file tree.
        if self.editor.is_none()
            && self.focus == Focus::Explorer
            && self.viewer_state.explorer.explorer_focus_on_diff_list
        {
            self.viewer_state.explorer.explorer_focus_on_diff_list = false;
            self.focus_changed_at = std::time::Instant::now();
            return;
        }
        let prev = match self.focus {
            Focus::Worktree | Focus::Explorer => Focus::TerminalShell,
            Focus::Viewer => Focus::Explorer,
            Focus::Editor => Focus::TerminalShell,
            Focus::TerminalClaude => Focus::Viewer,
            Focus::TerminalShell => Focus::TerminalClaude,
        };
        // Entering the Explorer column from the Viewer side lands on the
        // changed-files panel (nearest), so a further Tab-back reaches the tree.
        if prev == Focus::Explorer {
            self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        }
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
                    rows.push(WorktreeListRow::Session {
                        wt_idx: i,
                        pty_idx: *pty_idx,
                    });
                }
            }
        }
        self.worktree_list_rows = rows;
        // Clamp selected index.
        if !self.worktree_list_rows.is_empty()
            && self.worktree_list_selected >= self.worktree_list_rows.len()
        {
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
    assets: &[crate::update_checker::ReleaseAsset],
) {
    let tmpdir = std::env::temp_dir().join(format!("conductor-update-{version}"));
    let _ = std::fs::remove_dir_all(&tmpdir);
    if std::fs::create_dir_all(&tmpdir).is_err() {
        let _ = tx.send(UpdateProgress::Error(
            "Failed to create temp directory".to_string(),
        ));
        return;
    }

    let installed = try_binary_update(tx, version, assets, &tmpdir);
    let _ = std::fs::remove_dir_all(&tmpdir);

    if installed {
        let _ = tx.send(UpdateProgress::Done(format!(
            "v{version} installed successfully! Restarting..."
        )));
    } else {
        // Deliberately no in-app source build: compiling inside the TUI is
        // slow and fragile, and anyone able to build from source can run the
        // command themselves. Point them at the manual path instead.
        let _ = tx.send(UpdateProgress::Error(
            "Could not install the pre-built binary. Update manually with \
             `cargo install --path .` or download a binary from the releases page."
                .to_string(),
        ));
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
    let new_binary = tmpdir.join("conductor");
    if !new_binary.exists() {
        log::warn!("conductor binary not found in archive");
        return false;
    }

    // Install over the *currently running* executable, resolved to its real
    // path. Guessing `~/.cargo/bin/conductor` would silently update the wrong
    // file when conductor was launched from elsewhere (Homebrew prefix,
    // /usr/local/bin, a symlink), leaving the user's actual binary untouched.
    let dest = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("could not resolve current executable: {e}");
            return false;
        }
    };
    let Some(dest_dir) = dest.parent().map(|d| d.to_path_buf()) else {
        log::warn!("executable has no parent directory");
        return false;
    };

    // Stage the new binary in the *same directory* as `dest` so the final swap
    // can be an atomic rename(2). A cross-filesystem rename fails with EXDEV
    // and would silently degrade to a copy, which is exactly the bug we avoid.
    let staged = dest_dir.join(format!(".conductor-update-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    let _ = tx.send(UpdateProgress::Status("Installing binary...".to_string()));
    if let Err(e) = std::fs::copy(&new_binary, &staged) {
        log::warn!("failed to stage binary: {e}");
        return false;
    }

    // Executable permission on the staged file (set before the swap).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }

    // Strip the macOS quarantine xattr so Gatekeeper won't block it. (The code
    // signature itself is embedded in the Mach-O, not an xattr; this only
    // clears `com.apple.quarantine`.)
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr").args(["-cr"]).arg(&staged).output();
    }

    // Verify the staged binary actually launches before swapping it in — this
    // catches corrupt/truncated downloads. (It does NOT exercise the
    // in-place-overwrite SIGKILL; that class is prevented structurally by the
    // atomic rename below, since `staged` is a brand-new inode.)
    if !verify_runnable(&staged) {
        log::warn!("staged binary failed to launch; aborting install");
        let _ = std::fs::remove_file(&staged);
        return false;
    }

    // Back up the current binary, then atomically swap in the new one.
    // `rename(2)` rebinds the path to a fresh inode, so the still-running
    // process keeps executing from the old (now-unlinked) inode and the next
    // `exec` sees a clean, validly-signed file. Overwriting `dest` in place
    // (the previous `fs::copy`) corrupted the running binary's code-signing
    // state on macOS arm64 and got it SIGKILLed on every subsequent launch.
    let backup = dest_dir.join(".conductor.bak");
    let _ = std::fs::remove_file(&backup);
    if let Err(e) = std::fs::rename(&dest, &backup) {
        log::warn!("failed to back up current binary: {e}");
        let _ = std::fs::remove_file(&staged);
        return false;
    }
    if let Err(e) = std::fs::rename(&staged, &dest) {
        log::warn!("failed to install new binary: {e}; rolling back");
        let _ = std::fs::rename(&backup, &dest);
        let _ = std::fs::remove_file(&staged);
        return false;
    }
    // Success — the new binary is verified and in place; discard the backup.
    let _ = std::fs::remove_file(&backup);

    true
}

/// Spawn `path --version` and report whether it exits successfully.
///
/// Used as a pre-install smoke test: a freshly downloaded binary that can't
/// even print its version (corrupt download, wrong arch, bad signature) must
/// not replace the working one.
fn verify_runnable(path: &std::path::Path) -> bool {
    use std::process::{Command, Stdio};
    match Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(e) => {
            log::warn!("failed to spawn staged binary for verification: {e}");
            false
        }
    }
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
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

/// Pick the worktree index to keep selected after the worktree list is
/// refreshed.
///
/// The list order is not stable across refreshes — adding or removing a
/// worktree shifts every index after it. Selecting purely by the old index
/// would silently re-point the selection at a *different* branch, which then
/// shows that branch's review data (including the change summary) against the
/// wrong worktree. So we re-pin by branch identity first; only when the
/// previously selected branch is gone do we clamp the old index into range.
///
/// Returns `None` when there are no worktrees (nothing to select).
fn reselect_worktree_index(
    worktrees: &[git_engine::WorktreeInfo],
    prev_branch: &str,
    old_index: usize,
) -> Option<usize> {
    if worktrees.is_empty() {
        return None;
    }
    if !prev_branch.is_empty()
        && let Some(idx) = worktrees.iter().position(|w| w.branch == prev_branch)
    {
        return Some(idx);
    }
    Some(old_index.min(worktrees.len() - 1))
}

/// Resolve the absolute path to hand an external editor from the viewer's
/// relative `current_file` and the worktree root. `None` (no file open, or an
/// empty path) means "nothing to edit" — the caller flashes a hint rather than
/// launching an editor on a bogus target.
fn editor_target(current_file: Option<&str>, worktree_root: &std::path::Path) -> Option<PathBuf> {
    let rel = current_file?;
    if rel.is_empty() {
        return None;
    }
    Some(worktree_root.join(rel))
}

/// Content size (rows, cols) for the embedded editor PTY given its region size
/// and whether it is maximized. The title row is always present; non-maximized
/// also has a bottom border row and left/right border columns. A zero region
/// (layout not computed yet) seeds a reasonable default — the per-frame resize
/// in `sync_pty_sizes` corrects it. Never returns 0 in either dimension (vt100
/// needs at least 1×1).
fn editor_content_size(region_w: u16, region_h: u16, expanded: bool) -> (u16, u16) {
    if region_w == 0 || region_h == 0 {
        return (24, 80);
    }
    let border_rows: u16 = if expanded { 1 } else { 2 };
    let border_cols: u16 = if expanded { 0 } else { 2 };
    (
        region_h.saturating_sub(border_rows).max(1),
        region_w.saturating_sub(border_cols).max(1),
    )
}

/// Resolve the editor command line from `$VISUAL` / `$EDITOR`, falling back to
/// `fallback`. Empty or whitespace-only values are ignored so a stray
/// `EDITOR=""` doesn't produce an empty command. The chosen value is split on
/// whitespace into program + arguments (so `"code -w"` works); an editor whose
/// *path* contains spaces is intentionally not supported (no shell-style
/// quoting — editor-flavor handling is out of scope).
fn resolve_editor_command(
    visual: Option<&str>,
    editor: Option<&str>,
    fallback: &str,
) -> Vec<String> {
    let chosen = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(fallback);
    let parts: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
    if parts.is_empty() {
        vec![fallback.to_string()]
    } else {
        parts
    }
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

/// Compute the new `(explorer, viewer)` width percentages after moving the
/// Explorer|Viewer divider by `delta` points. Explorer+Viewer is conserved
/// (Terminal width is untouched), and both columns are kept `>= min`. A `delta`
/// that would push a column below the floor is clamped, so the divider stops at
/// the boundary rather than overshooting.
fn clamp_ev_divider(explorer: u16, viewer: u16, delta: i16, min: u16) -> (u16, u16) {
    let e = explorer as i16;
    let v = viewer as i16;
    let min = min as i16;
    let upper = (e + v - min).max(min);
    let new_e = (e + delta).clamp(min, upper);
    (new_e as u16, (e + v - new_e) as u16)
}

/// Compute the new Viewer width percentage after moving the Viewer|Terminal
/// divider by `delta` points. Explorer is untouched; Viewer and the implicit
/// Terminal column (`100 - explorer - viewer`) are each kept `>= min`.
fn clamp_vt_divider(explorer: u16, viewer: u16, delta: i16, min: u16) -> u16 {
    let e = explorer as i16;
    let v = viewer as i16;
    let min = min as i16;
    // Terminal = 100 - E - V, so keep new V in [min, 100 - E - min].
    let upper = (100 - e - min).max(min);
    (v + delta).clamp(min, upper) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── tmux-style pane resize divider math ──────────────────────────

    const MIN: u16 = 10;

    #[test]
    fn ev_divider_moves_space_between_explorer_and_viewer() {
        // Growing Explorer (delta +5) takes 5 points from Viewer; Terminal
        // (the conserved remainder) is untouched.
        assert_eq!(clamp_ev_divider(24, 38, 5, MIN), (29, 33));
        // Growing Viewer (delta -5) gives 5 points back to Viewer.
        assert_eq!(clamp_ev_divider(24, 38, -5, MIN), (19, 43));
        // Explorer + Viewer is always conserved.
        let (e, v) = clamp_ev_divider(24, 38, 5, MIN);
        assert_eq!(e + v, 62);
    }

    #[test]
    fn ev_divider_clamps_at_min_floor() {
        // Explorer can't drop below MIN even with a big shrink.
        assert_eq!(clamp_ev_divider(12, 50, -5, MIN), (10, 52));
        // Viewer can't drop below MIN even when Explorer wants to grow.
        assert_eq!(clamp_ev_divider(50, 12, 5, MIN), (52, 10));
    }

    #[test]
    fn vt_divider_protects_the_terminal_column() {
        // Explorer 24, Viewer 38 → Terminal 38. Growing Viewer right eats into
        // Terminal but never past its MIN floor: max Viewer = 100 - 24 - 10 = 66.
        assert_eq!(clamp_vt_divider(24, 38, 5, MIN), 43);
        assert_eq!(clamp_vt_divider(24, 64, 5, MIN), 66); // clamped, Terminal=10
        // Shrinking Viewer (grow Terminal) is floored at Viewer = MIN.
        assert_eq!(clamp_vt_divider(24, 12, -5, MIN), 10);
    }

    #[test]
    fn dividers_never_let_a_column_vanish() {
        // Sweep deltas across the full range; all three columns stay >= MIN.
        for delta in [-50i16, -20, -5, 5, 20, 50] {
            let (e, v) = clamp_ev_divider(24, 38, delta, MIN);
            let t = 100u16.saturating_sub(e + v);
            assert!(e >= MIN && v >= MIN && t >= MIN, "ev delta={delta}: {e}/{v}/{t}");

            let v2 = clamp_vt_divider(24, 38, delta, MIN);
            let t2 = 100u16.saturating_sub(24 + v2);
            assert!(v2 >= MIN && t2 >= MIN, "vt delta={delta}: 24/{v2}/{t2}");
        }
    }

    #[test]
    fn focus_is_pty_only_for_pty_panels() {
        assert!(Focus::TerminalClaude.is_pty());
        assert!(Focus::TerminalShell.is_pty());
        assert!(Focus::Editor.is_pty());
        assert!(!Focus::Worktree.is_pty());
        assert!(!Focus::Explorer.is_pty());
        assert!(!Focus::Viewer.is_pty());
    }

    #[test]
    fn editor_focus_uses_editor_keymap_context() {
        assert_eq!(Focus::Editor.key_context(), crate::keymap::KeyContext::Editor);
    }

    #[test]
    fn editor_target_resolves_relative_against_worktree() {
        let root = std::path::Path::new("/repo/wt");
        assert_eq!(
            editor_target(Some("src/main.rs"), root),
            Some(PathBuf::from("/repo/wt/src/main.rs"))
        );
    }

    #[test]
    fn editor_target_is_none_when_no_file_open() {
        // The load-bearing branch: no current file → no editor launch.
        assert_eq!(editor_target(None, std::path::Path::new("/repo/wt")), None);
    }

    #[test]
    fn editor_target_is_none_for_empty_path() {
        assert_eq!(editor_target(Some(""), std::path::Path::new("/repo/wt")), None);
    }

    #[test]
    fn resolve_editor_falls_back_when_unset() {
        assert_eq!(resolve_editor_command(None, None, "vi"), vec!["vi"]);
    }

    #[test]
    fn resolve_editor_visual_takes_precedence() {
        assert_eq!(
            resolve_editor_command(Some("vim"), Some("nano"), "vi"),
            vec!["vim"]
        );
    }

    #[test]
    fn resolve_editor_uses_editor_when_visual_unset() {
        assert_eq!(resolve_editor_command(None, Some("nano"), "vi"), vec!["nano"]);
    }

    #[test]
    fn resolve_editor_splits_args() {
        assert_eq!(
            resolve_editor_command(Some("code -w"), None, "vi"),
            vec!["code", "-w"]
        );
        assert_eq!(
            resolve_editor_command(Some("code\t-w  -n"), None, "vi"),
            vec!["code", "-w", "-n"]
        );
    }

    #[test]
    fn resolve_editor_ignores_blank_values() {
        // A blank/whitespace-only VISUAL is skipped so EDITOR (or the fallback)
        // still wins, rather than producing an empty command.
        assert_eq!(resolve_editor_command(Some(""), None, "vi"), vec!["vi"]);
        assert_eq!(resolve_editor_command(Some("   "), None, "vi"), vec!["vi"]);
        assert_eq!(
            resolve_editor_command(Some(""), Some("nano"), "vi"),
            vec!["nano"]
        );
        assert_eq!(resolve_editor_command(Some("  vim  "), None, "vi"), vec!["vim"]);
    }

    #[test]
    fn editor_content_size_subtracts_borders() {
        // Non-maximized: title row + bottom border (2 rows) and L/R borders (2 cols).
        assert_eq!(editor_content_size(80, 40, false), (38, 78));
        // Maximized: only the title row, no borders.
        assert_eq!(editor_content_size(80, 40, true), (39, 80));
    }

    #[test]
    fn editor_content_size_defaults_on_zero_region() {
        assert_eq!(editor_content_size(0, 40, false), (24, 80));
        assert_eq!(editor_content_size(80, 0, false), (24, 80));
    }

    #[test]
    fn editor_content_size_never_returns_zero() {
        // Tiny regions clamp to 1×1 rather than underflowing (vt100 needs ≥1).
        for w in 1..=3u16 {
            for h in 1..=3u16 {
                let (rows, c) = editor_content_size(w, h, false);
                assert!(rows >= 1 && c >= 1, "w={w} h={h} → ({rows},{c})");
            }
        }
    }

    #[test]
    fn resolve_editor_naive_split_does_not_honor_quotes() {
        // Documented limitation: no shell-style quoting. A quoted argument is
        // split on its inner spaces. This pins the intentional behavior.
        assert_eq!(
            resolve_editor_command(Some("vim -c 'set ft=rust'"), None, "vi"),
            vec!["vim", "-c", "'set", "ft=rust'"]
        );
    }

    fn wt(branch: &str) -> git_engine::WorktreeInfo {
        git_engine::WorktreeInfo {
            path: std::path::PathBuf::from(format!("/tmp/{branch}")),
            branch: branch.to_string(),
            is_main: branch == "main",
            added: 0,
            modified: 0,
            deleted: 0,
            is_clean: true,
            ahead: None,
            behind: None,
        }
    }

    #[test]
    fn reselect_pins_to_branch_when_order_shifts() {
        // Selection points at "feat-b" (index 2). A new worktree inserted
        // earlier shifts indices; the selection must follow "feat-b", not stay
        // at index 2 (which now holds a different branch).
        let after = [wt("main"), wt("feat-a"), wt("feat-aa"), wt("feat-b")];
        assert_eq!(reselect_worktree_index(&after, "feat-b", 2), Some(3));
    }

    #[test]
    fn reselect_falls_back_when_branch_removed() {
        // "feat-a" (index 1) was removed; only "main" remains. The stale index 1
        // is out of range and must clamp to the last valid index (main).
        let after = [wt("main")];
        assert_eq!(reselect_worktree_index(&after, "feat-a", 1), Some(0));
    }

    #[test]
    fn reselect_keeps_index_when_branch_unchanged() {
        let after = [wt("main"), wt("feat-a")];
        assert_eq!(reselect_worktree_index(&after, "feat-a", 1), Some(1));
    }

    #[test]
    fn reselect_returns_none_for_empty_list() {
        assert_eq!(reselect_worktree_index(&[], "main", 0), None);
    }

    #[test]
    fn reselect_clamps_when_prev_branch_empty() {
        // No previously selected branch (e.g. first load): just keep the index
        // in range.
        let after = [wt("main"), wt("feat-a")];
        assert_eq!(reselect_worktree_index(&after, "", 5), Some(1));
    }

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
