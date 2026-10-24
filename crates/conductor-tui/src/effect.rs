//! パネルやモーダルが外の世界に及ぼす影響。語彙は小さく保つ。

use std::path::PathBuf;

use conductor_core::diff_state::OpenDiff;
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
        diff: Option<Box<OpenDiff>>,
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
    /// ツアーが見せている範囲。行を指すだけだと、その停留所がどこまでの話なのかが
    /// Viewer から消える。
    TourRange {
        path: String,
        start: u32,
        end: u32,
    },
    /// Explorer のツリーを開いて選択する。相対パスは Viewer の根から。
    RevealInTree(String),
    SearchInFile(String),
    /// レビュー済みの印。持ち主は Explorer。
    ToggleViewed(String),
    StepChangedFile(isize),
    SelectWorktree(usize),
    /// 一覧を開く。どのフォーカスからでも。
    OpenFleet,
    /// 一覧を開いていれば閉じて、そのフォーカスへ。人が押したフォーカスのキーの行き先。
    LeaveFleet(Focus),
    /// そのセッションの worktree を開いた今の画面に入る。
    EnterSession(crate::panels::fleet::RowKey),
    /// 一覧で選んだ 1 つを止める。[Effect::StopSessions] はこの窓のものをまとめて止める方。
    StopFleetSession(crate::panels::fleet::StopTarget),
    NewSession(SessionKind),
    NewClaudeWithChrome,
    /// 自由記述のタスクを Smart Worktree に渡す。Prompt の on_submit は関数
    /// ポインタで設定もリポジトリも掴めないので、Task を組むのは apply の側。
    SmartWorktree(String),
    /// 作ったばかりの worktree で Claude を起こし、入力を受けられるようになったら
    /// prompt を流し込む。今映しているものは動かさない。
    SmartSession {
        worktree: PathBuf,
        name: Option<String>,
        prompt: String,
    },
    /// シェルへ 1 行流して実行させる。テスト実行ボタンの行き先。
    SendToShell(String),
    /// Claude Code の入力欄へ送信せずに貼る。続きを打つのは人。
    PasteToClaude(String),
    /// 既存の Claude セッションを `--resume` で開き直す。`worktree` を指すと
    /// 選択とは別の場所で開く。grab はブランチを持ってきた main で開き直す。
    ResumeSession {
        id: String,
        worktree: Option<PathBuf>,
        chrome: bool,
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
    Play(crate::fx::Kind, crate::fx::Target),
    Spawn(Task),
    /// 端末のセッションを止める。worktree を渡せばそこのものだけ、無ければすべて。
    /// ホストはウィンドウより長生きするので、止めなければ裏で動き続ける。
    StopSessions(Option<PathBuf>),
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
            Effect::TourRange { path, start, end } => {
                ws.panels.viewer.set_tour_range(path, start, end);
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
            Effect::NewSession(kind) => new_session(ws, kind, None, None, false),
            Effect::NewClaudeWithChrome => {
                new_session(ws, SessionKind::ClaudeCode, None, None, true)
            }
            Effect::SmartWorktree(description) => {
                queue.push_back(Effect::Spawn(Task::SmartWorktree {
                    description,
                    api: ws.config.api.clone(),
                }));
                queue.push_back(Effect::Status(
                    StatusLevel::Info,
                    "Smart worktree: generating\u{2026}".into(),
                ));
            }
            Effect::SmartSession {
                worktree,
                name,
                prompt,
            } => queue.extend(smart_session(ws, &worktree, name.as_deref(), prompt)),
            Effect::SendToShell(command) => queue.extend(send_to_shell(ws, &command)),
            Effect::PasteToClaude(text) => {
                queue.push_back(if ws.panels.terminal.paste_to_claude(&text) {
                    Effect::Focus(Focus::TerminalClaude)
                } else {
                    Effect::Status(
                        StatusLevel::Warning,
                        "No Claude Code session to paste into.".into(),
                    )
                })
            }
            Effect::ResumeSession {
                id,
                worktree,
                chrome,
            } => new_session(ws, SessionKind::ClaudeCode, Some(&id), worktree, chrome),
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
            Effect::Focus(Focus::Fleet) => ws.focus = Focus::Fleet,
            // 映すものが無い右へは入らない。打った先が見えない。
            Effect::Focus(Focus::FleetSession) => {
                ws.focus = match ws.fleet_session() {
                    Some(_) => Focus::FleetSession,
                    None => Focus::Fleet,
                };
            }
            Effect::Focus(focus) => {
                ws.chrome.review_open =
                    focus == Focus::Revidere || (ws.reviewing() && focus == Focus::TerminalClaude);
                ws.set_screen_focus(focus);
            }
            Effect::OpenFleet => {
                ws.panels
                    .fleet
                    .open(ws.focus, ws.panels.terminal.own_claude_sessions());
                ws.chrome.fleet_open = true;
                ws.focus = Focus::Fleet;
            }
            Effect::LeaveFleet(focus) => {
                close_fleet(ws);
                queue.push_front(Effect::Focus(focus));
            }
            Effect::EnterSession(key) => queue.extend(enter_session(ws, svc, key)),
            Effect::StopFleetSession(target) => queue.extend(stop_fleet_session(ws, target)),
            Effect::Status(level, text) => {
                ws.chrome.status = Some(StatusMessage {
                    level,
                    text,
                    shown_at: std::time::Instant::now(),
                });
            }
            Effect::PushModal(modal) => ws.modals.push(modal),
            Effect::Play(kind, target) => ws.fx.play(kind, target),
            Effect::PopModal => {
                ws.modals.pop();
            }
            Effect::Spawn(task) => {
                ws.panels.fleet.note_spawned(&task);
                ws.panels.worktree.note_spawned(&task);
                ws.panels.revidere.note_spawned(&task);
                task.spawn(svc, &ws.task_env());
            }
            Effect::StopSessions(worktree) => ws.panels.terminal.stop_sessions(worktree.as_deref()),
            Effect::Quit => ws.should_quit = true,
        }
    }
}

