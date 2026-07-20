//! `KeyContext` — selects which keymap layer a key event is resolved against.

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
    /// The Explorer bottom pane showing the AI walkthrough step list.
    ExplorerWalkthrough,
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
pub(crate) const PANEL_CONTEXTS: [KeyContext; 10] = [
    KeyContext::Worktree,
    KeyContext::Explorer,
    KeyContext::ExplorerDiffList,
    KeyContext::ExplorerCommentList,
    KeyContext::ExplorerWalkthrough,
    KeyContext::Viewer,
    KeyContext::ViewerDiffMode,
    KeyContext::Terminal,
    KeyContext::Editor,
    KeyContext::Overlay,
];

impl KeyContext {
    /// The keymap-suite layer name backing this context. `Global` lives in the
    /// bare `[keys]` table, which the suite exposes as the `GLOBAL_LAYER`.
    pub(crate) fn layer_name(self) -> &'static str {
        match self {
            KeyContext::Global => keymap_suite::GLOBAL_LAYER,
            KeyContext::Worktree => "worktree",
            KeyContext::Explorer => "explorer",
            KeyContext::ExplorerDiffList => "explorer_diff_list",
            KeyContext::ExplorerCommentList => "explorer_comment_list",
            KeyContext::ExplorerWalkthrough => "explorer_walkthrough",
            KeyContext::Viewer => "viewer",
            KeyContext::ViewerDiffMode => "viewer_diff_mode",
            KeyContext::Terminal => "terminal",
            KeyContext::Editor => "editor",
            KeyContext::Overlay => "overlay",
        }
    }
}
