//! コマンドの唯一の実行口と、使えるかどうかの唯一の述語。
//!
//! メニュー・パレット・キーマップはどれもここを通る。行き先が 1 本なので、
//! メニューに行を足してもコマンドの意味は増えない。

use conductor_core::keymap::Action;
use conductor_svc::pty::SessionKind;

use super::CommandId;
use crate::effect::Effect;
use crate::modal::{Modal, Prompt, grep, help, history, repo, session, theme};
use crate::panels::explorer::BottomView;
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

/// まだ実装していない操作。到達できることだけ先に固定してある。
pub(super) const NOT_YET: &[(CommandId, &str)] = &[
    (CommandId::SwitchBranch, "switching branches"),
    (CommandId::GrabBranch, "grabbing a branch"),
    (CommandId::UngrabBranch, "ungrabbing a branch"),
    (CommandId::PruneWorktrees, "pruning worktrees"),
    (CommandId::MergeToMain, "merging into main"),
    (CommandId::ResetMainToOrigin, "resetting main"),
    (CommandId::CherryPick, "cherry-picking"),
    (CommandId::PullWorktree, "pulling a worktree"),
    (CommandId::OpenPullRequest, "opening a pull request"),
    (CommandId::ReviewPullRequest, "reviewing a pull request"),
    (CommandId::PublishReview, "publishing comments"),
    (CommandId::ShowRevidere, "the review view"),
    (CommandId::AnalyzeRevidere, "generating a review"),
    (CommandId::ForceAnalyzeRevidere, "generating a review"),
    (CommandId::ShowReviewTemplates, "review templates"),
    (CommandId::ToggleMarkdownRender, "rendered markdown"),
    (CommandId::RebuildCodeIndex, "the code index"),
    (CommandId::CheckForUpdate, "checking for updates"),
    (CommandId::UpdateAndRestart, "updating and restarting"),
];

fn not_yet(id: CommandId) -> Option<&'static str> {
    NOT_YET
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, what)| *what)
}

