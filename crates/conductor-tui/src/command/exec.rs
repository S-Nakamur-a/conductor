//! コマンドの唯一の実行口と、使えるかどうかの唯一の述語。
//!
//! メニュー・パレット・キーマップはどれもここを通る。行き先が 1 本なので、
//! メニューに行を足してもコマンドの意味は増えない。

use conductor_core::keymap::Action;
use conductor_svc::pty::SessionKind;
use ratatui::layout::Rect;

use super::CommandId;
use crate::effect::Effect;
use crate::layout::Divider;
use crate::modal::revidere as revidere_modal;
use crate::modal::{
    Confirm, Modal, Prompt, branch, commits, grep, help, history, pr, repo, session, theme, update,
};
use crate::panels::explorer::BottomView;
use crate::panels::revidere as revidere_panel;
use crate::task::{Persist, Task};
use crate::workspace::{Focus, StatusLevel, Workspace};

/// 使えないコマンドは理由を持つ。メニューはこれで灰色にし、パレットは実行時に出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enabled {
    Yes,
    No(&'static str),
}

impl Enabled {
    pub fn is_yes(self) -> bool {
        self == Enabled::Yes
    }
}

/// 今この状態でコマンドが意味を持つか。
///
/// 既定は Yes。No を書くのは、コマンド自身が具体的な場所で既に拒んでいるときだけで、
/// 誤って No にすると全操作を並べるはずの UI から動く操作が黙って消える。
/// 逆に Yes を誤ってもコマンドがステータスで理由を返すだけで済む。
pub fn enabled(ws: &Workspace, id: CommandId) -> Enabled {
    let worktree = ws.panels.worktree.selected();
    match id {
        CommandId::SwitchRepo if ws.repo.known.len() < 2 => {
            Enabled::No("only one repository is known")
        }
        CommandId::DeleteWorktree if worktree.is_none_or(|w| w.is_main) => {
            Enabled::No("the main worktree cannot be deleted")
        }
        CommandId::MergeToMain if worktree.is_none_or(|w| w.is_main) => {
            Enabled::No("main cannot be merged into itself")
        }
        CommandId::OpenPullRequest if worktree.is_none() => Enabled::No("no worktree is selected"),
        CommandId::GrabBranch if ws.panels.worktree.grabbed().is_some() => {
            Enabled::No("a branch is already grabbed")
        }
        CommandId::UngrabBranch if ws.panels.worktree.grabbed().is_none() => {
            Enabled::No("no branch is grabbed")
        }
        CommandId::CherryPick if ws.panels.worktree.other_branches().is_empty() => {
            Enabled::No("no other worktree branch to pick from")
        }
        // 「PR のブランチか」は DB を引くので毎フレームは訊けない。それはコマンドが答える。
        CommandId::PublishReview if ws.review.unpublished_count() == 0 => {
            Enabled::No("no unpublished comments on this branch")
        }
        CommandId::UpdateAndRestart if ws.chrome.update.is_none() => {
            Enabled::No("no newer release is known \u{2014} check for updates first")
        }
        CommandId::FoldOneLevel
        | CommandId::UnfoldOneLevel
        | CommandId::FoldAll
        | CommandId::UnfoldAll
            if ws.panels.viewer.fold.max_depth().is_none() =>
        {
            Enabled::No("the open file has nothing to fold")
        }
        _ => Enabled::Yes,
    }
}