/// 一覧を出る前の行き先へ戻してから閉じる。続くフォーカスの移動が、一覧の無いときと
/// 同じ画面から始まるように。
fn close_fleet(ws: &mut Workspace) {
    if ws.chrome.fleet_open {
        ws.focus = ws.panels.fleet.came_from();
        ws.chrome.fleet_open = false;
    }
}

/// 一覧から今の画面に入る。別の repo なら開き直し、worktree はその一覧が届いてから選ぶ。
fn enter_session(
    ws: &mut Workspace,
    svc: &mut Services<TaskResult>,
    key: crate::panels::fleet::RowKey,
) -> Vec<Effect> {
    let warn = |text: String| vec![Effect::Status(StatusLevel::Warning, text)];
    let crate::panels::fleet::RowKey::Panel(id) = key else {
        return warn("This Claude runs outside this window; it cannot be opened from here.".into());
    };
    let Some(dir) = ws.panels.terminal.session_dir(&id) else {
        return warn("That session has ended.".into());
    };
    if conductor_core::git_engine::GitEngine::open(&dir).is_err() {
        return warn(format!("{} is no longer a git worktree.", dir.display()));
    }
    if let Some(editor) = ws.panels.terminal.editor_name()
        && ws.worktree_path() != dir
    {
        return warn(format!("Close {editor} first."));
    }
    let listed = ws.panels.worktree.list().iter().position(|w| w.path == dir);
    // 一覧を閉じてから worktree の一覧を待つと、届くまでの打鍵が今の worktree の Claude に入る。
    if listed.is_none() && same_repo(&ws.repo.root, &dir) {
        return warn("The worktree list is still loading; try again in a moment.".into());
    }
    close_fleet(ws);
    let mut effects = match listed {
        Some(index) => select_worktree(ws, index),
        None => {
            // 別の repo へはレビュー画面を持ち込まない。
            ws.chrome.review_open = false;
            ws.focus = Focus::TerminalClaude;
            let effects = switch_repo(ws, svc, &dir);
            // switch_repo が worktree のパネルを作り直すので、待ちはその後に立てる。
            ws.panels.worktree.select_when_listed(dir);
            effects
        }
    };
    ws.panels.terminal.show_claude_when_selected(&id);
    effects.push(Effect::Focus(Focus::TerminalClaude));
    effects
}

