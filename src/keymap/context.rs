//! KeyContext — キーイベントをどのキーマップレイヤーに対して解決するかを選ぶ。

// KeyContext — どのレイヤーを参照するかを選ぶ

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyContext {
    Global,
    Worktree,
    Explorer,
    ExplorerDiffList,
    ExplorerCommentList,
    /// AI walkthrough のステップ一覧を表示する Explorer 下段ペイン。
    ExplorerWalkthrough,
    Viewer,
    ViewerDiffMode,
    Terminal,
    /// 埋め込みエディタパネル。Terminal と同様、ほとんどのチョードは内側の
    /// プログラム（vim/emacs）へ転送される — 自分自身のレイヤーは「フォーカスを
    /// 抜ける」チョードだけをバインドし、残りはグローバルレイヤー
    /// （terminal で発火する操作にフィルタされたもの）か PTY へ落ちる。
    Editor,
    /// オーバーレイのポップアップ（リスト/ツリーのナビゲーション）で共有される
    /// コンテキスト。他のコンテキストと同じく Global にフォールバックする。
    Overlay,
}

/// グローバル以外のコンテキスト。それぞれ名前付きの [layers.<name>] テーブルで裏打ちされる。
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
    /// このコンテキストを裏打ちする keymap-suite のレイヤー名。Global は
    /// 素の [keys] テーブルにあり、suite はこれを GLOBAL_LAYER として公開する。
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
