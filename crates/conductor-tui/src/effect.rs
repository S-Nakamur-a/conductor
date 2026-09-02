//! パネルやモーダルが外の世界に及ぼす影響。語彙は小さく保つ。

use std::path::PathBuf;

use conductor_core::diff_state::FileDiff;
use conductor_svc::Services;
use conductor_svc::pty::SessionKind;

use crate::command::CommandId;
use crate::modal::Modal;
use crate::task::{Task, TaskResult};
use crate::workspace::{Focus, RepoState, StatusLevel, StatusMessage, Workspace};

#[derive(Debug)]
pub enum Effect {
    /// 相対パスは Viewer の根から解決する。`diff` があれば素の本文ではなく差分として開く。
    OpenFile {
        path: PathBuf,
        line: Option<usize>,
        diff: Option<Box<FileDiff>>,
        preview: bool,
    },
    FindFile(String),
    SearchInFile(String),
    /// レビュー済みの印。持ち主は Explorer。
    ToggleViewed(String),
    StepChangedFile(isize),
    SelectWorktree(usize),
    NewSession(SessionKind),
    /// 既存の Claude セッションを `--resume` で開き直す。`worktree` を指すと
    /// 選択とは別の場所で開く。grab はブランチを持ってきた main で開き直す。
    ResumeSession {
        id: String,
        worktree: Option<PathBuf>,
    },
    Command(CommandId),
    /// メニューバーにキーボードフォーカスを移す。開くのはそこから。
    FocusMenuBar,
    /// リポジトリを開き直す。既知の一覧に無ければ足す。
    SwitchRepo(PathBuf),
    /// `persist` ならライブプレビューではなく確定として設定に書く。
    SetTheme {
        name: String,
        persist: bool,
    },
    Focus(Focus),
    Status(StatusLevel, String),
    PushModal(Modal),
    PopModal,
    Spawn(Task),
    Quit,
}

/// テストで種類だけを見るための等価。中身が主張の一部なら分解して assert する。
impl PartialEq for Effect {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}
impl Eq for Effect {}

/// Effect を Workspace と svc に反映する唯一の場所。
pub fn apply(ws: &mut Workspace, svc: &mut Services<TaskResult>, effects: Vec<Effect>) {
    let mut queue = std::collections::VecDeque::from(effects);
    while let Some(effect) = queue.pop_front() {
        match effect {
            Effect::OpenFile {
                path,
                line,
                diff,
                preview,
            } => {
                let follow_up = open_file(ws, &path, line, diff, preview);
                queue.extend(follow_up);
            }
            Effect::FindFile(query) => {
                if let Some(effect) = ws.panels.explorer.find_file(&query) {
                    queue.push_back(effect);
                }
            }
            Effect::SearchInFile(query) => {
                let follow_up = ws.panels.viewer.search_for(&query);
                queue.extend(follow_up);
            }
            Effect::ToggleViewed(path) => {
                let write = crate::task::ReviewWrite::SetViewed {
                    viewed: !ws.review.is_viewed(&path),
                    file_path: path,
                };
                queue.push_back(Effect::Spawn(Task::WriteReview(write)));
            }
            Effect::StepChangedFile(delta) => {
                if let Some(effect) = ws.panels.explorer.step_changed_file(delta) {
                    queue.push_back(effect);
                }
            }
            Effect::SelectWorktree(index) => queue.extend(select_worktree(ws, index)),
            Effect::NewSession(kind) => new_session(ws, kind, None, None),
            Effect::ResumeSession { id, worktree } => {
                new_session(ws, SessionKind::ClaudeCode, Some(&id), worktree)
            }
            Effect::Command(id) => queue.extend(crate::command::execute(ws, id)),
            Effect::FocusMenuBar => ws.chrome.menu = crate::menu::MenuBar::Bar { index: 0 },
            Effect::SwitchRepo(path) => queue.extend(switch_repo(ws, svc, &path)),
            Effect::SetTheme { name, persist } => {
                set_theme(ws, &name);
                if persist {
                    queue.push_back(Effect::Spawn(Task::PersistConfig(
                        crate::task::Persist::Theme(name),
                    )));
                }
            }
            Effect::Focus(focus) => ws.focus = focus,
            Effect::Status(level, text) => {
                ws.chrome.status = Some(StatusMessage {
                    level,
                    text,
                    shown_at: std::time::Instant::now(),
                });
            }
            Effect::PushModal(modal) => ws.modals.push(modal),
            Effect::PopModal => {
                ws.modals.pop();
            }
            Effect::Spawn(task) => {
                ws.panels.worktree.note_spawned(&task);
                task.spawn(svc, &ws.task_env());
            }
            Effect::Quit => ws.should_quit = true,
        }
    }
}

