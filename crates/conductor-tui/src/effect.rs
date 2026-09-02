//! パネルやモーダルが外の世界に及ぼす影響。語彙は小さく保つ。

use std::path::PathBuf;

use conductor_svc::Services;
use conductor_svc::pty::SessionKind;

use crate::modal::Modal;
use crate::task::{Task, TaskResult};
use crate::workspace::{Focus, StatusLevel, StatusMessage, Workspace};

#[derive(Debug)]
pub enum Effect {
    OpenFile { path: PathBuf, line: Option<usize> },
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
    for effect in effects {
        match effect {
            Effect::OpenFile { .. } => {}
            Effect::SelectWorktree(index) => select_worktree(ws, index),
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

/// 選択の移動は 2 つのパネルに跨がるので、パネルの update ではなくここが持つ。
fn select_worktree(ws: &mut Workspace, index: usize) {
    ws.panels.worktree.select(index);
    let worktree = ws.panels.worktree.selected().map(|w| w.path.clone());
    ws.panels.terminal.follow_worktree(worktree);
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
