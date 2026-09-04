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
    /// 戻り先を積んでからファイルを開く。ジャンプの出どころが Viewer の外にもあるので、
    /// 積む場所を 1 つにするために [Effect::OpenFile] と分けてある。
    JumpTo {
        path: PathBuf,
        line: usize,
    },
    FindFile(String),
    /// 変更ファイルとして開く (差分表示のまま)。読む順のビューから Viewer へ渡す唯一の口。
    OpenChangedFile {
        path: String,
        line: Option<usize>,
    },
    /// Explorer のツリーを開いて選択する。相対パスは Viewer の根から。
    RevealInTree(String),
    SearchInFile(String),
    /// レビュー済みの印。持ち主は Explorer。
    ToggleViewed(String),
    StepChangedFile(isize),
    SelectWorktree(usize),
    NewSession(SessionKind),
    /// シェルへ 1 行流して実行させる。テスト実行ボタンの行き先。
    SendToShell(String),
    /// 既存の Claude セッションを `--resume` で開き直す。`worktree` を指すと
    /// 選択とは別の場所で開く。grab はブランチを持ってきた main で開き直す。
    ResumeSession {
        id: String,
        worktree: Option<PathBuf>,
    },
    /// 開いているファイルを埋め込みエディタで開く。パスは絶対。
    OpenInEditor(PathBuf),
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
            Effect::JumpTo { path, line } => {
                ws.panels.viewer.note_jump_from();
                queue.push_back(Effect::OpenFile {
                    path,
                    line: Some(line),
                    diff: None,
                    preview: false,
                });
            }
            Effect::FindFile(query) => {
                if let Some(effect) = ws.panels.explorer.find_file(&query) {
                    queue.push_back(effect);
                }
            }
            Effect::OpenChangedFile { path, line } => {
                queue.push_back(match ws.panels.explorer.open_changed(&path, line) {
                    Some(effect) => effect,
                    None => Effect::Status(
                        StatusLevel::Warning,
                        format!("Section's file isn't in this diff: {path}"),
                    ),
                });
            }
            Effect::RevealInTree(path) => ws.panels.explorer.reveal_in_tree(&path),
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
            Effect::SendToShell(command) => queue.extend(send_to_shell(ws, &command)),
            Effect::ResumeSession { id, worktree } => {
                new_session(ws, SessionKind::ClaudeCode, Some(&id), worktree)
            }
            Effect::OpenInEditor(path) => queue.extend(open_in_editor(ws, &path)),
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
                ws.panels.revidere.note_spawned(&task);
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
    effects.extend(ws.panels.explorer.set_root(worktree.clone()));
    // コメントはブランチで引くので、worktree が動いたら読み直す。
    effects.push(Effect::Spawn(Task::LoadReview));
    // 確認ダイアログが「作り直しか、初めてか」を答えられるよう、成果物も先に読む。
    // UI スレッドでファイルを読まないので、訊かれてから調べることはできない。
    effects.push(ws.panels.revidere.reload(worktree));
    effects
}

/// 見えているシェルへ 1 行送る。走っていなければ起こしてから送る — 押した人は
/// テストを走らせたいのであって、先にシェルを開けと言われたいわけではない。
fn send_to_shell(ws: &mut Workspace, command: &str) -> Vec<Effect> {
    if !ws.panels.terminal.has_shell() {
        new_session(ws, SessionKind::Shell, None, None);
    }
    match ws.panels.terminal.send_line(command) {
        Ok(()) => vec![Effect::Focus(Focus::TerminalShell)],
        Err(e) => vec![Effect::Status(StatusLevel::Error, format!("{e:#}"))],
    }
}

/// $EDITOR を PTY で起こし、Explorer と Viewer を併合した区画に映す。
fn open_in_editor(ws: &mut Workspace, path: &std::path::Path) -> Vec<Effect> {
    let worktree = ws
        .panels
        .worktree
        .selected()
        .map_or_else(|| ws.repo.root.clone(), |w| w.path.clone());
    let argv = crate::panels::terminal::editor_argv();
    match ws.panels.terminal.open_editor(path, &worktree, &argv) {
        Ok(name) => vec![
            Effect::Focus(Focus::Editor),
            Effect::Status(
                StatusLevel::Info,
                format!("editing {name} \u{2014} :q to close"),
            ),
        ],
        Err(e) => vec![Effect::Status(
            StatusLevel::Error,
            format!("could not launch the editor: {e:#}"),
        )],
    }
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
