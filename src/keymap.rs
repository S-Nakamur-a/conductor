//! Configurable keybindings — maps key chords to semantic actions.
//!
//! Provides a `KeyMap` that resolves `KeyEvent` → `Action` for a given
//! `KeyContext`, with user overrides from `config.toml`.
//!
//! The engine is [`keymap-suite`](keymap_suite), the one-import facade over
//! `keymap-core`/`keymap-config`/`keymap-seq`. We follow its design directly:
//!
//! * **Loaded once, owned whole.** [`KeyMap`] holds one [`Loaded<Action>`] — the
//!   facade's TOML-build result — whose `layers` map is keyed by name. Each
//!   `KeyContext` names one layer; `Global` is the bare `[keys]` table
//!   ([`keymap_suite::GLOBAL_LAYER`]).
//! * **The caller assembles the active chain.** Per key event we hand
//!   `resolve_layered([context_layer, global], …)` to the library — the context
//!   layer wins, misses fall through to global, and a total miss returns `None`
//!   ("pass through to the PTY"). The library never tracks our focus/mode; that
//!   stack is ours, exactly as the suite intends.
//! * **Defaults ⊕ user via [`merge`](keymap_suite::merge).** Defaults are
//!   authored in `default_keybinds.toml` (embedded at compile time); user
//!   bindings from `[keybinds]` are an *overlay* merged on top. A user chord
//!   overrides the default for that exact chord; `"<chord>" = false` is a
//!   tombstone that removes a default. We surface only genuine problems as
//!   [`KeybindWarning`]s — override/unbind notes are informational, not warnings.
//! * **Help is the reverse of resolution.** [`KeyMap::keys_for_action`] uses the
//!   facade's [`keys_for_action`](keymap_suite::keys_for_action) so the rendered
//!   shortcuts can never drift from what actually resolves.

use crossterm::event::KeyEvent;
use keymap_suite::{KeyInput, Keymap, Loaded, resolve_layered};

// ---------------------------------------------------------------------------
// Action — every customisable user action
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    // ── Global ────────────────────────────────────────────────────
    Quit,
    ShowHelp,
    CommandPalette,
    CycleFocusForward,
    CycleFocusBackward,
    /// Switch the selected worktree to the next/previous one, from any panel
    /// (the worktree strip follows the selection). Distinct from focus cycling.
    NextWorktree,
    PrevWorktree,
    FocusWorktree,
    FocusExplorer,
    FocusExplorerDiffList,
    FocusViewer,
    FocusTerminalClaude,
    FocusTerminalShell,
    NewClaudeCode,
    NewShell,
    OpenRepo,
    SwitchRepo,

    // ── Shared navigation ────────────────────────────────────────
    NavigateUp,
    NavigateDown,
    GoToTop,
    GoToBottom,
    ExpandOrRight,
    CollapseOrLeft,
    Select,

    // ── Worktree panel ───────────────────────────────────────────
    CreateWorktree,
    DeleteWorktree,
    SwitchBranch,
    GrabBranch,
    UngrabBranch,
    PruneWorktrees,
    MergeToMain,
    RefreshWorktrees,
    ResetMainToOrigin,
    CherryPick,
    PullWorktree,
    SessionHistory,
    OpenPullRequest,

    // ── Explorer panel ───────────────────────────────────────────
    ShowDiffList,
    ShowCommentList,
    /// Open the full-screen comment-list modal (overview of all comments on the
    /// branch, with jump-to-location).
    OpenCommentList,
    SearchFilename,
    DeleteComment,
    ToggleResolve,
    EditComment,
    ReplyToComment,
    ViewCommentDetail,
    ExitSubPanel,

    // ── Viewer panel ─────────────────────────────────────────────
    ScrollHalfPageDown,
    ScrollHalfPageUp,
    ScrollLeft,
    ScrollRight,
    ScrollHome,
    SearchInFile,
    NextSearchMatch,
    PrevSearchMatch,
    AddComment,
    ExitToExplorer,
    /// Open the file shown in the Viewer in an external editor ($VISUAL /
    /// $EDITOR): suspend the TUI, run the editor, then restore and reload.
    OpenInEditor,

    // ── Terminal panel ────────────────────────────────────────────
    LeaveTerminal,
    ScrollbackUp,
    ScrollbackDown,
    ScrollbackTop,
    SnapToLive,
    OpenFileFromTerminal,

    // ── App ──────────────────────────────────────────────────────
    UpdateAndRestart,

    // ── Search ──────────────────────────────────────────────────
    SearchFullText,

    // ── Code navigation ─────────────────────────────────────────
    JumpBack,
    JumpForward,
    ToggleInlineThread,
    InlineReply,

    // ── Diff navigation ─────────────────────────────────────────
    NextHunk,
    PrevHunk,
    NextComment,
    PrevComment,
    /// Jump to the next/previous changed file in the diff list (GitHub-style
    /// "next file" — the lightweight substitute for cross-file scrolling).
    NextChangedFile,
    PrevChangedFile,

    // ── Diff context expansion ─────────────────────────────────
    ExpandContext,
    ExpandAllContext,

    // ── Panel layout ────────────────────────────────────────────
    TogglePanelExpand,
    TogglePanelOverlay,
    /// Grow the focused panel toward the left (tmux `resize-pane -L`).
    ResizePaneLeft,
    /// Grow the focused panel toward the right (tmux `resize-pane -R`).
    ResizePaneRight,
    /// Grow the focused panel upward (tmux `resize-pane -U`).
    ResizePaneUp,
    /// Grow the focused panel downward (tmux `resize-pane -D`).
    ResizePaneDown,

    // ── UI ──────────────────────────────────────────────────────
    /// Open the theme picker overlay to switch the UI color theme at runtime.
    OpenThemePicker,
}