pub fn execute(ws: &mut Workspace, id: CommandId) -> Vec<Effect> {
    if let Enabled::No(reason) = enabled(ws, id) {
        return vec![Effect::Status(StatusLevel::Warning, reason.to_string())];
    }
    match id {
        CommandId::FocusWorktree => vec![Effect::Focus(Focus::Worktree)],
        CommandId::FocusExplorer => vec![Effect::Focus(Focus::Explorer)],
        CommandId::FocusViewer => vec![Effect::Focus(Focus::Viewer)],
        CommandId::FocusTerminalClaude => vec![Effect::Focus(Focus::TerminalClaude)],
        CommandId::FocusTerminalShell => vec![Effect::Focus(Focus::TerminalShell)],
        CommandId::NextWorktree => step_worktree(ws, 1),
        CommandId::PrevWorktree => step_worktree(ws, -1),
        CommandId::TogglePanelExpand => {
            ws.chrome.maximized = !ws.chrome.maximized;
            Vec::new()
        }
        CommandId::ResizePaneLeft => resize(ws, Resize::Left),
        CommandId::ResizePaneRight => resize(ws, Resize::Right),
        CommandId::ResizePaneUp => resize(ws, Resize::Up),
        CommandId::ResizePaneDown => resize(ws, Resize::Down),

        // 対象は選択中の worktree なので、フォーカスがどこでも同じ意味になる。
        CommandId::CreateWorktree => to_panel(ws, Focus::Worktree, Action::CreateWorktree),
        CommandId::DeleteWorktree => to_panel(ws, Focus::Worktree, Action::DeleteWorktree),
        CommandId::RefreshWorktrees => vec![Effect::Spawn(Task::ListWorktrees)],

        CommandId::NewClaudeCode => vec![Effect::NewSession(SessionKind::ClaudeCode)],
        CommandId::NewShell => vec![Effect::NewSession(SessionKind::Shell)],
        CommandId::ResumeClaudeSession => {
            let (picker, load) = session::ResumePicker::open();
            vec![Effect::PushModal(Modal::Resume(picker)), load]
        }
        CommandId::SaveSessionHistory => ws.panels.terminal.save_history(),
        CommandId::SessionHistory => {
            let (browser, load) = history::HistoryBrowser::open();
            vec![Effect::PushModal(Modal::History(browser)), load]
        }

        CommandId::RefreshDiff => ws.panels.explorer.refresh(),

        CommandId::SearchInFile => {
            let mut effects = to_panel(ws, Focus::Viewer, Action::SearchInFile);
            effects.push(Effect::Focus(Focus::Viewer));
            effects
        }
        CommandId::SearchFullText => vec![Effect::PushModal(Modal::Grep(grep::Grep::open()))],
        CommandId::FoldOneLevel => ws.panels.viewer.fold_chord('m'),
        CommandId::UnfoldOneLevel => ws.panels.viewer.fold_chord('r'),
        CommandId::FoldAll => ws.panels.viewer.fold_chord('M'),
        CommandId::UnfoldAll => ws.panels.viewer.fold_chord('R'),
        CommandId::ToggleHelp => vec![Effect::PushModal(Modal::Help(help::Help::open(ws.focus)))],
        CommandId::ShowDiffList => show_explorer(ws, BottomView::GitChanges),
        CommandId::ShowCommentList | CommandId::ShowReviewComments => {
            show_explorer(ws, BottomView::Comments)
        }
        CommandId::SwitchTheme => vec![Effect::PushModal(Modal::ThemePicker(
            theme::ThemePicker::open(&ws.appearance.name),
        ))],
        CommandId::ToggleHighContrast => toggle_high_contrast(ws),
        CommandId::ToggleMarkdownRender => {
            to_panel(ws, Focus::Viewer, Action::ToggleMarkdownRender)
        }

        // 対象は選択中の行。行を持たない場所から押しても宛先がないので、
        // フォーカス中のパネルにそのまま渡す。
        CommandId::AddReviewComment => to_focused(ws, Action::AddComment),
        CommandId::ViewCommentDetail => to_focused(ws, Action::ViewCommentDetail),
        CommandId::DeleteComment => to_focused(ws, Action::DeleteComment),
        CommandId::ToggleCommentResolve => to_focused(ws, Action::ToggleResolve),
        CommandId::EditComment => to_focused(ws, Action::EditComment),
        CommandId::ReplyToComment => to_focused(ws, Action::ReplyToComment),

        CommandId::OpenRepo => vec![Effect::PushModal(Modal::Prompt(Prompt::single(
            "Open repository (path)",
            |path| match path.trim() {
                "" => Vec::new(),
                path => vec![Effect::SwitchRepo(repo::expand_home(path))],
            },
        )))],
        CommandId::SwitchRepo => vec![Effect::PushModal(Modal::RepoPicker(
            repo::RepoPicker::open(ws.repo.known_index()),
        ))],
        CommandId::Quit => vec![Effect::Quit],

        CommandId::SwitchBranch => {
            let (picker, load) = branch::BranchPicker::remote();
            let mut effects = vec![Effect::PushModal(Modal::BranchPicker(picker))];
            effects.extend(load);
            effects
        }
        CommandId::GrabBranch => grab(ws),
        CommandId::UngrabBranch => confirm(
            ws.panels
                .worktree
                .grabbed()
                .map(|g| format!("Ungrab '{}'? Main returns to its own branch.", g.branch)),
            Task::Ungrab,
        ),
        CommandId::PruneWorktrees => vec![Effect::Spawn(Task::ListStaleWorktrees)],
        CommandId::CherryPick => cherry_pick(ws),
        CommandId::PullWorktree => {
            let worktree = ws.panels.worktree.selected();
            confirm(
                worktree.map(|w| format!("Pull '{}' (fast-forward only)?", w.branch)),
                Task::PullWorktree {
                    worktree: worktree.map(|w| w.path.clone()).unwrap_or_default(),
                },
            )
        }
        CommandId::MergeToMain => {
            let branch = ws.panels.worktree.selected().map(|w| w.branch.clone());
            confirm(
                branch
                    .as_ref()
                    .map(|b| format!("Merge '{b}' into '{}'?", ws.repo.main_branch)),
                Task::MergeToMain {
                    branch: branch.unwrap_or_default(),
                },
            )
        }
        // R は refresh の r の隣で、失うのはローカルのコミット。押し間違いで走らせない。
        CommandId::ResetMainToOrigin => confirm(
            Some(format!(
                "Reset '{}' to origin? Local commits on it are DISCARDED.",
                ws.repo.main_branch
            )),
            Task::ResetMainToOrigin,
        ),
        CommandId::OpenPullRequest => match ws.panels.worktree.selected() {
            Some(worktree) => vec![Effect::Spawn(Task::OpenPullRequest {
                branch: worktree.branch.clone(),
            })],
            None => Vec::new(),
        },
        CommandId::ReviewPullRequest => {
            vec![Effect::PushModal(Modal::PrInput(pr::PrInput::default()))]
        }
        CommandId::PublishReview => vec![Effect::Spawn(Task::LoadPublishable)],

        CommandId::CheckForUpdate => vec![
            Effect::Status(
                StatusLevel::Info,
                format!("Checking for updates\u{2026} (running v{})", ws.version),
            ),
            // 手で頼んだのだからキャッシュは見ない。
            Effect::Spawn(Task::CheckForUpdate {
                max_age: std::time::Duration::ZERO,
                announce: true,
            }),
        ],
        CommandId::UpdateAndRestart => match &ws.chrome.update {
            Some(info) => vec![Effect::PushModal(Modal::Update(update::Update::Confirm(
                Box::new(info.clone()),
            )))],
            None => Vec::new(),
        },

        // 索引はファイル単位で鮮度を持つので、別のツリーで作った索引は内容の違う
        // ファイルについてだけ答えなくなる。読むだけの worktree では編集が起きず
        // 引き金が引かれないので、手で頼む口が要る。
        CommandId::RebuildCodeIndex => {
            if ws.index.semantic.rebuild_reading() {
                vec![Effect::Status(
                    StatusLevel::Info,
                    "Rebuilding the code index for this file\u{2026}".into(),
                )]
            } else {
                vec![Effect::Status(
                    StatusLevel::Warning,
                    "no indexable file is open".into(),
                )]
            }
        }

        CommandId::ShowRevidere => show_revidere(ws),
        CommandId::AnalyzeRevidere => confirm_analyze(ws),
        // 押した人はもう一度作ると決めている。ヘルプも "without asking" と名乗る。
        CommandId::ForceAnalyzeRevidere => analyze(ws, true),
    }
}

