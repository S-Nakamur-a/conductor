//! 実行できる操作の一覧。1 コマンド 1 エントリで、表は分類と表示だけを持つ。

mod exec;
#[cfg(test)]
mod tests;

pub use exec::{Enabled, enabled, execute};

use conductor_core::keymap::{Action, KeyContext, KeyMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
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
    OpenPullRequest,

    NewClaudeCode,
    NewShell,
    ResumeClaudeSession,
    SaveSessionHistory,
    SessionHistory,

    RefreshDiff,

    SearchInFile,
    SearchFullText,
    ToggleMarkdownRender,
    FoldOneLevel,
    UnfoldOneLevel,
    FoldAll,
    UnfoldAll,
    ToggleHelp,
    ShowDiffList,
    ShowCommentList,
    ShowRevidere,
    SwitchTheme,
    ToggleHighContrast,

    ShowReviewComments,
    ShowReviewTemplates,
    ReviewPullRequest,
    AnalyzeRevidere,
    ForceAnalyzeRevidere,
    PublishReview,
    AddReviewComment,
    ViewCommentDetail,
    DeleteComment,
    ToggleCommentResolve,
    EditComment,
    ReplyToComment,

    OpenRepo,
    SwitchRepo,
    RebuildCodeIndex,
    CheckForUpdate,
    UpdateAndRestart,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Navigation,
    Worktree,
    Terminal,
    Git,
    View,
    Review,
    Repository,
    App,
}

impl Category {
    pub fn label(self) -> &'static str {
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

/// コマンドがフォーカス中のパネルからどう見えるか。パレットの並び順を決める。
/// Other でもパレットからは実行できる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Current,
    Global,
    Other,
}

pub struct Command {
    pub id: CommandId,
    pub label: &'static str,
    pub category: Category,
    /// キーバインドを持つなら、それを引くための Action。表示するチョードと
    /// スコープはここから keymap 経由で導くので、表が古びることがない。
    pub action: Option<Action>,
    pub keywords: &'static str,
}

/// 絞り込みの 1 件。`index` は [COMMANDS] への添字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub index: usize,
    pub score: i32,
    pub scope: Scope,
}

macro_rules! commands {
    ($($id:ident, $label:literal, $category:ident, $action:expr, $keywords:literal;)*) => {
        pub const COMMANDS: &[Command] = &[$(Command {
            id: CommandId::$id,
            label: $label,
            category: Category::$category,
            action: $action,
            keywords: $keywords,
        },)*];
    };
}