impl Action {
    /// Convert from config string to Action.
    pub fn from_str(s: &str) -> Option<Action> {
        match s {
            "quit" => Some(Action::Quit),
            "show_help" => Some(Action::ShowHelp),
            "command_palette" => Some(Action::CommandPalette),
            "cycle_focus_forward" => Some(Action::CycleFocusForward),
            "cycle_focus_backward" => Some(Action::CycleFocusBackward),
            "next_worktree" => Some(Action::NextWorktree),
            "prev_worktree" => Some(Action::PrevWorktree),
            "focus_worktree" => Some(Action::FocusWorktree),
            "focus_explorer" => Some(Action::FocusExplorer),
            "focus_explorer_diff_list" => Some(Action::FocusExplorerDiffList),
            "focus_viewer" => Some(Action::FocusViewer),
            "focus_terminal_claude" => Some(Action::FocusTerminalClaude),
            "focus_terminal_shell" => Some(Action::FocusTerminalShell),
            "new_claude_code" => Some(Action::NewClaudeCode),
            "new_shell" => Some(Action::NewShell),
            "open_repo" => Some(Action::OpenRepo),
            "switch_repo" => Some(Action::SwitchRepo),
            "navigate_up" => Some(Action::NavigateUp),
            "navigate_down" => Some(Action::NavigateDown),
            "go_to_top" => Some(Action::GoToTop),
            "go_to_bottom" => Some(Action::GoToBottom),
            "expand_or_right" => Some(Action::ExpandOrRight),
            "collapse_or_left" => Some(Action::CollapseOrLeft),
            "select" => Some(Action::Select),
            "create_worktree" => Some(Action::CreateWorktree),
            "delete_worktree" => Some(Action::DeleteWorktree),
            "switch_branch" => Some(Action::SwitchBranch),
            "grab_branch" => Some(Action::GrabBranch),
            "ungrab_branch" => Some(Action::UngrabBranch),
            "prune_worktrees" => Some(Action::PruneWorktrees),
            "merge_to_main" => Some(Action::MergeToMain),
            "refresh_worktrees" => Some(Action::RefreshWorktrees),
            "reset_main_to_origin" => Some(Action::ResetMainToOrigin),
            "cherry_pick" => Some(Action::CherryPick),
            "pull_worktree" => Some(Action::PullWorktree),
            "session_history" => Some(Action::SessionHistory),
            "open_pull_request" => Some(Action::OpenPullRequest),
            "show_diff_list" => Some(Action::ShowDiffList),
            "show_comment_list" => Some(Action::ShowCommentList),
            "open_comment_list" => Some(Action::OpenCommentList),
            "search_filename" => Some(Action::SearchFilename),
            "delete_comment" => Some(Action::DeleteComment),
            "toggle_resolve" => Some(Action::ToggleResolve),
            "edit_comment" => Some(Action::EditComment),
            "reply_to_comment" => Some(Action::ReplyToComment),
            "view_comment_detail" => Some(Action::ViewCommentDetail),
            "exit_sub_panel" => Some(Action::ExitSubPanel),
            "scroll_half_page_down" => Some(Action::ScrollHalfPageDown),
            "scroll_half_page_up" => Some(Action::ScrollHalfPageUp),
            "scroll_left" => Some(Action::ScrollLeft),
            "scroll_right" => Some(Action::ScrollRight),
            "scroll_home" => Some(Action::ScrollHome),
            "search_in_file" => Some(Action::SearchInFile),
            "next_search_match" => Some(Action::NextSearchMatch),
            "prev_search_match" => Some(Action::PrevSearchMatch),
            "add_comment" => Some(Action::AddComment),
            "exit_to_explorer" => Some(Action::ExitToExplorer),
            "open_in_editor" => Some(Action::OpenInEditor),
            "leave_terminal" => Some(Action::LeaveTerminal),
            "scrollback_up" => Some(Action::ScrollbackUp),
            "scrollback_down" => Some(Action::ScrollbackDown),
            "scrollback_top" => Some(Action::ScrollbackTop),
            "snap_to_live" => Some(Action::SnapToLive),
            "open_file_from_terminal" => Some(Action::OpenFileFromTerminal),
            "update_and_restart" => Some(Action::UpdateAndRestart),
            "search_full_text" => Some(Action::SearchFullText),
            "jump_back" => Some(Action::JumpBack),
            "jump_forward" => Some(Action::JumpForward),
            "next_hunk" => Some(Action::NextHunk),
            "prev_hunk" => Some(Action::PrevHunk),
            "next_comment" => Some(Action::NextComment),
            "next_changed_file" => Some(Action::NextChangedFile),
            "prev_changed_file" => Some(Action::PrevChangedFile),
            "prev_comment" => Some(Action::PrevComment),
            "expand_context" => Some(Action::ExpandContext),
            "expand_all_context" => Some(Action::ExpandAllContext),
            "toggle_inline_thread" => Some(Action::ToggleInlineThread),
            "inline_reply" => Some(Action::InlineReply),
            "toggle_panel_expand" => Some(Action::TogglePanelExpand),
            "toggle_panel_overlay" => Some(Action::TogglePanelOverlay),
            "resize_pane_left" => Some(Action::ResizePaneLeft),
            "resize_pane_right" => Some(Action::ResizePaneRight),
            "resize_pane_up" => Some(Action::ResizePaneUp),
            "resize_pane_down" => Some(Action::ResizePaneDown),
            "open_theme_picker" => Some(Action::OpenThemePicker),
            _ => None,
        }
    }

