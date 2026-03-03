//! Configurable keybindings — maps key chords to semantic actions.
//!
//! Provides a `KeyMap` that resolves `KeyEvent` → `Action` for a given
//! `KeyContext`, with user overrides from `config.toml`.

use std::collections::HashMap;

use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{KeybindValue, KeybindsConfig};

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
    FocusWorktree,
    FocusExplorer,
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

    // ── Terminal panel ────────────────────────────────────────────
    LeaveTerminal,
    ScrollbackUp,
    ScrollbackDown,
    ScrollbackTop,
    SnapToLive,

    // ── App ──────────────────────────────────────────────────────
    UpdateAndRestart,

    // ── Search ──────────────────────────────────────────────────
    SearchFullText,

    // ── Panel layout ────────────────────────────────────────────
    TogglePanelExpand,
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
            "focus_worktree" => Some(Action::FocusWorktree),
            "focus_explorer" => Some(Action::FocusExplorer),
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
            "leave_terminal" => Some(Action::LeaveTerminal),
            "scrollback_up" => Some(Action::ScrollbackUp),
            "scrollback_down" => Some(Action::ScrollbackDown),
            "scrollback_top" => Some(Action::ScrollbackTop),
            "snap_to_live" => Some(Action::SnapToLive),
            "update_and_restart" => Some(Action::UpdateAndRestart),
            "search_full_text" => Some(Action::SearchFullText),
            "toggle_panel_expand" => Some(Action::TogglePanelExpand),
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
            Action::FocusWorktree => "focus_worktree",
            Action::FocusExplorer => "focus_explorer",
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
            Action::LeaveTerminal => "leave_terminal",
            Action::ScrollbackUp => "scrollback_up",
            Action::ScrollbackDown => "scrollback_down",
            Action::ScrollbackTop => "scrollback_top",
            Action::SnapToLive => "snap_to_live",
            Action::UpdateAndRestart => "update_and_restart",
            Action::SearchFullText => "search_full_text",
            Action::TogglePanelExpand => "toggle_panel_expand",
        }
    }
}

// ---------------------------------------------------------------------------
// KeyContext — determines which binding table to consult
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
}

// ---------------------------------------------------------------------------
// Key chord parsing
// ---------------------------------------------------------------------------

/// Parse a key chord string like `"ctrl+shift+a"`, `"enter"`, `"G"`, `"f1"`.
pub fn parse_key_chord(s: &str) -> Result<(KeyCode, KeyModifiers)> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty key chord");
    }

    let parts: Vec<&str> = s.split('+').collect();
    let mut modifiers = KeyModifiers::empty();

    // All parts except the last are modifiers.
    for &part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "alt" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            "super" | "cmd" | "meta" => modifiers |= KeyModifiers::SUPER,
            other => bail!("unknown modifier: {other}"),
        }
    }

    let key_part = parts[parts.len() - 1];
    let code = parse_key_name(key_part)?;

    // Normalise: uppercase single char → remove explicit SHIFT
    // (crossterm delivers 'G' with SHIFT set, but we store without it)
    if let KeyCode::Char(c) = code {
        if c.is_ascii_uppercase() {
            modifiers &= !KeyModifiers::SHIFT;
        }
    }

    Ok((code, modifiers))
}

fn parse_key_name(s: &str) -> Result<KeyCode> {
    // Single character
    let chars: Vec<char> = s.chars().collect();
    if chars.len() == 1 {
        return Ok(KeyCode::Char(chars[0]));
    }

    // Named keys (case-insensitive)
    match s.to_lowercase().as_str() {
        "enter" | "return" => Ok(KeyCode::Enter),
        "esc" | "escape" => Ok(KeyCode::Esc),
        "tab" => Ok(KeyCode::Tab),
        "backtab" => Ok(KeyCode::BackTab),
        "backspace" => Ok(KeyCode::Backspace),
        "delete" | "del" => Ok(KeyCode::Delete),
        "up" => Ok(KeyCode::Up),
        "down" => Ok(KeyCode::Down),
        "left" => Ok(KeyCode::Left),
        "right" => Ok(KeyCode::Right),
        "home" => Ok(KeyCode::Home),
        "end" => Ok(KeyCode::End),
        "pageup" => Ok(KeyCode::PageUp),
        "pagedown" => Ok(KeyCode::PageDown),
        "space" => Ok(KeyCode::Char(' ')),
        "f1" => Ok(KeyCode::F(1)),
        "f2" => Ok(KeyCode::F(2)),
        "f3" => Ok(KeyCode::F(3)),
        "f4" => Ok(KeyCode::F(4)),
        "f5" => Ok(KeyCode::F(5)),
        "f6" => Ok(KeyCode::F(6)),
        "f7" => Ok(KeyCode::F(7)),
        "f8" => Ok(KeyCode::F(8)),
        "f9" => Ok(KeyCode::F(9)),
        "f10" => Ok(KeyCode::F(10)),
        "f11" => Ok(KeyCode::F(11)),
        "f12" => Ok(KeyCode::F(12)),
        _ => bail!("unknown key name: {s}"),
    }
}