// 折りたたみの 2 打鍵目は再割り当てできる Action を持たないので、キーはラベルに書く。
commands! {
    FocusWorktree, "Focus: Worktree Panel", Navigation, Some(Action::FocusWorktree), "panel switch";
    FocusExplorer, "Focus: Explorer Panel", Navigation, Some(Action::FocusExplorer), "panel files";
    FocusViewer, "Focus: Viewer Panel", Navigation, Some(Action::FocusViewer), "panel file view";
    FocusTerminalClaude, "Focus: Claude Code Terminal", Navigation, Some(Action::FocusTerminalClaude), "terminal claude";
    FocusTerminalShell, "Focus: Shell Terminal", Navigation, Some(Action::FocusTerminalShell), "terminal shell";
    NextWorktree, "Next Worktree", Navigation, Some(Action::NextWorktree), "worktree switch next cycle tab";
    PrevWorktree, "Previous Worktree", Navigation, Some(Action::PrevWorktree), "worktree switch previous cycle tab";
    TogglePanelExpand, "Toggle Panel Expand", Navigation, Some(Action::TogglePanelExpand), "resize maximize fullscreen";
    ResizePaneLeft, "Layout: Resize Pane Left", Navigation, Some(Action::ResizePaneLeft), "resize pane panel width column shrink grow tmux left";
    ResizePaneRight, "Layout: Resize Pane Right", Navigation, Some(Action::ResizePaneRight), "resize pane panel width column shrink grow tmux right";
    ResizePaneUp, "Layout: Resize Pane Up", Navigation, Some(Action::ResizePaneUp), "resize pane panel height shell claude split shorter taller tmux up";
    ResizePaneDown, "Layout: Resize Pane Down", Navigation, Some(Action::ResizePaneDown), "resize pane panel height shell claude split shorter taller tmux down";

    CreateWorktree, "Worktree: Create New", Worktree, Some(Action::CreateWorktree), "branch new add";
    DeleteWorktree, "Worktree: Delete Selected", Worktree, Some(Action::DeleteWorktree), "remove branch";
    SwitchBranch, "Worktree: Switch Branch (Remote)", Worktree, Some(Action::SwitchBranch), "checkout remote";
    GrabBranch, "Worktree: Grab Branch", Worktree, Some(Action::GrabBranch), "grab checkout branch";
    UngrabBranch, "Worktree: Ungrab Branch", Worktree, Some(Action::UngrabBranch), "ungrab release branch";
    PruneWorktrees, "Worktree: Prune Stale", Worktree, Some(Action::PruneWorktrees), "clean stale";
    MergeToMain, "Worktree: Merge into Main", Worktree, Some(Action::MergeToMain), "merge main";
    RefreshWorktrees, "Worktree: Refresh List", Worktree, Some(Action::RefreshWorktrees), "reload update";
    ResetMainToOrigin, "Worktree: Reset Main to Origin", Worktree, Some(Action::ResetMainToOrigin), "reset origin";
    CherryPick, "Worktree: Cherry-pick", Worktree, Some(Action::CherryPick), "cherry pick commit";
    PullWorktree, "Worktree: Pull (fast-forward)", Worktree, Some(Action::PullWorktree), "pull fetch update fast-forward ff sync";
    OpenPullRequest, "Worktree: Open Pull Request", Worktree, Some(Action::OpenPullRequest), "pr github browser web open";

    NewClaudeCode, "Terminal: New Claude Code", Terminal, Some(Action::NewClaudeCode), "spawn ai";
    NewShell, "Terminal: New Shell", Terminal, Some(Action::NewShell), "spawn bash zsh";
    ResumeClaudeSession, "Terminal: Resume Claude Session", Terminal, None, "resume continue";
    SaveSessionHistory, "Terminal: Save Output", Terminal, None, "save record session output snapshot scrollback";
    SessionHistory, "Terminal: Saved Output…", Terminal, Some(Action::SessionHistory), "history log session output snapshot saved terminal claude shell";

    RefreshDiff, "Diff: Refresh", Git, None, "reload diff";

    SearchInFile, "Search in File", View, Some(Action::SearchInFile), "find grep";
    SearchFullText, "Search: Full-text Search (Grep)", View, Some(Action::SearchFullText), "grep search find text content regex ripgrep fulltext";
    ToggleMarkdownRender, "Viewer: Toggle Markdown Raw / Rendered", View, Some(Action::ToggleMarkdownRender), "markdown md render raw preview prose readme";
    FoldOneLevel, "Viewer: Fold One Level (zm)", View, None, "fold collapse depth nesting level outline structure zm";
    UnfoldOneLevel, "Viewer: Unfold One Level (zr)", View, None, "unfold expand depth nesting level outline structure zr";
    FoldAll, "Viewer: Fold All (zM)", View, None, "fold collapse all everything zM";
    UnfoldAll, "Viewer: Unfold All (zR)", View, None, "unfold expand all everything zR";
    ToggleHelp, "Show Help", View, Some(Action::ShowHelp), "keybindings shortcuts";
    ShowDiffList, "Explorer: Show Diff List", View, Some(Action::ShowDiffList), "diff changed files";
    ShowCommentList, "Explorer: Show Comment List", View, Some(Action::ShowCommentList), "comment review list";
    ShowRevidere, "Review: Show Review (sections + diff)", View, Some(Action::ShowRevidere), "revidere review sections importance diff two column reading order";
    SwitchTheme, "Switch Theme", View, Some(Action::OpenThemePicker), "theme color light dark appearance palette catppuccin solarized github";
    ToggleHighContrast, "UI: Toggle High Contrast", View, None, "high contrast accessibility a11y legibility bright bold theme readable vision";

    ShowReviewComments, "Review: Show Comments", Review, None, "comment list";
    ShowReviewTemplates, "Review: Show Templates", Review, None, "template prompt";
    ReviewPullRequest, "Review: Review Pull Request…", Review, Some(Action::ReviewPullRequest), "pr pull request github fetch worktree review number url";
    AnalyzeRevidere, "Review: Review Current Branch…", Review, Some(Action::AnalyzeRevidere), "revidere analyse analyze generate ai review sections coverage branch";
    ForceAnalyzeRevidere, "Review: Re-analyse Current Branch", Review, Some(Action::ForceAnalyzeRevidere), "revidere reanalyse regenerate force ignore cache rebuild ai review";
    PublishReview, "Review: Publish Comments to GitHub", Review, Some(Action::PublishReview), "publish post github pr comments review upload";
    AddReviewComment, "Review: Add Comment", Review, Some(Action::AddComment), "new comment add write";
    ViewCommentDetail, "Review: View Comment Detail", Review, Some(Action::ViewCommentDetail), "detail preview";
    DeleteComment, "Review: Delete Comment", Review, Some(Action::DeleteComment), "remove delete";
    ToggleCommentResolve, "Review: Toggle Resolve", Review, Some(Action::ToggleResolve), "resolve unresolve status";
    EditComment, "Review: Edit Comment", Review, Some(Action::EditComment), "edit modify update";
    ReplyToComment, "Review: Reply to Comment", Review, Some(Action::ReplyToComment), "reply respond";

    OpenRepo, "Repository: Open by Path", Repository, Some(Action::OpenRepo), "open directory";
    SwitchRepo, "Repository: Switch", Repository, Some(Action::SwitchRepo), "project change";
    RebuildCodeIndex, "Repo: Rebuild Code Index", Repository, None, "index scip code jump definition stale rebuild regenerate semantic";

    CheckForUpdate, "App: Check for Updates", App, None, "update upgrade version check latest release new github";
    UpdateAndRestart, "App: Update and Restart", App, None, "update upgrade restart download version";
    Quit, "Quit Conductor", App, Some(Action::Quit), "exit close";
}

