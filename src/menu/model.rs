//! 静的なメニューテーブル — どのコマンドがどのトップレベルメニューの、どの順序、
//! どの区切りで並ぶか。
//!
//! このテーブルが持つのは分類と表示だけである。実行可能な各エントリは
//! [CommandId] であり、実行時には必ず
//! [App::execute_palette_command](crate::app::App::execute_palette_command) を
//! 経由する。これはコマンドパレットやキーボードショートカットと同じ経路であり、
//! コマンドの振る舞いがここに二重管理される箇所はない。メニュー項目を追加しても
//! コマンドの動作が変わることは絶対にない。
//!
//! 各項目の短い label があえてメニューローカルなのは理由がある。パレット側の
//! ラベルはパレットがフラットな一覧("Worktree: Create New")であるため
//! 自己説明的である必要があるが、メニュー内ではすでにトップレベルのタイトルが
//! その文脈を与えているので、同じことを繰り返すと "Worktree ▸ Worktree: Create
//! New" のように冗長になる。ローカルなのは表示文字列だけであり、実際に実行される
//! のは id である。
//!
//! tests::every_command_is_reachable が完全性の保証を担う。すべての
//! CommandId はいずれか1つのメニューに現れるか、理由付きで
//! INTENTIONALLY_UNLISTED に列挙されていなければならない。

use crate::command_palette::CommandId;

/// ドロップダウンの1行 — 実行可能なコマンド、または水平の区切り線。
pub enum MenuItem {
    /// 実行可能なコマンド。label はドロップダウンに表示するテキスト、id が
    /// 実行される対象。
    Command {
        id: CommandId,
        label: &'static str,
    },
    /// 関連コマンド群の間に置く、選択不可の区切り。
    Separator,
}

impl MenuItem {
    /// この行が実行するコマンド。[MenuItem::Separator] の場合は None。
    pub fn command(&self) -> Option<CommandId> {
        match self {
            MenuItem::Command { id, .. } => Some(*id),
            MenuItem::Separator => None,
        }
    }

    /// この行が選択対象になり得るか。区切りはキーボードナビゲーションで
    /// スキップされ、クリックもできない。
    pub fn is_selectable(&self) -> bool {
        matches!(self, MenuItem::Command { .. })
    }
}

/// 1つのトップレベルメニューと、それが開くドロップダウン。
pub struct Menu {
    /// メニューバー本体に表示される単語。
    pub title: &'static str,
    /// タイトルの前に置くアイコン。Nerd Font が無い環境では描かれない
    /// （メニューバーは横幅が厳しく、記号で埋めても意味が伝わらないため）。
    pub icon: crate::icons::Glyph,
    pub items: &'static [MenuItem],
}

/// コマンド行を作る簡易関数。
const fn cmd(id: CommandId, label: &'static str) -> MenuItem {
    MenuItem::Command { id, label }
}

/// 区切り行。
const SEP: MenuItem = MenuItem::Separator;

/// 意図的にメニューバーから外されているコマンドと、それぞれの理由。完全性を
/// 検証するテストがこのリストを参照するので、コマンドをメニューに追加せずに
/// ここのエントリだけ削除するとビルドが失敗する。
///
/// ここでカバーするのは [CommandId] のみである点に注意。キーマップにはパネル
/// ごとのカーソル移動(NavigateUp, GoToTop, NextHunk, …)も存在するが、
/// これらは CommandId を持たない Action であり、操作ではなくモーダルな
/// カーソル移動なのでメニュー行としては意味を持たない。同じ理由でパレットにも
/// 現れない。
// tests::every_command_is_reachable が読む。このリストはメニューが意図的に
// 省いているものの記録であり、テストがその記録を黙って古びさせないようにする。
// 読み手はそのテストだけなので cfg(test) に閉じてある。
#[cfg(test)]
pub const INTENTIONALLY_UNLISTED: &[(CommandId, &str)] = &[
    (
        CommandId::AddReviewComment,
        "コメントは Viewer で行を選んでから書くもので、メニューから始めても宛先が無い",
    ),
    (
        CommandId::EditComment,
        "対象は一覧で選択中のコメント。選択を持たないメニューからは指せない",
    ),
    (CommandId::ReplyToComment, "EditComment と同じ"),
    (CommandId::DeleteComment, "EditComment と同じ"),
    (CommandId::ToggleCommentResolve, "EditComment と同じ"),
    (CommandId::ViewCommentDetail, "EditComment と同じ"),
    (
        CommandId::ForceAnalyzeRevidere,
        "作り直しは Review current branch の確認ダイアログが兼ねる。こちらは確認を飛ばす近道",
    ),
];