/// Format a `(KeyCode, KeyModifiers)` pair back to a human-readable string.
fn format_key_chord(code: &KeyCode, modifiers: &KeyModifiers) -> String {
    let mut parts = Vec::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        parts.push("Cmd".to_string());
    }

    let key_name = match code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Del".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        _ => "?".to_string(),
    };
    parts.push(key_name);
    parts.join("+")
}

// ---------------------------------------------------------------------------
// KeyMap
// ---------------------------------------------------------------------------

pub struct KeyMap {
    bindings: HashMap<KeyContext, HashMap<(KeyCode, KeyModifiers), Action>>,
}

impl KeyMap {
    /// Build a new KeyMap with defaults, then apply user overrides.
    pub fn new(config: &KeybindsConfig) -> Self {
        let mut km = Self {
            bindings: HashMap::new(),
        };
        km.load_defaults();
        km.apply_overrides(config);
        km
    }

    /// Resolve a key event to an action in the given context.
    /// Falls back to `KeyContext::Global` if no match in the specific context.
    pub fn resolve(&self, key: &KeyEvent, context: KeyContext) -> Option<Action> {
        let normalized = normalize_key(key);

        // First try the specific context.
        if context != KeyContext::Global {
            if let Some(map) = self.bindings.get(&context) {
                if let Some(action) = map.get(&normalized) {
                    return Some(*action);
                }
            }
        }

        // Fall back to global.
        if let Some(map) = self.bindings.get(&KeyContext::Global) {
            if let Some(action) = map.get(&normalized) {
                return Some(*action);
            }
        }

        None
    }

    /// Get the display strings for all keys bound to an action in a context.
    pub fn keys_for_action(&self, context: KeyContext, action: Action) -> Vec<String> {
        let mut keys = Vec::new();

        // Check context-specific bindings.
        if let Some(map) = self.bindings.get(&context) {
            for ((code, mods), a) in map {
                if *a == action {
                    keys.push(format_key_chord(code, mods));
                }
            }
        }

        // Also check global bindings for global actions.
        if context != KeyContext::Global {
            if let Some(map) = self.bindings.get(&KeyContext::Global) {
                for ((code, mods), a) in map {
                    if *a == action {
                        keys.push(format_key_chord(code, mods));
                    }
                }
            }
        }

        keys.sort();
        keys.dedup();
        keys
    }

    // ── Binding insertion helpers ─────────────────────────────────

    fn bind(&mut self, context: KeyContext, code: KeyCode, modifiers: KeyModifiers, action: Action) {
        let normalized = normalize_raw(code, modifiers);
        self.bindings
            .entry(context)
            .or_default()
            .insert(normalized, action);
    }

    fn bind_char(&mut self, context: KeyContext, c: char, action: Action) {
        self.bind(context, KeyCode::Char(c), KeyModifiers::empty(), action);
    }

    fn bind_key(&mut self, context: KeyContext, code: KeyCode, action: Action) {
        self.bind(context, code, KeyModifiers::empty(), action);
    }

    fn bind_ctrl(&mut self, context: KeyContext, c: char, action: Action) {
        self.bind(context, KeyCode::Char(c), KeyModifiers::CONTROL, action);
    }

    fn bind_alt(&mut self, context: KeyContext, c: char, action: Action) {
        self.bind(context, KeyCode::Char(c), KeyModifiers::ALT, action);
    }

    // ── Default bindings (mirrors current event.rs hardcoded keys) ──

