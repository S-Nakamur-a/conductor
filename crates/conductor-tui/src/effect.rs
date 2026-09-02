//! パネルやモーダルが外の世界に及ぼす影響。語彙は小さく保つ。

use std::path::PathBuf;

use conductor_core::diff_state::FileDiff;
use conductor_svc::Services;
use conductor_svc::pty::SessionKind;

use crate::modal::Modal;
use crate::task::{Task, TaskResult};
use crate::workspace::{Focus, StatusLevel, StatusMessage, Workspace};

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
    Focus(Focus),
    Status(StatusLevel, String),
    PushModal(Modal),
    PopModal,
    Spawn(Task),
    Quit,
}

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
            Effect::ToggleViewed(path) => ws.panels.explorer.toggle_viewed(&path),
            Effect::StepChangedFile(delta) => {
                if let Some(effect) = ws.panels.explorer.step_changed_file(delta) {
                    queue.push_back(effect);
                }
            }
            Effect::SelectWorktree(index) => queue.extend(select_worktree(ws, index)),
            Effect::NewSession(kind) => new_session(ws, kind),
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
    effects
}

fn new_session(ws: &mut Workspace, kind: SessionKind) {
    let worktree = ws
        .panels
        .worktree
        .selected()
        .map_or_else(|| ws.repo.root.clone(), |w| w.path.clone());
    let result = ws
        .panels
        .terminal
        .spawn(kind, &worktree, &ws.repo.root, &ws.config);
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