/// ファイルを開くのは Viewer だけの仕事だが、フォーカスの移動は跨ぐのでここが持つ。
fn open_file(
    ws: &mut Workspace,
    path: &std::path::Path,
    line: Option<usize>,
    diff: Option<Box<FileDiff>>,
    preview: bool,
) -> Vec<Effect> {
    let mut effects = ws.panels.viewer.open(path, line, diff, preview);
    // preview はクリックで開いた下見なので、キーボードは Explorer に残す。
    if !preview {
        effects.push(Effect::Focus(Focus::Viewer));
    }
    effects
}

/// 選択の移動は 3 つのパネルに跨がるので、パネルの update ではなくここが持つ。
///
/// 根は Explorer と Viewer が別々に持つ。相対パスの解決先は Viewer なので、
/// ツリーだけが新しい根に切り替わる瞬間を作らないよう同じ場所で書く。
fn select_worktree(ws: &mut Workspace, index: usize) -> Vec<Effect> {
    ws.panels.worktree.select(index);
    let Some(worktree) = ws.panels.worktree.selected().map(|w| w.path.clone()) else {
        return Vec::new();
    };
    ws.panels.terminal.follow_worktree(Some(worktree.clone()));
    let mut effects = ws.panels.viewer.set_root(worktree.clone());
    effects.extend(ws.panels.explorer.set_root(worktree));
    // コメントはブランチで引くので、worktree が動いたら読み直す。
    effects.push(Effect::Spawn(Task::LoadReview));
    effects
}

fn new_session(ws: &mut Workspace, kind: SessionKind, resume: Option<&str>, at: Option<PathBuf>) {
    let worktree = at.unwrap_or_else(|| {
        ws.panels
            .worktree
            .selected()
            .map_or_else(|| ws.repo.root.clone(), |w| w.path.clone())
    });
    let result = ws
        .panels
        .terminal
        .spawn(kind, resume, &worktree, &ws.repo.root, &ws.config);
    match result {
        Ok(()) => {
            ws.focus = match kind {
                SessionKind::Shell => Focus::TerminalShell,
                _ => Focus::TerminalClaude,
            }
        }
        Err(e) => {
            ws.chrome.status = Some(StatusMessage {
                level: StatusLevel::Error,
                text: format!("{e:#}"),
                shown_at: std::time::Instant::now(),
            })
        }
    }
}

/// テーマを差し替える。syntect 側は設定から毎フレーム引き直すので、ここは
/// `ui.theme` を書けば足りる。
fn set_theme(ws: &mut Workspace, name: &str) {
    ws.appearance.name = name.to_string();
    ws.theme = ws.appearance.build();
    ws.config.ui.theme = Some(name.to_string());
}

/// 別のリポジトリを開き直す。PTY は殺さない — 走っているセッションは元の
/// worktree で仕事を続けているので、画面を切り替えただけで落とすのは行き過ぎ。
fn switch_repo(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    path: &std::path::Path,
) -> Vec<Effect> {
    if !path.is_dir() {
        return vec![Effect::Status(
            StatusLevel::Error,
            format!("not a directory: {}", path.display()),
        )];
    }
    let root = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let name = match RepoState::open(&root, &ws.config.general.main_branch) {
        Ok(repo) => repo.name.clone(),
        Err(e) => {
            return vec![Effect::Status(
                StatusLevel::Error,
                format!("not a git repository: {} ({e})", root.display()),
            )];
        }
    };
    // 世代を進めてから作り直す。飛んでいる Task の結果が新しいツリーに着地しない。
    svc.bump_generation();
    let known = std::mem::take(&mut ws.repo.known);
    ws.repo = RepoState {
        root: root.clone(),
        name: name.clone(),
        main_branch: ws.config.general.main_branch.clone(),
        known,
    };
    ws.repo.remember(&root);
    ws.review = Default::default();
    ws.panels.worktree = Default::default();
    ws.panels.terminal.follow_worktree(None);
    let mut effects = ws.panels.viewer.set_root(root.clone());
    effects.extend(ws.panels.explorer.set_root(root));
    effects.push(Effect::Spawn(Task::ListWorktrees));
    effects.push(Effect::Spawn(Task::LoadGrabState));
    effects.push(Effect::Status(
        StatusLevel::Success,
        format!("switched to {name}"),
    ));
    effects
}