    fn load_defaults(&mut self) {
        use Action::*;
        use KeyContext::*;

        // ── Global ───────────────────────────────────────────────
        self.bind_char(Global, 'q', Quit);
        self.bind_char(Global, 'Q', Quit);
        self.bind_char(Global, '?', ShowHelp);
        self.bind_char(Global, ':', CommandPalette);
        self.bind_ctrl(Global, 'p', CommandPalette);
        self.bind_key(Global, KeyCode::Tab, CycleFocusForward);
        self.bind_key(Global, KeyCode::BackTab, CycleFocusBackward);
        self.bind_ctrl(Global, 'w', FocusWorktree);
        self.bind_ctrl(Global, 'n', NewClaudeCode);
        self.bind_ctrl(Global, 't', NewShell);
        self.bind_ctrl(Global, 'o', OpenRepo);
        self.bind_ctrl(Global, 'r', SwitchRepo);
        self.bind_alt(Global, '1', FocusWorktree);
        self.bind_alt(Global, '2', FocusExplorer);
        self.bind_alt(Global, '3', FocusViewer);
        self.bind_alt(Global, '4', FocusTerminalClaude);
        self.bind_alt(Global, '5', FocusTerminalShell);
        self.bind_ctrl(Global, 'g', SearchFullText);
        self.bind(Global, KeyCode::Char(' '), KeyModifiers::SUPER, TogglePanelExpand);

        // ── Worktree ─────────────────────────────────────────────
        self.bind_char(Worktree, 'j', NavigateDown);
        self.bind_key(Worktree, KeyCode::Down, NavigateDown);
        self.bind_char(Worktree, 'k', NavigateUp);
        self.bind_key(Worktree, KeyCode::Up, NavigateUp);
        self.bind_key(Worktree, KeyCode::Enter, Select);
        self.bind_char(Worktree, 'w', CreateWorktree);
        self.bind_char(Worktree, 'x', DeleteWorktree);
        self.bind_key(Worktree, KeyCode::Delete, DeleteWorktree);
        self.bind_char(Worktree, 's', SwitchBranch);
        self.bind_char(Worktree, 'g', GrabBranch);
        self.bind_char(Worktree, 'G', UngrabBranch);
        self.bind_char(Worktree, 'P', PruneWorktrees);
        self.bind_char(Worktree, 'm', MergeToMain);
        self.bind_char(Worktree, 'r', RefreshWorktrees);
        self.bind_char(Worktree, 'R', ResetMainToOrigin);
        self.bind_char(Worktree, 'p', CherryPick);
        self.bind_char(Worktree, 'u', PullWorktree);
        self.bind_char(Worktree, 'H', SessionHistory);
        self.bind_char(Worktree, 'v', OpenPullRequest);

        // ── Explorer (file tree) ─────────────────────────────────
        self.bind_char(Explorer, 'j', NavigateDown);
        self.bind_key(Explorer, KeyCode::Down, NavigateDown);
        self.bind_char(Explorer, 'k', NavigateUp);
        self.bind_key(Explorer, KeyCode::Up, NavigateUp);
        self.bind_char(Explorer, 'l', ExpandOrRight);
        self.bind_key(Explorer, KeyCode::Right, ExpandOrRight);
        self.bind_char(Explorer, 'h', CollapseOrLeft);
        self.bind_key(Explorer, KeyCode::Left, CollapseOrLeft);
        self.bind_key(Explorer, KeyCode::Enter, Select);
        self.bind_char(Explorer, 'g', GoToTop);
        self.bind_char(Explorer, 'G', GoToBottom);
        self.bind_char(Explorer, 'd', ShowDiffList);
        self.bind_char(Explorer, 'c', ShowCommentList);
        self.bind_char(Explorer, '/', SearchFilename);

        // ── Explorer: diff list ──────────────────────────────────
        self.bind_char(ExplorerDiffList, 'j', NavigateDown);
        self.bind_key(ExplorerDiffList, KeyCode::Down, NavigateDown);
        self.bind_char(ExplorerDiffList, 'k', NavigateUp);
        self.bind_key(ExplorerDiffList, KeyCode::Up, NavigateUp);
        self.bind_char(ExplorerDiffList, 'h', CollapseOrLeft);
        self.bind_key(ExplorerDiffList, KeyCode::Left, CollapseOrLeft);
        self.bind_char(ExplorerDiffList, 'l', ExpandOrRight);
        self.bind_key(ExplorerDiffList, KeyCode::Right, ExpandOrRight);
        self.bind_key(ExplorerDiffList, KeyCode::Enter, Select);
        self.bind_char(ExplorerDiffList, 'g', GoToTop);
        self.bind_char(ExplorerDiffList, 'G', GoToBottom);
        self.bind_key(ExplorerDiffList, KeyCode::Esc, ExitSubPanel);

        // ── Explorer: comment list ───────────────────────────────
        self.bind_char(ExplorerCommentList, 'j', NavigateDown);
        self.bind_key(ExplorerCommentList, KeyCode::Down, NavigateDown);
        self.bind_char(ExplorerCommentList, 'k', NavigateUp);
        self.bind_key(ExplorerCommentList, KeyCode::Up, NavigateUp);
        self.bind_char(ExplorerCommentList, 'g', GoToTop);
        self.bind_char(ExplorerCommentList, 'G', GoToBottom);
        self.bind_char(ExplorerCommentList, 'h', CollapseOrLeft);
        self.bind_key(ExplorerCommentList, KeyCode::Left, CollapseOrLeft);
        self.bind_key(ExplorerCommentList, KeyCode::Enter, Select);
        self.bind_char(ExplorerCommentList, 'l', ExpandOrRight);
        self.bind_key(ExplorerCommentList, KeyCode::Right, ExpandOrRight);
        self.bind_char(ExplorerCommentList, 'x', DeleteComment);
        self.bind_key(ExplorerCommentList, KeyCode::Delete, DeleteComment);
        self.bind_char(ExplorerCommentList, 'r', ToggleResolve);
        self.bind_char(ExplorerCommentList, 'e', EditComment);
        self.bind_char(ExplorerCommentList, 'R', ReplyToComment);
        self.bind_char(ExplorerCommentList, ' ', ViewCommentDetail);
        self.bind_key(ExplorerCommentList, KeyCode::Esc, ExitSubPanel);

        // ── Viewer ───────────────────────────────────────────────
        self.bind_char(Viewer, 'j', NavigateDown);
        self.bind_key(Viewer, KeyCode::Down, NavigateDown);
        self.bind_char(Viewer, 'k', NavigateUp);
        self.bind_key(Viewer, KeyCode::Up, NavigateUp);
        self.bind_ctrl(Viewer, 'd', ScrollHalfPageDown);
        self.bind_ctrl(Viewer, 'u', ScrollHalfPageUp);
        self.bind_char(Viewer, 'g', GoToTop);
        self.bind_char(Viewer, 'G', GoToBottom);
        self.bind_char(Viewer, 'h', ScrollLeft);
        self.bind_key(Viewer, KeyCode::Left, ScrollLeft);
        self.bind_char(Viewer, 'l', ScrollRight);
        self.bind_key(Viewer, KeyCode::Right, ScrollRight);
        self.bind_char(Viewer, '0', ScrollHome);
        self.bind_char(Viewer, '/', SearchInFile);
        self.bind_char(Viewer, 'n', NextSearchMatch);
        self.bind_char(Viewer, 'N', PrevSearchMatch);
        self.bind_char(Viewer, 'c', AddComment);
        self.bind_char(Viewer, ' ', ViewCommentDetail);
        self.bind_key(Viewer, KeyCode::Esc, ExitToExplorer);

        // ── Viewer: diff mode ────────────────────────────────────
        self.bind_char(ViewerDiffMode, 'j', NavigateDown);
        self.bind_key(ViewerDiffMode, KeyCode::Down, NavigateDown);
        self.bind_char(ViewerDiffMode, 'k', NavigateUp);
        self.bind_key(ViewerDiffMode, KeyCode::Up, NavigateUp);
        self.bind_ctrl(ViewerDiffMode, 'd', ScrollHalfPageDown);
        self.bind_ctrl(ViewerDiffMode, 'u', ScrollHalfPageUp);
        self.bind_char(ViewerDiffMode, 'g', GoToTop);
        self.bind_char(ViewerDiffMode, 'G', GoToBottom);
        self.bind_char(ViewerDiffMode, 'h', ScrollLeft);
        self.bind_key(ViewerDiffMode, KeyCode::Left, ScrollLeft);
        self.bind_char(ViewerDiffMode, 'l', ScrollRight);
        self.bind_key(ViewerDiffMode, KeyCode::Right, ScrollRight);
        self.bind_char(ViewerDiffMode, '0', ScrollHome);
        self.bind_char(ViewerDiffMode, 'c', AddComment);
        self.bind_char(ViewerDiffMode, ' ', ViewCommentDetail);
        self.bind_key(ViewerDiffMode, KeyCode::Esc, ExitToExplorer);

        // ── Terminal ─────────────────────────────────────────────
        self.bind(Terminal, KeyCode::Esc, KeyModifiers::CONTROL, LeaveTerminal);
        self.bind(Terminal, KeyCode::PageUp, KeyModifiers::SHIFT, ScrollbackUp);
        self.bind(Terminal, KeyCode::PageDown, KeyModifiers::SHIFT, ScrollbackDown);
        self.bind(Terminal, KeyCode::Home, KeyModifiers::SHIFT, ScrollbackTop);
        self.bind(Terminal, KeyCode::End, KeyModifiers::SHIFT, SnapToLive);
    }

