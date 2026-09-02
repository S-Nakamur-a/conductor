//! 画面が動いている理由の唯一の定義。tick レートと再描画の判定の両方がこれを読む。

use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Idle,
    Active,
    Terminal,
}

pub fn liveness(ws: &Workspace, input_recent: bool) -> Liveness {
    if ws.focus.is_pty() {
        return Liveness::Terminal;
    }
    if input_recent || ws.chrome.status.is_some() {
        return Liveness::Active;
    }
    Liveness::Idle
}