/// 開いているなら閉じる。成果物が無ければ「無い」と言って終わるより、作る口を
/// その場で出したほうが早い。
fn show_revidere(ws: &mut Workspace) -> Vec<Effect> {
    if ws.focus == Focus::Revidere {
        return vec![Effect::Focus(Focus::Explorer)];
    }
    let panel = &ws.panels.revidere;
    if let Some(why) = panel.error() {
        return vec![Effect::Status(
            StatusLevel::Error,
            format!("Review artifact unreadable: {why}"),
        )];
    }
    if panel.review().is_none() {
        return confirm_analyze(ws);
    }
    // 成果物が同じでも作業ツリーが動いていれば読む順は変わっている。
    let worktree = ws.worktree_path();
    vec![
        ws.panels.revidere.reload(worktree),
        Effect::Focus(Focus::Revidere),
    ]
}

/// 解析の前に確認を出す。worktree が無いときや既に走っているときの断り方は解析側が
/// 持っているので、そこには確認を挟まずそのまま渡す。
fn confirm_analyze(ws: &Workspace) -> Vec<Effect> {
    let branch = ws.branch().to_string();
    if branch.is_empty() || ws.panels.revidere.is_running(&branch) {
        return analyze(ws, false);
    }
    let head = ws
        .panels
        .worktree
        .selected()
        .and_then(|w| w.head_oid.clone());
    let artifact = ws.panels.revidere.artifact(head.as_deref());
    // 同じコミットの成果物があるなら貯めた応答を捨てる。
    let on_yes = analyze(ws, artifact == revidere_panel::Artifact::Current);
    vec![Effect::PushModal(Modal::RevidereConfirm(
        revidere_modal::RevidereConfirm {
            branch,
            scope: revidere_panel::scope_label(ws.panels.revidere.scope()),
            artifact,
            on_yes,
        },
    ))]
}