/// 外のプロセスは照合と猶予で待つので裏で止める。
fn stop_fleet_session(ws: &mut Workspace, target: crate::panels::fleet::StopTarget) -> Vec<Effect> {
    use crate::panels::fleet::StopTarget;
    let id = match target {
        StopTarget::Process { pid, started } => {
            return vec![Effect::Spawn(Task::StopSession { pid, started })];
        }
        StopTarget::Panel(id) => id,
    };
    let was_running = ws.panels.terminal.stop_claude(&id);
    let own = ws.panels.terminal.own_claude_sessions();
    ws.panels.fleet.forget_stopped(&id, own);
    vec![if was_running {
        Effect::Status(StatusLevel::Success, "Stopped the session.".into())
    } else {
        Effect::Status(StatusLevel::Info, "That session had already ended.".into())
    }]
}

/// 今の repo が git でなければ別の repo とみなす。
fn same_repo(root: &std::path::Path, dir: &std::path::Path) -> bool {
    let main = |path: &std::path::Path| {
        conductor_core::git_engine::GitEngine::open(path)
            .and_then(|git| git.main_worktree_path())
            .ok()
            .and_then(|main| std::fs::canonicalize(main).ok())
    };
    main(root).is_some_and(|ours| main(dir) == Some(ours))
}