    /// Convert Action to config string.
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::ShowHelp => "show_help",
            Action::CommandPalette => "command_palette",
            Action::CycleFocusForward => "cycle_focus_forward",
            Action::CycleFocusBackward => "cycle_focus_backward",
            Action::NextWorktree => "next_worktree",
            Action::PrevWorktree => "prev_worktree",
            Action::FocusWorktree => "focus_worktree",
            Action::FocusExplorer => "focus_explorer",
            Action::FocusExplorerDiffList => "focus_explorer_diff_list",
            Action::FocusViewer => "focus_viewer",
            Action::FocusTerminalClaude => "focus_terminal_claude",
            Action::FocusTerminalShell => "focus_terminal_shell",
            Action::NewClaudeCode => "new_claude_code",
            Action::NewShell => "new_shell",
            Action::OpenRepo => "open_repo",
            Action::SwitchRepo => "switch_repo",
            Action::NavigateUp => "navigate_up",
            Action::NavigateDown => "navigate_down",
            Action::GoToTop => "go_to_top",
            Action::GoToBottom => "go_to_bottom",
            Action::ExpandOrRight => "expand_or_right",
            Action::CollapseOrLeft => "collapse_or_left",
            Action::Select => "select",
            Action::CreateWorktree => "create_worktree",
            Action::DeleteWorktree => "delete_worktree",
            Action::SwitchBranch => "switch_branch",
            Action::GrabBranch => "grab_branch",
            Action::UngrabBranch => "ungrab_branch",
            Action::PruneWorktrees => "prune_worktrees",
            Action::MergeToMain => "merge_to_main",
            Action::RefreshWorktrees => "refresh_worktrees",
            Action::ResetMainToOrigin => "reset_main_to_origin",
            Action::CherryPick => "cherry_pick",
            Action::PullWorktree => "pull_worktree",
            Action::SessionHistory => "session_history",
            Action::OpenPullRequest => "open_pull_request",
            Action::ShowDiffList => "show_diff_list",
            Action::ShowCommentList => "show_comment_list",
            Action::OpenCommentList => "open_comment_list",
            Action::SearchFilename => "search_filename",
            Action::DeleteComment => "delete_comment",
            Action::ToggleResolve => "toggle_resolve",
            Action::EditComment => "edit_comment",
            Action::ReplyToComment => "reply_to_comment",
            Action::ViewCommentDetail => "view_comment_detail",
            Action::ExitSubPanel => "exit_sub_panel",
            Action::ScrollHalfPageDown => "scroll_half_page_down",
            Action::ScrollHalfPageUp => "scroll_half_page_up",
            Action::ScrollLeft => "scroll_left",
            Action::ScrollRight => "scroll_right",
            Action::ScrollHome => "scroll_home",
            Action::SearchInFile => "search_in_file",
            Action::NextSearchMatch => "next_search_match",
            Action::PrevSearchMatch => "prev_search_match",
            Action::AddComment => "add_comment",
            Action::ExitToExplorer => "exit_to_explorer",
            Action::OpenInEditor => "open_in_editor",
            Action::LeaveTerminal => "leave_terminal",
            Action::ScrollbackUp => "scrollback_up",
            Action::ScrollbackDown => "scrollback_down",
            Action::ScrollbackTop => "scrollback_top",
            Action::SnapToLive => "snap_to_live",
            Action::OpenFileFromTerminal => "open_file_from_terminal",
            Action::UpdateAndRestart => "update_and_restart",
            Action::SearchFullText => "search_full_text",
            Action::JumpBack => "jump_back",
            Action::JumpForward => "jump_forward",
            Action::NextHunk => "next_hunk",
            Action::PrevHunk => "prev_hunk",
            Action::NextComment => "next_comment",
            Action::PrevComment => "prev_comment",
            Action::NextChangedFile => "next_changed_file",
            Action::PrevChangedFile => "prev_changed_file",
            Action::ExpandContext => "expand_context",
            Action::ExpandAllContext => "expand_all_context",
            Action::ToggleInlineThread => "toggle_inline_thread",
            Action::InlineReply => "inline_reply",
            Action::TogglePanelExpand => "toggle_panel_expand",
            Action::TogglePanelOverlay => "toggle_panel_overlay",
            Action::ResizePaneLeft => "resize_pane_left",
            Action::ResizePaneRight => "resize_pane_right",
            Action::ResizePaneUp => "resize_pane_up",
            Action::ResizePaneDown => "resize_pane_down",
            Action::OpenThemePicker => "open_theme_picker",
        }
    }

    /// Whether this action is intercepted while a terminal panel (PTY) is
    /// focused. `false` (the default) means the chord is forwarded to the inner
    /// program (shell / Claude Code), so Conductor never steals a key the
    /// program needs — `ctrl+r` reverse-search, `ctrl+q`/XON, etc. Only the
    /// focus/navigation/scrollback actions listed here are stolen back. This is
    /// the single source of truth for terminal interception: both [`KeyMap::resolve`]
    /// and [`KeyMap::keys_for_action`] honor it, so resolution == behavior ==
    /// the rendered help, with no hand-maintained allowlist in the dispatcher.
    fn fires_in_terminal(self) -> bool {
        matches!(
            self,
            // Terminal-only actions (meaningful only with a terminal focused).
            Action::LeaveTerminal
                | Action::ScrollbackUp
                | Action::ScrollbackDown
                | Action::ScrollbackTop
                | Action::SnapToLive
                | Action::OpenFileFromTerminal
                // Global focus/navigation that stays useful over a PTY.
                | Action::FocusWorktree
                | Action::FocusExplorer
                | Action::FocusExplorerDiffList
                | Action::FocusViewer
                | Action::FocusTerminalClaude
                | Action::FocusTerminalShell
                | Action::CommandPalette
                | Action::CycleFocusForward
                | Action::CycleFocusBackward
                | Action::NextWorktree
                | Action::PrevWorktree
                | Action::TogglePanelExpand
                | Action::TogglePanelOverlay
                // Pane resizing is most useful with a terminal focused (resize
                // the Claude/Shell split or the terminal column while typing in
                // it), so these must fire over a PTY too.
                | Action::ResizePaneLeft
                | Action::ResizePaneRight
                | Action::ResizePaneUp
                | Action::ResizePaneDown
        )
    }
}

// ---------------------------------------------------------------------------
// KeyContext — selects which layer to consult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyContext {
    Global,
    Worktree,
    Explorer,
    ExplorerDiffList,
    ExplorerCommentList,
    Viewer,
    ViewerDiffMode,
    Terminal,
    /// The embedded editor panel. Like `Terminal`, almost every chord is
    /// forwarded to the inner program (vim/emacs) — its own layer binds only the
    /// "leave focus" chord; the rest fall through to the global layer (filtered
    /// to terminal-firing actions) or to the PTY.
    Editor,
    /// Shared navigation context for overlay popups (list/tree navigation).
    /// Falls back to Global like other contexts.
    Overlay,
}

/// The non-global contexts, each backed by a named `[layers.<name>]` table.
const PANEL_CONTEXTS: [KeyContext; 9] = [
    KeyContext::Worktree,
    KeyContext::Explorer,
    KeyContext::ExplorerDiffList,
    KeyContext::ExplorerCommentList,
    KeyContext::Viewer,
    KeyContext::ViewerDiffMode,
    KeyContext::Terminal,
    KeyContext::Editor,
    KeyContext::Overlay,
];

impl KeyContext {
    /// The keymap-suite layer name backing this context. `Global` lives in the
    /// bare `[keys]` table, which the suite exposes as the `GLOBAL_LAYER`.
    fn layer_name(self) -> &'static str {
        match self {
            KeyContext::Global => keymap_suite::GLOBAL_LAYER,
            KeyContext::Worktree => "worktree",
            KeyContext::Explorer => "explorer",
            KeyContext::ExplorerDiffList => "explorer_diff_list",
            KeyContext::ExplorerCommentList => "explorer_comment_list",
            KeyContext::Viewer => "viewer",
            KeyContext::ViewerDiffMode => "viewer_diff_mode",
            KeyContext::Terminal => "terminal",
            KeyContext::Editor => "editor",
            KeyContext::Overlay => "overlay",
        }
    }
}

// ---------------------------------------------------------------------------
// KeybindWarning — survivable problems found while building the keymap
// ---------------------------------------------------------------------------

