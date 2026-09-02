//! パネルやモーダルが外の世界に及ぼす影響。語彙は小さく保つ。

use std::path::PathBuf;

use conductor_svc::Services;

use crate::modal::Modal;
use crate::task::{Task, TaskResult};
use crate::workspace::{Focus, StatusLevel, StatusMessage, Workspace};

#[derive(Debug)]
pub enum Effect {
    OpenFile { path: PathBuf, line: Option<usize> },
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
            Effect::Spawn(task) => task.spawn(svc),
            Effect::Quit => ws.should_quit = true,
        }
    }
}