/// ファイルを開くのは Viewer だけの仕事だが、フォーカスの移動は跨ぐのでここが持つ。
fn open_file(
    ws: &mut Workspace,
    path: &std::path::Path,
    line: Option<usize>,
    diff: Option<Box<OpenDiff>>,
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
pub(crate) fn select_worktree(ws: &mut Workspace, index: usize) -> Vec<Effect> {
    let leaving = ws.worktree_path();
    ws.panels.worktree.select(index);
    let Some(worktree) = ws.panels.worktree.selected().map(|w| w.path.clone()) else {
        return Vec::new();
    };
    let review = swap_review(ws, leaving.clone(), &worktree);
    ws.panels.revidere.swap_scope(leaving, &worktree);
    ws.panels.terminal.follow_worktree(Some(worktree.clone()));
    let mut effects = ws.panels.viewer.set_root(worktree.clone());
    effects.extend(
        ws.panels
            .explorer
            .set_root(worktree.clone(), &ws.repo.main_branch),
    );
    // コメントはブランチで引くので、worktree が動いたら読み直す。
    effects.push(Effect::Spawn(Task::LoadReview));
    // 確認ダイアログが「作り直しか、初めてか」を答えられるよう、成果物も先に読む。
    // UI スレッドでファイルを読まないので、訊かれてから調べることはできない。
    effects.push(ws.panels.revidere.reload(worktree));
    effects.extend(review);
    effects
}

/// レビュー画面を開いているかは worktree ごとに持ち、引き継がない。
fn swap_review(ws: &mut Workspace, leaving: PathBuf, entering: &std::path::Path) -> Vec<Effect> {
    if leaving == entering {
        return Vec::new();
    }
    if ws.reviewing() {
        ws.chrome.review_open_in.insert(leaving, ws.screen_focus());
    }
    match ws.chrome.review_open_in.remove(entering) {
        // Claude へのフォーカスがレビュー画面の一部になるのは、画面が開いている間だけ。
        Some(focus) => vec![Effect::Focus(Focus::Revidere), Effect::Focus(focus)],
        None if ws.reviewing() => vec![Effect::Focus(Focus::Explorer)],
        None => Vec::new(),
    }
}

/// 見えているシェルへ 1 行送る。走っていなければ起こしてから送る — 押した人は
/// テストを走らせたいのであって、先にシェルを開けと言われたいわけではない。
fn send_to_shell(ws: &mut Workspace, command: &str) -> Vec<Effect> {
    if !ws.panels.terminal.has_shell() {
        new_session(ws, SessionKind::Shell, None, None, false);
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

fn smart_session(
    ws: &mut Workspace,
    worktree: &std::path::Path,
    name: Option<&str>,
    prompt: String,
) -> Vec<Effect> {
    let repo_root = ws.repo.root.clone();
    match ws
        .panels
        .terminal
        .launch_claude_at(worktree, &repo_root, &ws.config, name, prompt)
    {
        Ok(()) => Vec::new(),
        Err(e) => vec![Effect::Status(
            StatusLevel::Warning,
            format!("worktree is ready but Claude did not start: {e:#}"),
        )],
    }
}

fn new_session(
    ws: &mut Workspace,
    kind: SessionKind,
    resume: Option<&str>,
    at: Option<PathBuf>,
    chrome: bool,
) {
    let worktree = at.unwrap_or_else(|| {
        ws.panels
            .worktree
            .selected()
            .map_or_else(|| ws.repo.root.clone(), |w| w.path.clone())
    });
    let result =
        ws.panels
            .terminal
            .spawn(kind, resume, &worktree, &ws.repo.root, &ws.config, chrome);
    match result {
        Ok(()) => ws.set_screen_focus(match kind {
            SessionKind::Shell => Focus::TerminalShell,
            _ => Focus::TerminalClaude,
        }),
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
    let opened = match RepoState::open(&root) {
        Ok(repo) => repo,
        Err(e) => {
            return vec![Effect::Status(
                StatusLevel::Error,
                format!("not a git repository: {} ({e})", root.display()),
            )];
        }
    };
    // 索引が鍵を結び付けているのは Viewer の根であって、リポジトリの根ではない。
    // worktree を見ている最中に同じリポジトリを選び直すと、根は動く。
    let moving = ws.panels.viewer.root() != root;
    svc.bump_generation();
    ws.index.forget_in_flight();
    if moving {
        let repo = ws.repo.root.clone();
        ws.index.semantic.abort_regeneration(&repo);
    }
    let stopped = ws.panels.revidere.abort();
    ws.panels.fleet.forget_in_flight();
    let known = std::mem::take(&mut ws.repo.known);
    let name = opened.name.clone();
    ws.repo = RepoState { known, ..opened };
    ws.repo.remember(&root);
    ws.review = Default::default();
    ws.panels.worktree = Default::default();
    ws.panels.terminal.follow_worktree(None);
    let mut effects = ws.panels.viewer.set_root(root.clone());
    effects.extend(ws.panels.explorer.set_root(root, &ws.repo.main_branch));
    effects.push(Effect::Spawn(Task::ListWorktrees));
    effects.push(Effect::Spawn(Task::LoadGrabState));
    // 止めた解析は畳んで伝える。ステータスは 1 枠しかないので、別に出しても後勝ちで消える。
    let mut message = format!("switched to {name}");
    if stopped > 0 {
        let unit = if stopped == 1 { "review" } else { "reviews" };
        message.push_str(&format!(" (cancelled {stopped} running {unit})"));
    }
    effects.push(Effect::Status(StatusLevel::Success, message));
    effects
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::panels::fleet::StopTarget;

    #[test]
    fn 外のプロセスを止める指示は一覧に載せたときの起動時刻のまま裏の仕事になる() {
        let mut ws = Workspace::for_test();
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        let effects = stop_fleet_session(&mut ws, StopTarget::Process { pid: 9, started });
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Spawn(Task::StopSession { pid: 9, started: s })] if *s == started
            ),
            "{effects:?}"
        );
    }
}
