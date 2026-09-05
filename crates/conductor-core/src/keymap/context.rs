//! キーイベントをどのレイヤーに対して解決するかを選ぶ。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyContext {
    Global,
    Worktree,
    Explorer,
    ExplorerDiffList,
    ExplorerCommitLog,
    ExplorerCommentList,
    Viewer,
    ViewerDiffMode,
    Terminal,
    Editor,
    Revidere,
    Overlay,
}

impl KeyContext {
    /// Global 以外の全コンテキスト。それぞれ [layers.<name>] テーブルで裏打ちされる。
    pub const PANELS: [KeyContext; 11] = [
        KeyContext::Worktree,
        KeyContext::Explorer,
        KeyContext::ExplorerDiffList,
        KeyContext::ExplorerCommitLog,
        KeyContext::ExplorerCommentList,
        KeyContext::Viewer,
        KeyContext::ViewerDiffMode,
        KeyContext::Terminal,
        KeyContext::Editor,
        KeyContext::Revidere,
        KeyContext::Overlay,
    ];

    pub(crate) fn layer_name(self) -> &'static str {
        match self {
            KeyContext::Global => keymap_suite::GLOBAL_LAYER,
            KeyContext::Worktree => "worktree",
            KeyContext::Explorer => "explorer",
            KeyContext::ExplorerDiffList => "explorer_diff_list",
            KeyContext::ExplorerCommitLog => "explorer_commit_log",
            KeyContext::ExplorerCommentList => "explorer_comment_list",
            KeyContext::Viewer => "viewer",
            KeyContext::ViewerDiffMode => "viewer_diff_mode",
            KeyContext::Terminal => "terminal",
            KeyContext::Editor => "editor",
            KeyContext::Revidere => "revidere",
            KeyContext::Overlay => "overlay",
        }
    }

    /// 解決しなかったキーを内側のプログラム (PTY) へ転送するコンテキスト。
    pub(crate) fn forwards_to_pty(self) -> bool {
        matches!(self, KeyContext::Terminal | KeyContext::Editor)
    }
}
