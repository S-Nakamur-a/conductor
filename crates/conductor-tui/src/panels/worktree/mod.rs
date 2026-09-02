//! worktree の一覧と、そこへの選択。

pub mod render;

use std::path::{Path, PathBuf};

use conductor_core::git_engine::WorktreeInfo;
use conductor_core::keymap::Action;

use crate::effect::Effect;
use crate::modal::{Confirm, Modal, Prompt};
use crate::task::{Task, TaskResult};
use crate::workspace::{Ctx, StatusLevel};

#[derive(Debug, Default)]
pub struct WorktreePanel {
    list: Vec<WorktreeInfo>,
    /// 選んでいる worktree のパス。添字ではないので、一覧を差し替えても指す先が動かない。
    selected: Option<PathBuf>,
    /// 作成・削除の完了を待っている数。ストリップの回転マーカーが読む。
    pending: usize,
}

impl WorktreePanel {
    pub fn list(&self) -> &[WorktreeInfo] {
        &self.list
    }

    pub fn selected(&self) -> Option<&WorktreeInfo> {
        self.list.get(self.selected_index())
    }

    /// 選択中の添字。一覧に無ければ先頭。
    pub fn selected_index(&self) -> usize {
        self.selected
            .as_ref()
            .and_then(|path| self.list.iter().position(|w| w.path == *path))
            .unwrap_or(0)
    }

    pub fn is_busy(&self) -> bool {
        self.pending > 0
    }

    pub fn select(&mut self, index: usize) {
        if let Some(worktree) = self.list.get(index) {
            self.selected = Some(worktree.path.clone());
        }
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        let index = self.selected_index();
        let last = self.list.len().saturating_sub(1);
        Some(match action {
            Action::NavigateDown => vec![Effect::SelectWorktree((index + 1).min(last))],
            Action::NavigateUp => vec![Effect::SelectWorktree(index.saturating_sub(1))],
            Action::GoToTop => vec![Effect::SelectWorktree(0)],
            Action::GoToBottom => vec![Effect::SelectWorktree(last)],
            Action::Select => vec![
                Effect::SelectWorktree(index),
                Effect::Focus(ctx.focus.next()),
            ],
            Action::RefreshWorktrees => vec![Effect::Spawn(Task::ListWorktrees)],
            Action::CreateWorktree => vec![Effect::PushModal(Modal::Prompt(Prompt {
                title: "New worktree branch".into(),
                input: Default::default(),
                on_submit: |branch| match branch.trim() {
                    "" => vec![Effect::Status(
                        StatusLevel::Warning,
                        "no branch name".into(),
                    )],
                    branch => vec![Effect::Spawn(Task::CreateWorktree {
                        branch: branch.to_string(),
                    })],
                },
            }))],
            Action::DeleteWorktree => vec![self.delete_modal()],
            _ => return None,
        })
    }

    /// 消えるもの (未コミットの変更、main へ入っていないコミット) を確認文に出す。
    /// 反射的な y で黙って失わせないため。
    fn delete_modal(&self) -> Effect {
        let Some(worktree) = self.selected() else {
            return Effect::Status(StatusLevel::Warning, "no worktree selected".into());
        };
        if worktree.is_main {
            return Effect::Status(StatusLevel::Error, "cannot delete the main worktree".into());
        }
        let changes = worktree.added + worktree.modified + worktree.deleted;
        let question = if worktree.is_clean {
            format!("Delete worktree '{}' and its branch?", worktree.branch)
        } else {
            format!(
                "Delete worktree '{}'? {changes} uncommitted change(s) will be LOST.",
                worktree.branch
            )
        };
        Effect::PushModal(Modal::Confirm(Confirm {
            question,
            on_yes: vec![Effect::Spawn(Task::DeleteWorktree {
                path: worktree.path.clone(),
                branch: worktree.branch.clone(),
            })],
        }))
    }

    pub fn apply_result(&mut self, result: TaskResult, ctx: &Ctx) -> Vec<Effect> {
        match result {
            TaskResult::Worktrees(Ok(list)) => {
                self.list = list;
                self.settle_selection(&ctx.repo.root)
            }
            TaskResult::Worktrees(Err(e)) => {
                vec![Effect::Status(
                    StatusLevel::Error,
                    format!("worktrees: {e}"),
                )]
            }
            TaskResult::WorktreeCreated(result) => {
                self.pending = self.pending.saturating_sub(1);
                match result {
                    // 一覧に載るまで選べないので、先に選択だけ移しておく。
                    Ok((path, branch)) => {
                        self.selected = Some(path);
                        vec![
                            Effect::Status(StatusLevel::Success, format!("created '{branch}'")),
                            Effect::Spawn(Task::ListWorktrees),
                        ]
                    }
                    Err(e) => vec![Effect::Status(StatusLevel::Error, format!("create: {e}"))],
                }
            }
            TaskResult::WorktreeDeleted(result) => {
                self.pending = self.pending.saturating_sub(1);
                match result {
                    Ok(branch) => vec![
                        Effect::Status(StatusLevel::Success, format!("deleted '{branch}'")),
                        Effect::Spawn(Task::ListWorktrees),
                    ],
                    Err(e) => vec![Effect::Status(StatusLevel::Error, format!("delete: {e}"))],
                }
            }
            _ => Vec::new(),
        }
    }