    /// Apply user overrides from config. For each overridden action,
    /// remove all existing bindings for that action in the context,
    /// then insert the new key(s).
    fn apply_overrides(&mut self, config: &KeybindsConfig) {
        self.apply_section_overrides(KeyContext::Global, &config.global);
        self.apply_section_overrides(KeyContext::Worktree, &config.worktree);
        self.apply_section_overrides(KeyContext::Explorer, &config.explorer);
        self.apply_section_overrides(KeyContext::Viewer, &config.viewer);
        self.apply_section_overrides(KeyContext::Terminal, &config.terminal);
    }

    fn apply_section_overrides(
        &mut self,
        context: KeyContext,
        overrides: &HashMap<String, KeybindValue>,
    ) {
        for (action_name, value) in overrides {
            let Some(action) = Action::from_str(action_name) else {
                log::warn!("unknown keybind action: {action_name}");
                continue;
            };

            // Remove all existing bindings for this action in this context.
            if let Some(map) = self.bindings.get_mut(&context) {
                map.retain(|_, a| *a != action);
            }

            // Parse and insert the new key(s).
            let keys: Vec<&str> = match value {
                KeybindValue::Single(s) => vec![s.as_str()],
                KeybindValue::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
            };

            for key_str in keys {
                match parse_key_chord(key_str) {
                    Ok((code, mods)) => {
                        self.bind(context, code, mods, action);
                    }
                    Err(e) => {
                        log::warn!("invalid key chord '{key_str}' for action '{action_name}': {e}");
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key normalisation
// ---------------------------------------------------------------------------

/// Normalise a KeyEvent for lookup: strip SHIFT from uppercase ASCII chars.
fn normalize_key(key: &KeyEvent) -> (KeyCode, KeyModifiers) {
    normalize_raw(key.code, key.modifiers)
}

fn normalize_raw(code: KeyCode, mut modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    if let KeyCode::Char(c) = code {
        if c.is_ascii_uppercase() {
            modifiers &= !KeyModifiers::SHIFT;
        }
    }
    // Strip state flags that aren't meaningful for binding lookup.
    modifiers &= KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::SUPER;
    (code, modifiers)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_key_chord_simple_char() {
        let (code, mods) = parse_key_chord("j").unwrap();
        assert_eq!(code, KeyCode::Char('j'));
        assert_eq!(mods, KeyModifiers::empty());
    }

    #[test]
    fn test_parse_key_chord_uppercase() {
        let (code, mods) = parse_key_chord("G").unwrap();
        assert_eq!(code, KeyCode::Char('G'));
        // Shift is stripped for uppercase chars.
        assert_eq!(mods, KeyModifiers::empty());
    }

    #[test]
    fn test_parse_key_chord_ctrl() {
        let (code, mods) = parse_key_chord("ctrl+d").unwrap();
        assert_eq!(code, KeyCode::Char('d'));
        assert_eq!(mods, KeyModifiers::CONTROL);
    }

    #[test]
    fn test_parse_key_chord_ctrl_esc() {
        let (code, mods) = parse_key_chord("ctrl+esc").unwrap();
        assert_eq!(code, KeyCode::Esc);
        assert_eq!(mods, KeyModifiers::CONTROL);
    }

    #[test]
    fn test_parse_key_chord_special_keys() {
        assert_eq!(parse_key_chord("enter").unwrap().0, KeyCode::Enter);
        assert_eq!(parse_key_chord("tab").unwrap().0, KeyCode::Tab);
        assert_eq!(parse_key_chord("space").unwrap().0, KeyCode::Char(' '));
        assert_eq!(parse_key_chord("delete").unwrap().0, KeyCode::Delete);
        assert_eq!(parse_key_chord("up").unwrap().0, KeyCode::Up);
        assert_eq!(parse_key_chord("f1").unwrap().0, KeyCode::F(1));
    }

    #[test]
    fn test_parse_key_chord_alt_number() {
        let (code, mods) = parse_key_chord("alt+1").unwrap();
        assert_eq!(code, KeyCode::Char('1'));
        assert_eq!(mods, KeyModifiers::ALT);
    }

    #[test]
    fn test_parse_key_chord_shift_pageup() {
        let (code, mods) = parse_key_chord("shift+pageup").unwrap();
        assert_eq!(code, KeyCode::PageUp);
        assert_eq!(mods, KeyModifiers::SHIFT);
    }

    #[test]
    fn test_parse_key_chord_error() {
        assert!(parse_key_chord("").is_err());
        assert!(parse_key_chord("foobar").is_err());
    }

    #[test]
    fn test_parse_key_chord_super() {
        let (code, mods) = parse_key_chord("super+space").unwrap();
        assert_eq!(code, KeyCode::Char(' '));
        assert_eq!(mods, KeyModifiers::SUPER);

        let (code, mods) = parse_key_chord("cmd+a").unwrap();
        assert_eq!(code, KeyCode::Char('a'));
        assert_eq!(mods, KeyModifiers::SUPER);

        let (code, mods) = parse_key_chord("meta+b").unwrap();
        assert_eq!(code, KeyCode::Char('b'));
        assert_eq!(mods, KeyModifiers::SUPER);
    }

    #[test]
    fn test_default_bindings_complete() {
        let config = KeybindsConfig::default();
        let km = KeyMap::new(&config);

        // Check a few critical defaults.
        let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());
        assert_eq!(km.resolve(&key_q, KeyContext::Global), Some(Action::Quit));

        let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
        assert_eq!(km.resolve(&key_j, KeyContext::Worktree), Some(Action::NavigateDown));

        let key_ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(&key_ctrl_n, KeyContext::Global), Some(Action::NewClaudeCode));

        let key_ctrl_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL);
        assert_eq!(km.resolve(&key_ctrl_esc, KeyContext::Terminal), Some(Action::LeaveTerminal));
    }

    #[test]
    fn test_resolve_context_fallback() {
        let config = KeybindsConfig::default();
        let km = KeyMap::new(&config);

        // Tab is only in Global — should be resolved from Worktree via fallback.
        let key_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(km.resolve(&key_tab, KeyContext::Worktree), Some(Action::CycleFocusForward));
    }

    #[test]
    fn test_resolve_context_shadows_global() {
        let config = KeybindsConfig::default();
        let km = KeyMap::new(&config);

        // 'g' in Worktree = GrabBranch, in Global = not bound.
        let key_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty());
        assert_eq!(km.resolve(&key_g, KeyContext::Worktree), Some(Action::GrabBranch));
        // In Explorer, 'g' = GoToTop.
        assert_eq!(km.resolve(&key_g, KeyContext::Explorer), Some(Action::GoToTop));
    }

    #[test]
    fn test_user_override_replaces_default() {
        let mut config = KeybindsConfig::default();
        config.worktree.insert(
            "navigate_down".to_string(),
            KeybindValue::Multiple(vec!["n".to_string(), "down".to_string()]),
        );

        let km = KeyMap::new(&config);

        // 'j' should no longer be NavigateDown in worktree.
        let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
        assert_ne!(km.resolve(&key_j, KeyContext::Worktree), Some(Action::NavigateDown));

        // 'n' should now be NavigateDown.
        let key_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty());
        assert_eq!(km.resolve(&key_n, KeyContext::Worktree), Some(Action::NavigateDown));

        // Down arrow should still work.
        let key_down = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(km.resolve(&key_down, KeyContext::Worktree), Some(Action::NavigateDown));
    }

    #[test]
    fn test_action_from_str_roundtrip() {
        let actions = [
            Action::Quit,
            Action::NavigateDown,
            Action::LeaveTerminal,
            Action::AddComment,
            Action::ScrollHalfPageDown,
        ];
        for action in actions {
            let s = action.as_str();
            let parsed = Action::from_str(s);
            assert_eq!(parsed, Some(action), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn test_keys_for_action() {
        let config = KeybindsConfig::default();
        let km = KeyMap::new(&config);

        let keys = km.keys_for_action(KeyContext::Worktree, Action::NavigateDown);
        assert!(keys.contains(&"j".to_string()));
        assert!(keys.contains(&"Down".to_string()));
    }

    #[test]
    fn test_normalize_shift_uppercase() {
        // Simulates crossterm sending 'G' with SHIFT modifier.
        let key = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        let (code, mods) = normalize_key(&key);
        assert_eq!(code, KeyCode::Char('G'));
        assert_eq!(mods, KeyModifiers::empty());
    }
}