pub fn find(id: CommandId) -> &'static Command {
    COMMANDS
        .iter()
        .find(|c| c.id == id)
        .expect("COMMANDS covers every CommandId")
}

/// グローバルに割り当てたアクションは、パネル側で重ねて割り当てていても常に Global。
fn scope_of(command: &Command, keymap: &KeyMap, current: KeyContext) -> Scope {
    let Some(action) = command.action else {
        return Scope::Global;
    };
    if !keymap.keys_in_layer(KeyContext::Global, action).is_empty() {
        Scope::Global
    } else if current != KeyContext::Global && !keymap.keys_in_layer(current, action).is_empty() {
        Scope::Current
    } else {
        Scope::Other
    }
}

/// 小文字化したクエリに対するあいまいスコア。一致しなければ None。
fn score(command: &Command, query: &str) -> Option<i32> {
    let label = command.label.to_lowercase();
    let keywords = command.keywords.to_lowercase();
    let category = command.category.label().to_lowercase();
    if !format!("{label} {keywords} {category}").contains(query) {
        return None;
    }
    let word_start = label
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(query));
    Some(
        i32::from(label.starts_with(query)) * 100
            + i32::from(word_start) * 50
            + i32::from(label.contains(query)) * 20
            + i32::from(keywords.contains(query)) * 10
            + i32::from(category.contains(query)) * 5,
    )
}

/// クエリで絞り込み、スコープ順・関連度順に並べる。空クエリならスコープ順の全件。
///
/// 描画も選択もこの並びを共有する。選択位置はこの並びへの添字。
pub fn filter(query: &str, keymap: &KeyMap, current: KeyContext) -> Vec<Hit> {
    let query = query.to_lowercase();
    let mut hits: Vec<Hit> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            let score = if query.is_empty() {
                0
            } else {
                score(command, &query)?
            };
            Some(Hit {
                index,
                score,
                scope: scope_of(command, keymap, current),
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        a.scope
            .cmp(&b.scope)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.index.cmp(&b.index))
    });
    hits
}