/// 既定でキャッシュは効くので、diff が動いていなければ AI は起動せず即座に返る。
fn analyze(ws: &Workspace, force: bool) -> Vec<Effect> {
    let branch = ws.branch().to_string();
    if branch.is_empty() {
        return vec![Effect::Status(
            StatusLevel::Warning,
            "No worktree selected \u{2014} open one to analyse.".into(),
        )];
    }
    if ws.panels.revidere.is_running(&branch) {
        return vec![Effect::Status(
            StatusLevel::Warning,
            "revidere is already analysing this branch.".into(),
        )];
    }
    let scope = ws.panels.revidere.scope();
    vec![
        Effect::Spawn(Task::Analyze {
            worktree: ws.worktree_path(),
            branch,
            scope,
            force,
            api: ws.config.api.clone(),
            cancel: Default::default(),
        }),
        // どちらの区間かを言う。区間はビューを閉じても残るので、外から W を押すと
        // 数分待った先で思っていない方が出てくることがある。
        Effect::Status(
            StatusLevel::Info,
            format!(
                "Analysing [{}] with revidere \u{2014} this takes a few minutes.",
                revidere_panel::scope_label(scope)
            ),
        ),
    ]
}

/// 文言が作れないことが「対象が無い」の印。
fn confirm(question: Option<String>, on_yes: Task) -> Vec<Effect> {
    match question {
        Some(question) => vec![Effect::PushModal(Modal::Confirm(Confirm {
            question,
            on_yes: vec![Effect::Spawn(on_yes)],
        }))],
        None => vec![Effect::Status(
            StatusLevel::Warning,
            "no worktree selected".into(),
        )],
    }
}

fn grab(ws: &Workspace) -> Vec<Effect> {
    let sources = ws.panels.worktree.grab_sources();
    if sources.is_empty() {
        return vec![Effect::Status(
            StatusLevel::Warning,
            "no non-main worktrees to grab".into(),
        )];
    }
    vec![Effect::PushModal(Modal::BranchPicker(
        branch::BranchPicker::grab(sources),
    ))]
}

fn cherry_pick(ws: &Workspace) -> Vec<Effect> {
    let Some(target) = ws.panels.worktree.selected().map(|w| w.path.clone()) else {
        return Vec::new();
    };
    match commits::CherryPick::open(ws.panels.worktree.other_branches(), target) {
        Some((picker, load)) => vec![Effect::PushModal(Modal::CherryPick(picker)), load],
        None => Vec::new(),
    }
}

fn to_focused(ws: &mut Workspace, action: Action) -> Vec<Effect> {
    ws.dispatch(action).unwrap_or_default()
}

fn to_panel(ws: &mut Workspace, target: Focus, action: Action) -> Vec<Effect> {
    ws.dispatch_to(target, action).unwrap_or_default()
}

fn show_explorer(ws: &mut Workspace, view: BottomView) -> Vec<Effect> {
    ws.panels.explorer.show(view);
    vec![Effect::Focus(Focus::Explorer)]
}

fn step_worktree(ws: &Workspace, delta: isize) -> Vec<Effect> {
    let len = ws.panels.worktree.list().len();
    if len == 0 {
        return Vec::new();
    }
    let next = (ws.panels.worktree.selected_index() as isize + delta).rem_euclid(len as isize);
    vec![Effect::SelectWorktree(next as usize)]
}

fn toggle_high_contrast(ws: &mut Workspace) -> Vec<Effect> {
    let on = !ws.appearance.high_contrast;
    ws.appearance.high_contrast = on;
    ws.theme = ws.appearance.build();
    ws.config.ui.high_contrast = on;
    vec![
        Effect::Spawn(Task::PersistConfig(Persist::HighContrast(on))),
        Effect::Status(
            StatusLevel::Info,
            format!("high contrast {}", if on { "on" } else { "off" }),
        ),
    ]
}

/// tmux と同じ意味。その向きに隣がなければ反対側の境界が動いてパネルは縮む。
#[derive(Clone, Copy)]
enum Resize {
    Left,
    Right,
    Up,
    Down,
}