/// メニューバーの並び (左から右)。
pub const MENUS: &[Menu] = &[
    Menu {
        title: "Repo",
        icon: crate::icons::MENU_REPO,
        items: &[
            cmd(CommandId::OpenRepo, "Open Repository…"),
            cmd(CommandId::SwitchRepo, "Switch Repository…"),
            SEP,
            cmd(CommandId::RefreshDiff, "Refresh Diff"),
            SEP,
            cmd(CommandId::Quit, "Quit Conductor"),
        ],
    },
    Menu {
        title: "Worktree",
        icon: crate::icons::MENU_WORKTREE,
        items: &[
            cmd(CommandId::CreateWorktree, "New Worktree…"),
            cmd(CommandId::DeleteWorktree, "Delete Worktree…"),
            SEP,
            cmd(CommandId::NextWorktree, "Next Worktree"),
            cmd(CommandId::PrevWorktree, "Previous Worktree"),
            SEP,
            cmd(CommandId::SwitchBranch, "Switch Branch (Remote)…"),
            cmd(CommandId::GrabBranch, "Grab Branch…"),
            cmd(CommandId::UngrabBranch, "Ungrab Branch"),
            SEP,
            cmd(CommandId::PullWorktree, "Pull (fast-forward)"),
            cmd(CommandId::MergeToMain, "Merge into Main"),
            cmd(CommandId::CherryPick, "Cherry-pick…"),
            cmd(CommandId::ResetMainToOrigin, "Reset Main to Origin"),
            SEP,
            cmd(CommandId::PruneWorktrees, "Prune Stale Worktrees"),
            cmd(CommandId::RefreshWorktrees, "Refresh Worktree List"),
            SEP,
            cmd(CommandId::OpenPullRequest, "Open Pull Request in Browser"),
        ],
    },
    // レビューを作る → 読む → コメントを書く → 公開する、の順。コメントは
    // レビューの中にあるものなので、レビュー側の行より下に置く。
    Menu {
        title: "Review",
        icon: crate::icons::MENU_REVIEW,
        items: &[
            // 作る口は 2 つだけ。どちらも同じ解析に続き、違うのは対象の
            // worktree をどこから持ってくるかだけ。
            cmd(CommandId::AnalyzeRevidere, "Review current branch"),
            cmd(CommandId::ReviewPullRequest, "Review Pull Request…"),
            SEP,
            // revidere の行は View 配下の他の表示切り替えとではなく Review 配下に
            // まとめている。レビューを読むこと自体がレビュー活動だから。
            cmd(CommandId::ShowRevidere, "Show Review"),
            SEP,
            cmd(CommandId::ShowReviewComments, "Show Comments"),
            cmd(CommandId::ShowReviewTemplates, "Show Templates"),
            SEP,
            cmd(CommandId::PublishReview, "Publish Comments to GitHub…"),
        ],
    },
    Menu {
        title: "View",
        icon: crate::icons::MENU_VIEW,
        items: &[
            cmd(CommandId::ShowDiffList, "Changed Files"),
            cmd(CommandId::ShowCommentList, "Comment List"),
            SEP,
            cmd(CommandId::ToggleMarkdownRender, "Markdown: Raw / Rendered"),
            SEP,
            cmd(CommandId::SwitchTheme, "Switch Theme…"),
            cmd(CommandId::ToggleHighContrast, "Toggle High Contrast"),
        ],
    },
    Menu {
        title: "Panel",
        icon: crate::icons::MENU_PANEL,
        items: &[
            cmd(CommandId::FocusWorktree, "Focus Worktree"),
            cmd(CommandId::FocusExplorer, "Focus Explorer"),
            cmd(CommandId::FocusViewer, "Focus Viewer"),
            cmd(CommandId::FocusTerminalClaude, "Focus Claude Code"),
            cmd(CommandId::FocusTerminalShell, "Focus Shell"),
            SEP,
            cmd(CommandId::TogglePanelExpand, "Maximize / Restore Panel"),
            SEP,
            cmd(CommandId::ResizePaneLeft, "Resize Pane Left"),
            cmd(CommandId::ResizePaneRight, "Resize Pane Right"),
            cmd(CommandId::ResizePaneUp, "Resize Pane Up"),
            cmd(CommandId::ResizePaneDown, "Resize Pane Down"),
        ],
    },
    Menu {
        title: "Search",
        icon: crate::icons::MENU_SEARCH,
        items: &[
            cmd(CommandId::SearchInFile, "Search in File…"),
            cmd(CommandId::SearchFullText, "Full-text Search (Grep)…"),
        ],
    },
    Menu {
        title: "Terminal",
        icon: crate::icons::MENU_TERMINAL,
        items: &[
            cmd(CommandId::NewClaudeCode, "New Claude Code Session"),
            cmd(CommandId::NewShell, "New Shell Session"),
            cmd(CommandId::ResumeClaudeSession, "Resume Claude Session…"),
            SEP,
            cmd(CommandId::SaveSessionHistory, "Save Terminal Output"),
            cmd(CommandId::SessionHistory, "Saved Terminal Output…"),
        ],
    },
    Menu {
        title: "Help",
        icon: crate::icons::MENU_HELP,
        items: &[
            cmd(CommandId::ToggleHelp, "Keyboard Shortcuts"),
            SEP,
            cmd(CommandId::CheckForUpdate, "Check for Updates"),
            cmd(CommandId::UpdateAndRestart, "Update and Restart"),
        ],
    },
];
