//! テストから svc の往復を待つための足回り。

use std::path::{Path, PathBuf};
use std::time::Duration;

use conductor_core::git_engine::WorktreeInfo;
use conductor_svc::{EventKind, Services};

use crate::effect::apply;
use crate::task::TaskResult;
use crate::workspace::Workspace;

/// 何も届かなくなるまで svc の結果を消費する。
///
/// ワーカーは本物のスレッドなので、届く順も時刻も決めうちにしない。静かになってから
/// 少し待つのは、1 つの結果が次の Task を生む経路 (worktree 選択 → 走査) があるため。
pub fn pump(ws: &mut Workspace, svc: &mut Services<TaskResult>) {
    let mut quiet = 0;
    for _ in 0..500 {
        let mut got = false;
        while let Some(event) = svc.try_recv() {
            got = true;
            let effects = match event.kind {
                EventKind::Task(result) => ws.accept(result),
                EventKind::Watch(_) => Vec::new(),
            };
            apply(ws, svc, effects);
        }
        quiet = if got { 0 } else { quiet + 1 };
        if quiet > 20 {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// worktree 一覧に 1 つだけ載せて、そこを選ばせる。
pub fn select_only_worktree(ws: &mut Workspace, svc: &mut Services<TaskResult>, path: &Path) {
    let info = WorktreeInfo {
        path: PathBuf::from(path),
        branch: "main".into(),
        is_main: true,
        added: 0,
        modified: 0,
        deleted: 0,
        staged: 0,
        is_clean: true,
        ahead: None,
        behind: None,
        head_oid: None,
        head_time: None,
    };
    let effects = ws.accept(TaskResult::Worktrees(Ok(vec![info])));
    apply(ws, svc, effects);
    pump(ws, svc);
}