/// 今この状態でコマンドが意味を持つか。
///
/// 既定は Yes。No を書くのは、コマンド自身が具体的な場所で既に拒んでいるときだけで、
/// 誤って No にすると全操作を並べるはずの UI から動く操作が黙って消える。
/// 逆に Yes を誤ってもコマンドがステータスで理由を返すだけで済む。
pub fn enabled(ws: &Workspace, id: CommandId) -> Enabled {
    if not_yet(id).is_some() {
        return Enabled::No("not implemented yet");
    }
    let worktree = ws.panels.worktree.selected();
    match id {
        CommandId::SwitchRepo if ws.repo.known.len() < 2 => {
            Enabled::No("only one repository is known")
        }
        CommandId::DeleteWorktree if worktree.is_none_or(|w| w.is_main) => {
            Enabled::No("the main worktree cannot be deleted")
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
        let what = not_yet(id)
            .map(|what| format!("{what} is not implemented yet"))
            .unwrap_or_else(|| reason.to_string());
        return vec![Effect::Status(StatusLevel::Warning, what)];
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
        CommandId::ShowDiffList => show_explorer(ws, BottomView::Changes),
        CommandId::ShowCommentList | CommandId::ShowReviewComments => {
            show_explorer(ws, BottomView::Comments)
        }
        CommandId::SwitchTheme => vec![Effect::PushModal(Modal::ThemePicker(
            theme::ThemePicker::open(&ws.appearance.name),
        ))],
        CommandId::ToggleHighContrast => toggle_high_contrast(ws),

        // 対象は選択中の行。行を持たない場所から押しても宛先がないので、
        // フォーカス中のパネルにそのまま渡す。
        CommandId::AddReviewComment => to_focused(ws, Action::AddComment),
        CommandId::ViewCommentDetail => to_focused(ws, Action::ViewCommentDetail),
        CommandId::DeleteComment => to_focused(ws, Action::DeleteComment),
        CommandId::ToggleCommentResolve => to_focused(ws, Action::ToggleResolve),
        CommandId::EditComment => to_focused(ws, Action::EditComment),
        CommandId::ReplyToComment => to_focused(ws, Action::ReplyToComment),

        CommandId::OpenRepo => vec![Effect::PushModal(Modal::Prompt(Prompt {
            title: "Open repository (path)".into(),
            input: Default::default(),
            on_submit: |path| match path.trim() {
                "" => Vec::new(),
                path => vec![Effect::SwitchRepo(repo::expand_home(path))],
            },
        }))],
        CommandId::SwitchRepo => vec![Effect::PushModal(Modal::RepoPicker(
            repo::RepoPicker::open(ws.repo.known_index()),
        ))],
        CommandId::Quit => vec![Effect::Quit],

        // 未実装。enabled が先に弾くのでここには来ない。`_` で受けないのは、
        // コマンドを足したときに実装漏れを型で見つけるため。
        CommandId::SwitchBranch
        | CommandId::GrabBranch
        | CommandId::UngrabBranch
        | CommandId::PruneWorktrees
        | CommandId::MergeToMain
        | CommandId::ResetMainToOrigin
        | CommandId::CherryPick
        | CommandId::PullWorktree
        | CommandId::OpenPullRequest
        | CommandId::ReviewPullRequest
        | CommandId::PublishReview
        | CommandId::ShowRevidere
        | CommandId::AnalyzeRevidere
        | CommandId::ForceAnalyzeRevidere
        | CommandId::ShowReviewTemplates
        | CommandId::ToggleMarkdownRender
        | CommandId::RebuildCodeIndex
        | CommandId::CheckForUpdate
        | CommandId::UpdateAndRestart => Vec::new(),
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

/// 動かせる境界。3 列の幅は 2 本の縦境界で決まる。
#[derive(Clone, Copy)]
enum Divider {
    ExplorerViewer,
    ViewerTerminal,
}

/// 1 回で動かす割合と、どの区画も消えないための上下限。
const STEP: i32 = 5;
const MIN_PCT: i32 = 10;
const SPLIT_MIN: i32 = 20;
const SPLIT_MAX: i32 = 80;

fn resize(ws: &mut Workspace, dir: Resize) -> Vec<Effect> {
    let right = matches!(dir, Resize::Right);
    let moved = match (dir, ws.focus) {
        (Resize::Left | Resize::Right, Focus::Worktree | Focus::Revidere) => false,
        // 最左の列が持つ境界は 1 本だけ。
        (Resize::Left | Resize::Right, Focus::Explorer) => {
            move_divider(ws, Divider::ExplorerViewer, right)
        }
        // 中央の列は向いた側の境界を押すので、縮むだけの窮屈な列にならない。
        (Resize::Left | Resize::Right, Focus::Viewer) => move_divider(
            ws,
            if right {
                Divider::ViewerTerminal
            } else {
                Divider::ExplorerViewer
            },
            right,
        ),
        // 最右の列は右キーで縮む。動くのは同じ境界で、広がる側が逆になるだけ。
        (Resize::Left | Resize::Right, _) => move_divider(ws, Divider::ViewerTerminal, right),
        // 上下の分割を持つのはターミナル列と Explorer 列。Down で上側が広がる。
        (dir, focus) => {
            let step = if matches!(dir, Resize::Down) {
                STEP
            } else {
                -STEP
            };
            let split = match focus {
                Focus::TerminalClaude | Focus::TerminalShell => {
                    &mut ws.config.layout.terminal_split_pct
                }
                Focus::Explorer => &mut ws.config.layout.explorer_split_pct,
                _ => return Vec::new(),
            };
            let next = (i32::from(*split) + step).clamp(SPLIT_MIN, SPLIT_MAX) as u16;
            let moved = next != *split;
            *split = next;
            moved
        }
    };
    if !moved {
        return Vec::new();
    }
    vec![Effect::Spawn(Task::PersistConfig(Persist::Layout(
        Box::new(ws.config.layout.clone()),
    )))]
}

/// 境界を右へ (`right`) か左へ動かす。両側が下限を保てるときだけ動いて true。
///
/// terminal の幅は残りなので明示的には持たない。境界を動かすと勝手に増減する。
fn move_divider(ws: &mut Workspace, divider: Divider, right: bool) -> bool {
    let layout = &mut ws.config.layout;
    let (mut explorer, mut viewer) = (
        i32::from(layout.explorer_width_pct),
        i32::from(layout.viewer_width_pct),
    );
    let step = if right { STEP } else { -STEP };
    match divider {
        Divider::ExplorerViewer => {
            explorer += step;
            viewer -= step;
        }
        Divider::ViewerTerminal => viewer += step,
    }
    if explorer < MIN_PCT || viewer < MIN_PCT || 100 - explorer - viewer < MIN_PCT {
        return false;
    }
    layout.explorer_width_pct = explorer as u16;
    layout.viewer_width_pct = viewer as u16;
    true
}