/// 1 回で動かす割合と、どの区画も消えないための上下限。
const STEP: i32 = 5;
const MIN_PCT: i32 = 10;
const SPLIT_MIN: i32 = 20;
const SPLIT_MAX: i32 = 80;

fn resize(ws: &mut Workspace, dir: Resize) -> Vec<Effect> {
    let step = if matches!(dir, Resize::Right | Resize::Down) {
        STEP
    } else {
        -STEP
    };
    let divider = match (dir, ws.focus) {
        (Resize::Left | Resize::Right, Focus::Worktree | Focus::Revidere) => return Vec::new(),
        // 最左の列が持つ境界は 1 本だけ。
        (Resize::Left | Resize::Right, Focus::Explorer) => Divider::ExplorerViewer,
        // 中央の列は向いた側の境界を押すので、縮むだけの窮屈な列にならない。
        (Resize::Right, Focus::Viewer) => Divider::ViewerTerminal,
        (Resize::Left, Focus::Viewer) => Divider::ExplorerViewer,
        // 最右の列は右キーで縮む。動くのは同じ境界で、広がる側が逆になるだけ。
        (Resize::Left | Resize::Right, _) => Divider::ViewerTerminal,
        // 上下の分割を持つのはターミナル列と Explorer 列。Down で上側が広がる。
        (_, Focus::TerminalClaude | Focus::TerminalShell) => Divider::TerminalSplit,
        (_, Focus::Explorer) => Divider::ExplorerSplit,
        _ => return Vec::new(),
    };
    if !move_divider(ws, divider, step) {
        return Vec::new();
    }
    vec![persist_layout(ws)]
}

/// ドラッグ中の境界をポインタへ寄せる。比率が実際に動いたときだけ true。
/// 永続化はしない — 動くたびに書くと、1 回のドラッグで何十回も config を書き直す。
pub fn drag_divider(ws: &mut Workspace, divider: Divider, main: Rect, x: u16, y: u16) -> bool {
    let pct = |pos: u16, origin: u16, extent: u16| -> i32 {
        if extent == 0 {
            return 0;
        }
        i32::from(pos.saturating_sub(origin)) * 100 / i32::from(extent)
    };
    let layout = &ws.config.layout;
    let target = match divider {
        Divider::ExplorerViewer => pct(x, main.x, main.width),
        // 境界は Explorer と Viewer の合計の右端にある。
        Divider::ViewerTerminal => {
            pct(x, main.x, main.width) - i32::from(layout.explorer_width_pct)
        }
        Divider::ExplorerSplit | Divider::TerminalSplit => pct(y, main.y, main.height),
    };
    let current = match divider {
        Divider::ExplorerViewer => layout.explorer_width_pct,
        Divider::ViewerTerminal => layout.viewer_width_pct,
        Divider::ExplorerSplit => layout.explorer_split_pct,
        Divider::TerminalSplit => layout.terminal_split_pct,
    };
    move_divider(ws, divider, target - i32::from(current))
}

pub fn persist_layout(ws: &Workspace) -> Effect {
    Effect::Spawn(Task::PersistConfig(Persist::Layout(Box::new(
        ws.config.layout.clone(),
    ))))
}

/// 境界を delta だけ右 (下) へ動かす。両側が下限を保てるときだけ動いて true。
///
/// terminal の幅は残りなので明示的には持たない。境界を動かすと勝手に増減する。
fn move_divider(ws: &mut Workspace, divider: Divider, delta: i32) -> bool {
    let layout = &mut ws.config.layout;
    let split = match divider {
        Divider::ExplorerSplit => Some(&mut layout.explorer_split_pct),
        Divider::TerminalSplit => Some(&mut layout.terminal_split_pct),
        _ => None,
    };
    if let Some(split) = split {
        let next = (i32::from(*split) + delta).clamp(SPLIT_MIN, SPLIT_MAX) as u16;
        let moved = next != *split;
        *split = next;
        return moved;
    }
    let (mut explorer, mut viewer) = (
        i32::from(layout.explorer_width_pct),
        i32::from(layout.viewer_width_pct),
    );
    match divider {
        Divider::ExplorerViewer => {
            explorer += delta;
            viewer -= delta;
        }
        _ => viewer += delta,
    }
    if explorer < MIN_PCT || viewer < MIN_PCT || 100 - explorer - viewer < MIN_PCT {
        return false;
    }
    let moved = delta != 0;
    layout.explorer_width_pct = explorer as u16;
    layout.viewer_width_pct = viewer as u16;
    moved
}
