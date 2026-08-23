//! コマンドパレットのデータモデル: コマンドの分類体系、カテゴリ、スコープ、
//! および PaletteCommand/ScoredCommand のレコード型。

use crate::keymap::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    // Navigation
    FocusWorktree,
    FocusExplorer,
    FocusViewer,
    FocusTerminalClaude,
    FocusTerminalShell,
    NextWorktree,
    PrevWorktree,
    TogglePanelExpand,
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,

    // Worktree
    CreateWorktree,
    DeleteWorktree,
    SwitchBranch,
    GrabBranch,
    PruneWorktrees,
    MergeToMain,
    RefreshWorktrees,
    ResetMainToOrigin,
    CherryPick,
    PullWorktree,

    // Terminal
    NewClaudeCode,
    NewShell,
    ResumeClaudeSession,

    // Git
    RefreshDiff,

    // View
    SearchInFile,
    ToggleHelp,
    ToggleMarkdownRender,

    // Review
    ShowReviewComments,
    ShowReviewTemplates,
    SessionHistory,
    ReviewPullRequest,
    AnalyzeRevidere,
    ForceAnalyzeRevidere,
    PublishReview,

    // Repository
    OpenRepo,
    SwitchRepo,

    // Worktree (追加分)
    UngrabBranch,

    // Explorer
    ShowDiffList,
    ShowCommentList,
    ShowRevidere,

    // Viewer / Review
    AddReviewComment,
    ViewCommentDetail,

    // コメント操作
    DeleteComment,
    ToggleCommentResolve,
    EditComment,
    ReplyToComment,

    // Session
    SaveSessionHistory,

    // Search
    SearchFullText,

    // GitHub / PR
    OpenPullRequest,

    // App
    UpdateAndRestart,
    CheckForUpdate,
    RebuildCodeIndex,
    Quit,

    // UI
    SwitchTheme,
    ToggleHighContrast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Navigation,
    Worktree,
    Terminal,
    Git,
    View,
    Review,
    Repository,
    App,
}

impl CommandCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Worktree => "Worktree",
            Self::Terminal => "Terminal",
            Self::Git => "Git",
            Self::View => "View",
            Self::Review => "Review",
            Self::Repository => "Repository",
            Self::App => "App",
        }
    }
}

/// コマンドがフォーカス中パネルから見てどこに位置するか: グローバルに
/// バインドされているか、現在のパネル自身のレイヤーにバインドされているか、
/// あるいは別のパネルのレイヤーにだけバインドされている (それでもパレットからは
/// 実行できる) か。グループ表示の並び順を決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandScope {
    Current,
    Global,
    Other,
}

/// フィルタ結果を [CommandScope] でグループ化するための並び順ランク
/// (現在のパネル → グローバル → 他のパネルの順)。search::filter_commands と
/// そのテストで共有する。
pub(super) fn scope_rank(scope: CommandScope) -> u8 {
    match scope {
        CommandScope::Current => 0,
        CommandScope::Global => 1,
        CommandScope::Other => 2,
    }
}

pub struct PaletteCommand {
    pub id: CommandId,
    pub label: &'static str,
    pub category: CommandCategory,
    /// このコマンドがキーバインドを持つ場合、実行する keymap のアクション。
    /// パレット専用コマンド (チョードなし) では None。表示するショートカットと
    /// スコープは、これを keymap 経由で見て導出する。
    pub action: Option<Action>,
    pub keywords: &'static str,
}

pub struct ScoredCommand {
    pub index: usize,
    pub score: i32,
    pub scope: CommandScope,
}