    /// 一覧が入れ替わったあとの選択。指していた worktree が消えていたら、
    /// conductor を開いた worktree へ戻す。
    fn settle_selection(&mut self, root: &Path) -> Vec<Effect> {
        let known = self
            .selected
            .as_ref()
            .is_some_and(|path| self.list.iter().any(|w| w.path == *path));
        if known {
            return Vec::new();
        }
        let index = self
            .list
            .iter()
            .position(|w| w.path == root)
            .unwrap_or_default();
        vec![Effect::SelectWorktree(index)]
    }

    /// 走り始めた Task を数える。ストリップのマーカー以外は見ない。
    pub fn note_spawned(&mut self, task: &Task) {
        if matches!(
            task,
            Task::CreateWorktree { .. } | Task::DeleteWorktree { .. }
        ) {
            self.pending += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    fn info(path: &str, branch: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: PathBuf::from(path),
            branch: branch.into(),
            is_main,
            added: 0,
            modified: 0,
            deleted: 0,
            staged: 0,
            is_clean: true,
            ahead: None,
            behind: None,
            head_oid: None,
            head_time: None,
        }
    }

    /// 一覧を流し込んでから Action 列を当て、最後の状態を見る。
    fn drive(actions: &[Action]) -> Workspace {
        let mut ws = Workspace::for_test();
        ws.focus = crate::workspace::Focus::Worktree;
        let list = vec![
            info("/tmp/repo", "main", true),
            info("/tmp/wt/a", "feature/a", false),
            info("/tmp/wt/b", "feature/b", false),
        ];
        let mut svc = conductor_svc::Services::new();
        let effects = ws.accept(TaskResult::Worktrees(Ok(list)));
        crate::effect::apply(&mut ws, &mut svc, effects);
        for action in actions {
            let effects = ws.dispatch(*action).unwrap_or_default();
            crate::effect::apply(&mut ws, &mut svc, effects);
        }
        ws
    }

    #[test]
    fn 一覧が届くと開いた場所が選ばれる() {
        let ws = drive(&[]);
        assert_eq!(ws.panels.worktree.selected_index(), 0);
        assert_eq!(ws.panels.worktree.selected().unwrap().branch, "main");
    }

    #[test]
    fn 移動は両端で止まる() {
        use Action::{GoToBottom, GoToTop, NavigateDown, NavigateUp};
        let cases: [(&[Action], &str); 5] = [
            (&[NavigateDown], "feature/a"),
            (&[NavigateDown, NavigateDown, NavigateDown], "feature/b"),
            (&[NavigateUp], "main"),
            (&[GoToBottom], "feature/b"),
            (&[GoToBottom, GoToTop], "main"),
        ];
        for (actions, expected) in cases {
            let ws = drive(actions);
            let branch = &ws.panels.worktree.selected().unwrap().branch;
            assert_eq!(branch, expected, "{actions:?}");
        }
    }

    #[test]
    fn 選択は添字ではなくパスで覚える() {
        let mut ws = drive(&[Action::GoToBottom]);
        let shorter = vec![info("/tmp/wt/b", "feature/b", false)];
        let effects = ws.accept(TaskResult::Worktrees(Ok(shorter)));
        assert!(effects.is_empty(), "選び直しが起きた: {effects:?}");
        assert_eq!(ws.panels.worktree.selected().unwrap().branch, "feature/b");
    }

    #[test]
    fn 消えた行を指していたら開いた場所へ戻る() {
        let mut ws = drive(&[Action::GoToBottom]);
        let mut svc = conductor_svc::Services::new();
        let remaining = vec![info("/tmp/repo", "main", true)];
        let effects = ws.accept(TaskResult::Worktrees(Ok(remaining)));
        crate::effect::apply(&mut ws, &mut svc, effects);
        assert_eq!(ws.panels.worktree.selected().unwrap().branch, "main");
    }

    #[test]
    fn mainは削除できず確認も出さない() {
        let mut ws = drive(&[Action::DeleteWorktree]);
        assert!(ws.modals.is_empty(), "main で確認が出た");
        assert!(matches!(
            ws.chrome.status.take().map(|s| s.level),
            Some(StatusLevel::Error)
        ));
    }

    #[test]
    fn 削除の確認は失うものを名指しする() {
        let mut ws = drive(&[]);
        let mut dirty = info("/tmp/wt/a", "feature/a", false);
        dirty.is_clean = false;
        dirty.modified = 3;
        let effects = ws.accept(TaskResult::Worktrees(Ok(vec![dirty])));
        let mut svc = conductor_svc::Services::new();
        crate::effect::apply(&mut ws, &mut svc, effects);
        let effects = ws.dispatch(Action::DeleteWorktree).unwrap();
        crate::effect::apply(&mut ws, &mut svc, effects);

        let Some(Modal::Confirm(confirm)) = ws.modals.last() else {
            panic!("確認が出ていない: {:?}", ws.modals);
        };
        assert!(confirm.question.contains("feature/a"));
        assert!(confirm.question.contains('3'), "{}", confirm.question);
    }

    #[test]
    fn 作成は入力を受けてからタスクになる() {
        let ws = drive(&[Action::CreateWorktree]);
        let Some(Modal::Prompt(prompt)) = ws.modals.last() else {
            panic!("入力が出ていない");
        };
        assert!(matches!(
            (prompt.on_submit)("  ".into()).as_slice(),
            [Effect::Status(StatusLevel::Warning, _)]
        ));
        assert!(matches!(
            (prompt.on_submit)("feature/c".into()).as_slice(),
            [Effect::Spawn(Task::CreateWorktree { branch })] if branch == "feature/c"
        ));
    }
}
