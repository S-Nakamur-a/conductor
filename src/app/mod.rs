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

use crate::background::BackgroundOp;

use syntect::parsing::SyntaxSet;

use crate::config;
use crate::diff_state::DiffState;
use crate::git_engine;
use crate::jump_history::JumpHistory;
use crate::keymap::KeyMap;
use crate::overlay::{ActiveOverlay, OverlayManager};
use crate::overlay::{HoverInfoOverlay, ReferencesOverlay, SymbolActionOverlay, SymbolHintOverlay};
use crate::pty_manager;
use crate::review_state::ReviewState;
use crate::review_store::{self, ReviewStore};
use crate::symbol_index::SymbolIndex;
use crate::terminal_state::TerminalState;
use crate::theme::Theme;
use crate::ui::common::list_row::HoverRow;
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
pub use update::{UpdateProgress, UpdateState};

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
    /// In-flight walkthrough generations, at most one per branch so worktrees
    /// don't block each other: each is a background thread's result channel
    /// plus enough context to report against the right branch. Drained by
    /// [`App::poll_walkthrough_generation`].
    pub walkthrough_gens: WalkthroughGenerations,
    /// The selected worktree's walkthrough (header + steps), reloaded by
    /// [`App::refresh_reviews`] alongside the comment list.
    pub current_walkthrough: Option<(
        crate::walkthrough::Walkthrough,
        Vec<crate::walkthrough::WalkthroughStep>,
    )>,
    /// Pending y/n confirmation for `Action::PublishReview`: `Some` while the
    /// confirm overlay is showing (holding the already-filtered comments and
    /// skip count to display), cleared on either answer.
    pub publish_confirm: Option<crate::review_publish::PublishConfirm>,
    /// In-flight GitHub-publish operation, polled by
    /// [`App::poll_publish_review`].
    pub publish_op: BackgroundOp<crate::review_publish::PublishOutcome>,
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
    /// Whether the high-contrast transform is applied to `theme`. Mirrors
    /// `config.ui.high_contrast`; kept as a field so `set_theme` and the live
    /// reload rebuild the theme with the right polarity.
    pub high_contrast: bool,
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

    /// The panel divider currently being dragged with the mouse, if any. Set on
    /// mouse-down over a boundary, moved on each drag event, and cleared (with a
    /// single config persist) on mouse-up. While `Some`, drag events resize
    /// instead of doing their normal per-panel work.
    pub divider_drag: Option<Divider>,
    /// The panel divider the mouse is hovering, if any. Drives the resize
    /// affordance — the hovered boundary is highlighted, standing in for a
    /// `col-resize`/`row-resize` cursor (a terminal can't switch the OS cursor
    /// shape). A live drag takes precedence over hover when rendering.
    pub divider_hover: Option<Divider>,

    /// Which Explorer file-tree row (by visible-list index) the mouse is
    /// hovering, plus the fade-out state of the row it just left. Shared
    /// tracking type so the tree, Changed files, and worktree panels don't
    /// each reimplement the same hover/selection priority rules — see
    /// `src/ui/common/list_row.rs`.
    pub explorer_tree_hover: HoverRow,
    /// Same as [`explorer_tree_hover`](Self::explorer_tree_hover) but for the
    /// Changed files (diff) list in the Explorer's bottom half.
    pub diff_list_hover: HoverRow,

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
    /// Set when the user manually triggered an update check (via the command
    /// palette), so the next poll result flashes explicit feedback — including
    /// the "already up to date" / "check failed" cases the silent startup check
    /// swallows.
    pub update_check_requested: bool,

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
    /// Action the mouse is currently over in the worktree bar (from the last
    /// `Moved` event, resolved against `wtbar_hits`). Drives hover background
    /// on chips and the `[x]` delete button.
    pub wtbar_hover: Option<crate::ui::worktree_bar::WtbarAction>,

    /// Menu bar interaction state: which menu is focused or open, and the
    /// click regions recorded by the last bar/dropdown render.
    pub menu: crate::menu::MenuState,

    // ── Code navigation (symbol index + jump history) ───────────
    pub symbol_index: SymbolIndex,
    pub jump_history: JumpHistory,
    pub references_overlay: ReferencesOverlay,
    pub symbol_hint_overlay: SymbolHintOverlay,
    pub symbol_action_overlay: SymbolActionOverlay,
    pub hover_info_overlay: HoverInfoOverlay,

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

    pub fn is_any_overlay_active(&self) -> bool {
        self.overlays.active != ActiveOverlay::None
            || self.worktree_mgr.input_mode != WorktreeInputMode::Normal
            || self.review_state.input_mode != crate::review_state::ReviewInputMode::Normal
            || self.review_state.template_picker_active
            || self.review_state.comment_detail_active
            || self.update_state != UpdateState::Idle
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
}