/// A non-fatal problem found while loading user keybindings. Conductor's own
/// type so the public surface does not depend on `keymap_suite::Warning`
/// (which is `#[non_exhaustive]` and carries sequence concepts Conductor does
/// not use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindWarning {
    /// An action name in the config was not recognized; the binding was skipped.
    UnknownAction { key: String, action: String },
    /// Two keys resolved to the same chord within one layer; the last one won.
    Conflict { chord: String },
    /// A `[keybinds.layers.<name>]` table used a layer name with no matching
    /// context; its bindings were ignored.
    UnknownLayer { layer: String },
    /// The `[keybinds]` config could not be parsed at all (malformed, or the
    /// pre-0.x `[keybinds.<context>]` action→key format). User overrides were
    /// ignored and the built-in defaults are used.
    InvalidConfig { detail: String },
}

impl std::fmt::Display for KeybindWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeybindWarning::UnknownAction { key, action } => {
                write!(f, "unknown keybind action {action:?} for key {key:?}")
            }
            KeybindWarning::Conflict { chord } => {
                write!(
                    f,
                    "keybind chord {chord:?} is bound more than once in one layer"
                )
            }
            KeybindWarning::UnknownLayer { layer } => {
                write!(f, "unknown keybind layer {layer:?}")
            }
            KeybindWarning::InvalidConfig { detail } => {
                write!(f, "could not parse [keybinds] config: {detail}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// KeyMap
// ---------------------------------------------------------------------------

/// Embedded default bindings (keymap-suite key→action TOML). See the file for
/// the schema; it is the reference for what users can write under `[keybinds]`.
const DEFAULT_KEYBINDS: &str = include_str!("default_keybinds.toml");

pub struct KeyMap {
    /// The merged keymap: defaults (`default_keybinds.toml`) with the user's
    /// `[keybinds]` overlaid via [`keymap_suite::merge`]. Its `layers` map is
    /// keyed by layer name; [`KeyContext::layer_name`] selects one per event and
    /// `global()` is consulted last. Holding the facade's own `Loaded` value
    /// (rather than re-bucketing it) is the suite's intended shape.
    loaded: Loaded<Action>,
}

impl KeyMap {
    /// Build a `KeyMap` from defaults plus the user's `[keybinds]` config table,
    /// discarding any warnings. See [`KeyMap::with_warnings`] to inspect them.
    #[allow(dead_code)] // convenience constructor; the app uses `with_warnings`.
    pub fn new(user: &toml::Table) -> Self {
        Self::with_warnings(user).0
    }

    /// Build a `KeyMap`, returning any non-fatal problems found in the user's
    /// config so the caller can surface them (the app flashes them on startup).
    pub fn with_warnings(user: &toml::Table) -> (Self, Vec<KeybindWarning>) {
        let mut warnings = Vec::new();

        // 1. Embedded defaults — the merge base. Authored in-repo, so any
        //    warning is a build bug: fail loudly in debug, never reach the user.
        let defaults = keymap_suite::from_toml_str(DEFAULT_KEYBINDS, Action::from_str)
            .expect("embedded default keybinds must be valid TOML");
        debug_assert!(
            defaults.warnings.is_empty(),
            "default keybinds produced warnings: {:?}",
            defaults.warnings
        );
        for w in &defaults.warnings {
            log::error!("default keybinds produced a warning (bug): {w:?}");
        }

        // 2. Parse the user's `[keybinds]` overlay and merge it onto the
        //    defaults. `merge` does the per-chord override and applies any
        //    `= false` tombstones; we keep only real problems as warnings (its
        //    override/unbind notes are informational, not warnings).
        let loaded = match parse_user_keybinds(user, &mut warnings) {
            Some(overlay) => {
                warn_unknown_layers(&overlay, &mut warnings);
                let merged = keymap_suite::merge(defaults, overlay);
                collect_warnings(&merged.output.warnings, &mut warnings);
                merged.output
            }
            None => defaults,
        };

        (KeyMap { loaded }, warnings)
    }

    /// The active layer chain for `context`: the context's own layer first (when
    /// it has one and is not `Global`), then the always-on global layer. This is
    /// the per-event stack the suite asks the caller to assemble.
    fn chain(&self, context: KeyContext) -> Vec<&Keymap<Action>> {
        let global = self.loaded.global();
        if context == KeyContext::Global {
            return vec![global];
        }
        match self.loaded.layers.get(context.layer_name()) {
            Some(layer) => vec![layer, global],
            None => vec![global],
        }
    }

    /// Resolve a key event to an action in the given context. The context layer
    /// is consulted first, then the global layer; an unmappable key event or a
    /// total miss yields `None` (the caller passes the key through).
    ///
    /// In the terminal context, an action that does not [fire in the
    /// terminal](Action::fires_in_terminal) resolves to `None` so the chord
    /// reaches the PTY — the global fallback stays, but globally-bound actions
    /// the terminal shouldn't steal (quit, switch-repo, …) are filtered here
    /// rather than by an allowlist in the dispatcher.
    pub fn resolve(&self, key: &KeyEvent, context: KeyContext) -> Option<Action> {
        let input = KeyInput::try_from(*key).ok()?;
        let action = resolve_layered(self.chain(context).iter().copied(), &input).copied()?;
        // The editor panel forwards keys to its PTY exactly like the terminal,
        // so it honors the same "only steal terminal-firing actions" filter —
        // everything else (Esc, Ctrl+G, …) reaches vim/emacs untouched.
        if matches!(context, KeyContext::Terminal | KeyContext::Editor)
            && !action.fires_in_terminal()
        {
            return None;
        }
        Some(action)
    }

    /// Display strings for every key bound to an action in a context (context
    /// layer plus the global layer), for the help screen. Strings are
    /// keymap-core canonical form (e.g. `"ctrl+d"`, `"down"`, `"G"`), which
    /// round-trips back through the config grammar.
    pub fn keys_for_action(&self, context: KeyContext, action: Action) -> Vec<String> {
        // Keep the rendered help honest with `resolve`: in the terminal and
        // editor contexts, a globally-bound action that doesn't fire there has
        // no working chord.
        if matches!(context, KeyContext::Terminal | KeyContext::Editor)
            && !action.fires_in_terminal()
        {
            return Vec::new();
        }

        // The reverse of resolution, over the same chain `resolve` consults, so
        // the rendered help can never advertise a chord that would not fire.
        let mut keys: Vec<String> = self
            .chain(context)
            .iter()
            .flat_map(|layer| keymap_suite::keys_for_action(layer, &action))
            .map(|input| input.to_string())
            .collect();

        keys.sort();
        keys.dedup();
        keys
    }

    /// Keys bound to `action` in `context`'s OWN layer only — unlike
    /// [`keys_for_action`](Self::keys_for_action), this does NOT fold in the
    /// global layer. Lets a caller tell "bound in this panel" from "bound
    /// globally and merely reachable here" (used to scope the command palette).
    pub fn keys_in_layer(&self, context: KeyContext, action: Action) -> Vec<String> {
        let layer = if context == KeyContext::Global {
            self.loaded.global()
        } else {
            match self.loaded.layers.get(context.layer_name()) {
                Some(layer) => layer,
                None => return Vec::new(),
            }
        };
        let mut keys: Vec<String> = keymap_suite::keys_for_action(layer, &action)
            .into_iter()
            .map(|input| input.to_string())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }
}

/// Warn about any user `[keybinds.layers.<name>]` whose name matches no
/// [`KeyContext`] — its bindings are merged but never consulted. The empty
/// `GLOBAL_LAYER` the loader always injects is skipped, so only a genuinely
/// unrecognized, non-empty named layer warns.
fn warn_unknown_layers(overlay: &Loaded<Action>, warnings: &mut Vec<KeybindWarning>) {
    for (name, layer) in &overlay.layers {
        if name == keymap_suite::GLOBAL_LAYER || layer.is_empty() {
            continue;
        }
        if PANEL_CONTEXTS.iter().all(|c| c.layer_name() != name) {
            warnings.push(KeybindWarning::UnknownLayer {
                layer: name.clone(),
            });
        }
    }
}

/// Parse the user's `[keybinds]` table into a keymap-suite overlay. Returns
/// `None` (no overrides) when the table is empty or cannot be parsed; a parse
/// failure is recorded as a [`KeybindWarning::InvalidConfig`] so the app can
/// tell the user their customizations were ignored.
fn parse_user_keybinds(
    user: &toml::Table,
    warnings: &mut Vec<KeybindWarning>,
) -> Option<Loaded<Action>> {
    if user.is_empty() {
        return None;
    }

    // keymap-suite parses a standalone document; re-emit just the [keybinds]
    // subtree as TOML text. (Conductor's `toml` and the suite's may differ in
    // version, so the interface between them is text, not types.)
    let toml_text = match toml::to_string(user) {
        Ok(text) => text,
        Err(e) => {
            warnings.push(KeybindWarning::InvalidConfig {
                detail: e.to_string(),
            });
            return None;
        }
    };

    match keymap_suite::from_toml_str(&toml_text, Action::from_str) {
        Ok(build) => Some(build),
        Err(e) => {
            warnings.push(KeybindWarning::InvalidConfig {
                detail: format!(
                    "{e} (note: the keybind format is now key→action under \
                     [keybinds.keys] / [keybinds.layers.*]; the old \
                     [keybinds.<context>] action→key tables are no longer read)"
                ),
            });
            None
        }
    }
}

/// Translate the keymap-suite warnings Conductor cares about into its own
/// warning type, dropping sequence-related variants it does not use.
fn collect_warnings(from: &[keymap_suite::Warning], into: &mut Vec<KeybindWarning>) {
    for w in from {
        match w {
            keymap_suite::Warning::UnknownAction { key, action } => {
                into.push(KeybindWarning::UnknownAction {
                    key: key.clone(),
                    action: action.clone(),
                });
            }
            keymap_suite::Warning::Conflict { chord, .. } => {
                into.push(KeybindWarning::Conflict {
                    chord: chord.clone(),
                });
            }
            // PrefixShadow / EmptySequence / SequenceShadow concern sequences,
            // which Conductor does not use. `Warning` is #[non_exhaustive].
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn default_keymap() -> KeyMap {
        KeyMap::new(&toml::Table::new())
    }

    #[test]
    fn defaults_build_without_warnings() {
        let (_km, warnings) = KeyMap::with_warnings(&toml::Table::new());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn every_default_action_name_resolves() {
        // Guards against a typo in default_keybinds.toml: an unknown action name
        // would surface as a warning when the defaults are parsed.
        let build = keymap_suite::from_toml_str(DEFAULT_KEYBINDS, Action::from_str).unwrap();
        assert!(build.warnings.is_empty(), "{:?}", build.warnings);
    }

    #[test]
    fn critical_defaults_resolve() {
        let km = default_keymap();

        // Quit moved to ctrl+q; bare q is unbound (passes through) so it can no
        // longer kill the app by accident.
        let key_ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(&key_ctrl_q, KeyContext::Global), Some(Action::Quit));
        let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());
        assert_eq!(km.resolve(&key_q, KeyContext::Global), None);

        let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key_j, KeyContext::Worktree),
            Some(Action::NavigateDown)
        );

        let key_ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&key_ctrl_n, KeyContext::Global),
            Some(Action::NewClaudeCode)
        );

        // Ctrl+Esc leaves the terminal.
        let key_ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&key_ctrl_esc, KeyContext::Terminal),
            Some(Action::LeaveTerminal)
        );
    }

    #[test]
    fn worktree_switch_and_zoom_aliases_resolve() {
        // alt+]/alt+[ are the kitty-protocol-free aliases for ctrl+tab worktree
        // switching; ctrl+alt+z zooms the focused panel (tmux `prefix z`), joining
        // the ctrl+alt pane-sizing family.
        let km = default_keymap();
        let cases = [
            (KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT), Action::NextWorktree),
            (KeyEvent::new(KeyCode::Char('['), KeyModifiers::ALT), Action::PrevWorktree),
            (
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::ALT),
                Action::TogglePanelExpand,
            ),
        ];
        for (key, action) in cases {
            assert_eq!(km.resolve(&key, KeyContext::Global), Some(action), "{key:?}");
        }
    }

    #[test]
    fn terminal_intercepts_only_firing_actions() {
        let km = default_keymap();

        // Quit (ctrl+q) is global but does NOT fire in the terminal — the chord
        // reaches the PTY instead of killing the app (so the inner program keeps
        // ctrl+q / XON). Same for switch_repo (ctrl+r → shell reverse-search).
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(&ctrl_q, KeyContext::Global), Some(Action::Quit));
        assert_eq!(km.resolve(&ctrl_q, KeyContext::Terminal), None);
        let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(&ctrl_r, KeyContext::Global), Some(Action::SwitchRepo));
        assert_eq!(km.resolve(&ctrl_r, KeyContext::Terminal), None);

        // Focus/navigation chords ARE stolen back from the PTY.
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&ctrl_p, KeyContext::Terminal),
            Some(Action::CommandPalette)
        );
        let alt_next = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
        assert_eq!(
            km.resolve(&alt_next, KeyContext::Terminal),
            Some(Action::NextWorktree)
        );
        let ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&ctrl_esc, KeyContext::Terminal),
            Some(Action::LeaveTerminal)
        );

        // Rendered help stays honest with resolution: no chord is advertised for
        // Quit in the terminal, but terminal-firing actions keep theirs.
        assert!(km.keys_for_action(KeyContext::Terminal, Action::Quit).is_empty());
        assert!(
            !km.keys_for_action(KeyContext::Terminal, Action::LeaveTerminal)
                .is_empty()
        );
    }

    #[test]
    fn terminal_usable_actions_all_resolve_in_terminal() {
        // Every action classified as firing in the terminal must actually have a
        // chord that resolves there — guards against adding a variant to
        // `fires_in_terminal` but forgetting to bind it (or vice versa).
        let km = default_keymap();
        let usable = [
            Action::LeaveTerminal,
            Action::ScrollbackUp,
            Action::ScrollbackDown,
            Action::ScrollbackTop,
            Action::SnapToLive,
            Action::OpenFileFromTerminal,
            Action::FocusWorktree,
            Action::FocusExplorer,
            Action::FocusExplorerDiffList,
            Action::FocusViewer,
            Action::FocusTerminalClaude,
            Action::FocusTerminalShell,
            Action::CommandPalette,
            Action::CycleFocusForward,
            Action::CycleFocusBackward,
            Action::NextWorktree,
            Action::PrevWorktree,
            Action::TogglePanelExpand,
            Action::TogglePanelOverlay,
        ];
        for a in usable {
            assert!(
                !km.keys_for_action(KeyContext::Terminal, a).is_empty(),
                "{a:?} should have a working chord in the terminal"
            );
        }
    }

    #[test]
    fn editor_context_steals_only_leave_and_globals() {
        // The embedded editor forwards almost everything to vim/emacs. It steals
        // back only Ctrl+Esc (leave) and the terminal-firing global chords; keys
        // the editor needs — Esc, Ctrl+G, Shift+PageUp — pass through (None).
        let km = default_keymap();

        let ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&ctrl_esc, KeyContext::Editor),
            Some(Action::LeaveTerminal)
        );

        // Bare Esc → vim mode changes; must not be stolen.
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        assert_eq!(km.resolve(&esc, KeyContext::Editor), None);

        // Ctrl+G is open_file_from_terminal in the *terminal* layer and
        // search_full_text globally — neither fires in the editor, so it reaches
        // the inner program instead of being intercepted.
        let ctrl_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(&ctrl_g, KeyContext::Editor), None);

        // Scrollback lives only in the terminal layer, so it does not leak into
        // the editor.
        let shift_pgup = KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT);
        assert_eq!(km.resolve(&shift_pgup, KeyContext::Editor), None);

        // Global focus/zoom chords still work over the editor.
        let alt_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT);
        assert_eq!(
            km.resolve(&alt_l, KeyContext::Editor),
            Some(Action::CycleFocusForward)
        );
        let ctrl_alt_z =
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert_eq!(
            km.resolve(&ctrl_alt_z, KeyContext::Editor),
            Some(Action::TogglePanelExpand)
        );
    }

    #[test]
    fn ctrl_esc_is_additive_in_viewer() {
        // The app-wide "leave focus" chord is bound in non-PTY panels too, but
        // additively: bare Esc keeps working alongside it.
        let km = default_keymap();
        let ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        assert_eq!(
            km.resolve(&ctrl_esc, KeyContext::Viewer),
            Some(Action::ExitToExplorer)
        );
        assert_eq!(
            km.resolve(&esc, KeyContext::Viewer),
            Some(Action::ExitToExplorer)
        );
    }

    #[test]
    fn context_falls_back_to_global() {
        let km = default_keymap();

        // Tab is bound per non-terminal context — resolves in Worktree but NOT
        // in Terminal (terminal layer has no Tab, neither does global).
        let key_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key_tab, KeyContext::Worktree),
            Some(Action::CycleFocusForward)
        );
        assert_eq!(km.resolve(&key_tab, KeyContext::Terminal), None);

        // Alt+l resolves globally, including from the Terminal context.
        let key_alt_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT);
        assert_eq!(
            km.resolve(&key_alt_l, KeyContext::Terminal),
            Some(Action::CycleFocusForward)
        );
    }

    #[test]
    fn context_shadows_are_per_context() {
        let km = default_keymap();

        // 'c' = CherryPick in Worktree, ShowCommentList in Explorer.
        let key_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key_c, KeyContext::Worktree),
            Some(Action::CherryPick)
        );
        assert_eq!(
            km.resolve(&key_c, KeyContext::Explorer),
            Some(Action::ShowCommentList)
        );
    }

    #[test]
    fn worktree_git_action_keys_resolve() {
        // The intentional 0.67 remap of the worktree panel's git actions to
        // more mnemonic chords. Pins the new bindings against silent regression.
        let km = default_keymap();
        let cases = [
            (KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty()), Action::PullWorktree),
            (KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()), Action::CherryPick),
            (KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()), Action::OpenPullRequest),
            // 'X' arrives as the resolved glyph 'X' + redundant SHIFT, which
            // keymap-core folds to match the "X" binding (cf. shift_g test).
            (KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT), Action::PruneWorktrees),
            // g/G are now go_to_top/bottom here too (was grab/ungrab), matching
            // every other panel; grab/ungrab moved to b/B ("branch", do/undo).
            (KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty()), Action::GoToTop),
            (KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT), Action::GoToBottom),
            (KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()), Action::GrabBranch),
            (KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT), Action::UngrabBranch),
        ];
        for (key, action) in cases {
            assert_eq!(km.resolve(&key, KeyContext::Worktree), Some(action), "{key:?}");
        }

        // The keys vacated by the remap are now unbound in the worktree panel
        // (no global fallback for bare u/v/P) — a deliberate no-op, not a
        // surprise reassignment.
        for key in [
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::empty()),
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
        ] {
            assert_eq!(km.resolve(&key, KeyContext::Worktree), None, "{key:?}");
        }
    }

    #[test]
    fn shift_g_resolves_uppercase_binding() {
        let km = default_keymap();
        // A normal terminal delivers Shift+g as the resolved glyph 'G' + SHIFT;
        // keymap-core folds the redundant SHIFT, matching the "G" binding.
        let key = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(&key, KeyContext::Worktree),
            Some(Action::GoToBottom)
        );
    }

    #[test]
    fn shift_tab_is_cycle_backward() {
        let km = default_keymap();
        // BackTab and Tab+SHIFT both normalize to Tab+SHIFT in keymap-core.
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty());
        assert_eq!(
            km.resolve(&backtab, KeyContext::Worktree),
            Some(Action::CycleFocusBackward)
        );
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(&shift_tab, KeyContext::Worktree),
            Some(Action::CycleFocusBackward)
        );
    }

    #[test]
    fn ctrl_tab_switches_worktree() {
        let km = default_keymap();
        // Global layer, so it resolves in every non-terminal context. Ctrl+Tab
        // jumps worktrees while plain Tab still cycles panel focus.
        let ctrl_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&ctrl_tab, KeyContext::Explorer),
            Some(Action::NextWorktree)
        );
        let plain_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(
            km.resolve(&plain_tab, KeyContext::Explorer),
            Some(Action::CycleFocusForward)
        );
        // Ctrl+Shift+Tab and Ctrl+BackTab both normalize to Ctrl+Shift+Tab.
        let ctrl_shift_tab =
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(&ctrl_shift_tab, KeyContext::Explorer),
            Some(Action::PrevWorktree)
        );
        let ctrl_backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&ctrl_backtab, KeyContext::Explorer),
            Some(Action::PrevWorktree)
        );
    }

    #[test]
    fn user_override_adds_a_chord() {
        // Bind "n" -> navigate_down in the worktree layer.
        let mut layer = toml::Table::new();
        layer.insert(
            "n".to_string(),
            toml::Value::String("navigate_down".to_string()),
        );
        let mut layers = toml::Table::new();
        layers.insert("worktree".to_string(), toml::Value::Table(layer));
        let mut user = toml::Table::new();
        user.insert("layers".to_string(), toml::Value::Table(layers));

        let (km, warnings) = KeyMap::with_warnings(&user);
        assert!(warnings.is_empty(), "{warnings:?}");

        // 'n' now navigates down …
        let key_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key_n, KeyContext::Worktree),
            Some(Action::NavigateDown)
        );
        // … and the default 'j' still works (layering, not replacement).
        let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key_j, KeyContext::Worktree),
            Some(Action::NavigateDown)
        );
    }

    #[test]
    fn user_override_shadows_a_default_chord() {
        // Rebind "g" -> go_to_top in worktree (default is grab_branch).
        let mut layer = toml::Table::new();
        layer.insert("g".to_string(), toml::Value::String("go_to_top".to_string()));
        let mut layers = toml::Table::new();
        layers.insert("worktree".to_string(), toml::Value::Table(layer));
        let mut user = toml::Table::new();
        user.insert("layers".to_string(), toml::Value::Table(layers));

        let km = KeyMap::new(&user);
        let key_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key_g, KeyContext::Worktree),
            Some(Action::GoToTop)
        );
    }

    #[test]
    fn user_tombstone_unbinds_a_default_chord() {
        // `"ctrl+q" = false` removes the default Quit binding outright (the
        // keymap-suite `merge` tombstone), so the chord passes through instead of
        // being shadowed by another action. This is a no-op warning-wise.
        let mut keys = toml::Table::new();
        keys.insert("ctrl+q".to_string(), toml::Value::Boolean(false));
        let mut user = toml::Table::new();
        user.insert("keys".to_string(), toml::Value::Table(keys));

        let (km, warnings) = KeyMap::with_warnings(&user);
        assert!(warnings.is_empty(), "{warnings:?}");

        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(&ctrl_q, KeyContext::Global), None);
        // A default the tombstone did not touch still resolves.
        let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&ctrl_n, KeyContext::Global),
            Some(Action::NewClaudeCode)
        );
    }

    #[test]
    fn user_tombstone_in_panel_layer_unbinds() {
        // Tombstones work in a named layer too: drop worktree 'c' (cherry-pick).
        let mut layer = toml::Table::new();
        layer.insert("c".to_string(), toml::Value::Boolean(false));
        let mut layers = toml::Table::new();
        layers.insert("worktree".to_string(), toml::Value::Table(layer));
        let mut user = toml::Table::new();
        user.insert("layers".to_string(), toml::Value::Table(layers));

        let (km, warnings) = KeyMap::with_warnings(&user);
        assert!(warnings.is_empty(), "{warnings:?}");
        let key_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
        assert_eq!(km.resolve(&key_c, KeyContext::Worktree), None);
    }

    #[test]
    fn user_unknown_action_is_warned() {
        let mut keys = toml::Table::new();
        keys.insert(
            "ctrl+z".to_string(),
            toml::Value::String("frobnicate".to_string()),
        );
        let mut user = toml::Table::new();
        user.insert("keys".to_string(), toml::Value::Table(keys));

        let (_km, warnings) = KeyMap::with_warnings(&user);
        assert!(
            warnings.iter().any(|w| matches!(
                w,
                KeybindWarning::UnknownAction { action, .. } if action == "frobnicate"
            )),
            "expected UnknownAction, got {warnings:?}"
        );
    }

    #[test]
    fn legacy_format_is_reported_not_silent() {
        // Old schema: [keybinds.worktree] navigate_down = "j" — a top-level
        // table named "worktree" rather than "keys"/"layers".
        let mut wt = toml::Table::new();
        wt.insert(
            "navigate_down".to_string(),
            toml::Value::String("j".to_string()),
        );
        let mut user = toml::Table::new();
        user.insert("worktree".to_string(), toml::Value::Table(wt));

        let (_km, warnings) = KeyMap::with_warnings(&user);
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, KeybindWarning::InvalidConfig { .. })),
            "expected InvalidConfig, got {warnings:?}"
        );
    }

    #[test]
    fn ctrl_f_is_filename_search_in_viewer() {
        let km = default_keymap();
        let key = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(&key, KeyContext::Viewer),
            Some(Action::SearchFilename)
        );
        assert_eq!(
            km.resolve(&key, KeyContext::ViewerDiffMode),
            Some(Action::SearchFilename)
        );
    }

    #[test]
    fn keys_for_action_lists_canonical_strings() {
        let km = default_keymap();
        let keys = km.keys_for_action(KeyContext::Worktree, Action::NavigateDown);
        assert!(keys.contains(&"j".to_string()), "{keys:?}");
        assert!(keys.contains(&"down".to_string()), "{keys:?}");
    }

    #[test]
    fn action_from_str_as_str_roundtrip() {
        let actions = [
            Action::Quit,
            Action::NavigateDown,
            Action::LeaveTerminal,
            Action::AddComment,
            Action::ScrollHalfPageDown,
        ];
        for action in actions {
            assert_eq!(Action::from_str(action.as_str()), Some(action));
        }
    }

    #[test]
    fn lowercase_char_with_shift_is_not_recased() {
        // Behavior divergence from the old hand-rolled normalizer, locked in:
        // keymap-core trusts the glyph and only drops a redundant sole SHIFT, so
        // 'g'+SHIFT stays Char('g') and hits the bare 'g' binding (GoToTop) — it
        // is NOT re-cased to 'G' (GoToBottom). A terminal that delivers the
        // resolved glyph 'G' (the common case) still hits GoToBottom; see
        // `shift_g_resolves_uppercase_binding`.
        let km = default_keymap();
        let key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(&key, KeyContext::Worktree),
            Some(Action::GoToTop)
        );
    }

    #[test]
    fn macos_unicode_fallback_chords_resolve() {
        // These glyphs are otherwise undetectable-by-eye in the TOML; this proves
        // the file→keymap-config→keymap-core→crossterm path survives multi-byte
        // chars for both the plain-Option and Shift-Option families.
        let km = default_keymap();
        let cases = [
            ('˙', Action::CycleFocusBackward),
            ('¬', Action::CycleFocusForward),
            ('¡', Action::FocusWorktree),
            ('§', Action::FocusTerminalShell),
            ('÷', Action::TogglePanelOverlay),
        ];
        for (glyph, action) in cases {
            let key = KeyEvent::new(KeyCode::Char(glyph), KeyModifiers::empty());
            assert_eq!(km.resolve(&key, KeyContext::Global), Some(action), "glyph {glyph:?}");
        }
    }

    #[test]
    fn alt_shift_digit_does_not_fold_into_alt_digit() {
        // The "keep SHIFT when another modifier is held" rule: alt+1 focuses the
        // worktree, but alt+shift+1 must NOT drop the SHIFT and collapse onto it.
        // alt+shift+digit is now unbound (focus+expand was removed), so a correct
        // resolver returns None rather than folding to FocusWorktree.
        let km = default_keymap();
        let alt_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
        let alt_shift_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(&alt_1, KeyContext::Global),
            Some(Action::FocusWorktree)
        );
        assert_eq!(km.resolve(&alt_shift_1, KeyContext::Global), None);
    }

    #[test]
    fn enter_and_shift_enter_distinct_in_diff_mode() {
        // SHIFT discrimination on a named key, in one layer.
        let km = default_keymap();
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(&enter, KeyContext::ViewerDiffMode),
            Some(Action::ExpandContext)
        );
        assert_eq!(
            km.resolve(&shift_enter, KeyContext::ViewerDiffMode),
            Some(Action::ExpandAllContext)
        );
    }

    #[test]
    fn keys_for_action_uses_lowercase_canonical_form() {
        // The help screen renders these verbatim; pin the casing that changed
        // from the old "Ctrl+d" to keymap-core's canonical "ctrl+d".
        let km = default_keymap();
        let keys = km.keys_for_action(KeyContext::Viewer, Action::ScrollHalfPageDown);
        assert_eq!(keys, vec!["ctrl+d".to_string()]);
    }

    #[test]
    fn unmappable_key_event_passes_through() {
        // A key with no neutral representation (CapsLock) fails KeyInput::try_from
        // and must resolve to None ("pass through"), never panic.
        let km = default_keymap();
        let key = KeyEvent::new(KeyCode::CapsLock, KeyModifiers::empty());
        assert_eq!(km.resolve(&key, KeyContext::Terminal), None);
    }

    #[test]
    fn in_layer_conflict_is_warned() {
        // Two spellings of the same chord in one layer: keymap-config reports a
        // Conflict and the last binding wins.
        let mut keys = toml::Table::new();
        keys.insert("ctrl+x".to_string(), toml::Value::String("quit".to_string()));
        keys.insert(
            "control+x".to_string(),
            toml::Value::String("show_help".to_string()),
        );
        let mut user = toml::Table::new();
        user.insert("keys".to_string(), toml::Value::Table(keys));

        let (km, warnings) = KeyMap::with_warnings(&user);
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, KeybindWarning::Conflict { .. })),
            "expected Conflict, got {warnings:?}"
        );
        // Whichever won, ctrl+x must resolve to one of the two contenders.
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        let resolved = km.resolve(&key, KeyContext::Global);
        assert!(
            matches!(resolved, Some(Action::Quit) | Some(Action::ShowHelp)),
            "got {resolved:?}"
        );
    }

    #[test]
    fn viewer_c_is_add_comment() {
        let km = default_keymap();
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key, KeyContext::Viewer),
            Some(Action::AddComment)
        );
        assert_eq!(
            km.resolve(&key, KeyContext::ViewerDiffMode),
            Some(Action::AddComment)
        );
    }

    #[test]
    fn removed_lsp_actions_no_longer_parse() {
        // Unwired actions were dropped from the vocabulary so binding them
        // warns (UnknownAction) instead of silently doing nothing.
        assert_eq!(Action::from_str("go_to_definition"), None);
        assert_eq!(Action::from_str("go_to_implementation"), None);
        assert_eq!(Action::from_str("find_references"), None);
    }

    #[test]
    fn f_keys_are_unbound_after_cleanup() {
        let km = default_keymap();
        for n in 2..=7 {
            let key = KeyEvent::new(KeyCode::F(n), KeyModifiers::empty());
            assert_eq!(km.resolve(&key, KeyContext::Global), None, "F{n}");
        }
    }

    #[test]
    fn unknown_layer_with_bindings_is_warned() {
        // Guards the empty-GLOBAL_LAYER suppression: a non-empty unrecognized
        // layer name must warn (an empty one, always injected, must not).
        let mut layer = toml::Table::new();
        layer.insert(
            "j".to_string(),
            toml::Value::String("navigate_down".to_string()),
        );
        let mut layers = toml::Table::new();
        layers.insert("bogus".to_string(), toml::Value::Table(layer));
        let mut user = toml::Table::new();
        user.insert("layers".to_string(), toml::Value::Table(layers));

        let (_km, warnings) = KeyMap::with_warnings(&user);
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, KeybindWarning::UnknownLayer { layer } if layer == "bogus")),
            "expected UnknownLayer, got {warnings:?}"
        );
    }
}
