//! worktree の一覧と、そこへの選択。

use std::path::{Path, PathBuf};

use conductor_core::git_engine::{GrabState, WorktreeInfo};
use conductor_core::keymap::Action;

use crate::effect::Effect;
use crate::modal::{Confirm, Modal, Mode, Prompt};
use crate::task::{GrabDone, Task, TaskResult};
use crate::workspace::{Ctx, StatusLevel};

#[derive(Debug, Default)]
pub struct WorktreePanel {
    list: Vec<WorktreeInfo>,
    /// 選んでいる worktree のパス。添字ではないので、一覧を差し替えても指す先が動かない。
    selected: Option<PathBuf>,
    listing: bool,
    /// 件数を数えている最中の worktree。
    counting: std::collections::HashSet<PathBuf>,
    /// 一覧に載ったら選び直したい worktree。作った直後や取り込んだ直後はまだ
    /// 載っていないので、パスだけ覚えて次の一覧で拾う。
    pending_select: Option<PathBuf>,
    /// main worktree へ持ってきているブランチ。wt-grab の中身。
    grabbed: Option<GrabState>,
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

    /// 取得中に周期の取り直しを重ねると、175k ファイルのリポジトリ(1 回 25 秒)で 8 本並走して互いを遅くする。
    pub fn is_listing(&self) -> bool {
        self.listing
    }

    pub fn grabbed(&self) -> Option<&GrabState> {
        self.grabbed.as_ref()
    }

    pub fn grab_sources(&self) -> std::collections::HashMap<String, PathBuf> {
        self.list
            .iter()
            .filter(|w| !w.is_main)
            .map(|w| (w.branch.clone(), w.path.clone()))
            .collect()
    }

    pub fn other_branches(&self) -> Vec<String> {
        let current = self.selected().map(|w| w.branch.as_str());
        self.list
            .iter()
            .map(|w| w.branch.clone())
            .filter(|b| Some(b.as_str()) != current)
            .collect()
    }

    pub fn select_when_listed(&mut self, path: PathBuf) {
        self.pending_select = Some(path);
    }

    pub fn select(&mut self, index: usize) {
        if let Some(worktree) = self.list.get(index) {
            self.selected = Some(worktree.path.clone());
        }
    }

    /// メニューとパレットの作成・削除。対象は選択中の worktree。
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::CreateWorktree => vec![create_modal()],
            Action::DeleteWorktree => vec![self.delete_modal(self.selected_index())],
            _ => Vec::new(),
        }
    }

    /// 消えるもの (未コミットの変更、main へ入っていないコミット、止まるセッション) を
    /// 確認文に出す。反射的な y で黙って失わせないため。
    fn delete_modal(&self, index: usize) -> Effect {
        let Some(worktree) = self.list.get(index) else {
            return Effect::Status(StatusLevel::Warning, "no worktree selected".into());
        };
        if worktree.is_main {
            return Effect::Status(StatusLevel::Error, "cannot delete the main worktree".into());
        }
        // 数え直しの最中の件数は前回の値なので、clean と名乗らせない。
        let settled = worktree
            .counts
            .filter(|_| !self.counting.contains(&worktree.path));
        let question = match settled {
            Some(counts) if counts.is_clean() => {
                format!("Delete worktree '{}' and its branch?", worktree.branch)
            }
            Some(counts) => format!(
                "Delete worktree '{}'? {} uncommitted change(s) will be LOST.",
                worktree.branch,
                counts.total()
            ),
            // 数えられなかったのか数え終わっていないのかは区別しない。どちらも
            // 「失うものが無い」と言えないので、確認文は同じでよい。
            None => format!(
                "Delete worktree '{}'? Uncommitted changes could not be confirmed; any will be LOST.",
                worktree.branch
            ),
        };
        Effect::PushModal(Modal::Confirm(
            Confirm::destructive(
                question,
                vec![
                    Effect::StopSessions(Some(worktree.path.clone())),
                    Effect::Spawn(Task::DeleteWorktree {
                        path: worktree.path.clone(),
                        branch: worktree.branch.clone(),
                    }),
                ],
            )
            // 質問は 1 行で折り返さないので、足すと枠からはみ出して読めない。
            .detail("Its Claude Code and shell sessions will be stopped, even mid-task.")
            .title("Delete worktree"),
        ))
    }

    pub fn apply_result(&mut self, result: TaskResult, ctx: &Ctx) -> Vec<Effect> {
        match result {
            TaskResult::Worktrees(Ok(mut list)) => {
                self.listing = false;
                // 一覧が入れ替わるたびに件数を捨てると、表示が ~? に戻り続ける。
                for worktree in &mut list {
                    worktree.counts = worktree.counts.or_else(|| {
                        self.list
                            .iter()
                            .find(|known| known.path == worktree.path)
                            .and_then(|known| known.counts)
                    });
                }
                self.list = list;
                let mut effects = self.count_changes();
                effects.extend(self.settle_selection(&ctx.repo.root));
                effects
            }
            TaskResult::WorktreeCounts { path, counts } => {
                self.counting.remove(&path);
                if let Some(worktree) = self.list.iter_mut().find(|w| w.path == path) {
                    worktree.counts = counts;
                }
                Vec::new()
            }
            TaskResult::GitDone(outcome) => self.git_done(outcome),
            TaskResult::GrabState(state) => {
                self.grabbed = state.unwrap_or_else(|e| {
                    log::warn!("could not read the grab state: {e}");
                    None
                });
                Vec::new()
            }
            TaskResult::Grab(Ok(done)) => self.grab_done(*done),
            TaskResult::Grab(Err(e)) => vec![Effect::Status(StatusLevel::Error, e)],
            TaskResult::StaleWorktrees(Ok(stale)) if stale.is_empty() => {
                vec![Effect::Status(
                    StatusLevel::Info,
                    "No stale worktrees found.".into(),
                )]
            }
            TaskResult::StaleWorktrees(Ok(stale)) => {
                vec![Effect::PushModal(Modal::Confirm(
                    Confirm::destructive(
                        format!(
                            "Prune {} stale worktree(s)? {}",
                            stale.len(),
                            stale.join(", ")
                        ),
                        vec![Effect::Spawn(Task::PruneWorktrees { names: stale })],
                    )
                    .title("Prune worktrees"),
                ))]
            }
            TaskResult::StaleWorktrees(Err(e)) => vec![Effect::Status(StatusLevel::Error, e)],
            TaskResult::Worktrees(Err(e)) => {
                self.listing = false;
                vec![Effect::Status(
                    StatusLevel::Error,
                    format!("worktrees: {e}"),
                )]
            }
            TaskResult::WorktreeCreated(result) => match result {
                Ok(created) => {
                    self.select_when_listed(created.path);
                    let text = format!("created '{}'", created.branch);
                    let status = match created.warning {
                        Some(warning) => {
                            Effect::Status(StatusLevel::Warning, format!("{text} ({warning})"))
                        }
                        None => Effect::Status(StatusLevel::Success, text),
                    };
                    vec![status, Effect::Spawn(Task::ListWorktrees)]
                }
                Err(e) => vec![Effect::Status(StatusLevel::Error, format!("create: {e}"))],
            },
            TaskResult::SmartWorktreeCreated(result) => {
                match result {
                    // 作った worktree へは移らない。書いた人は今見ているものを
                    // 見続けたいので、Claude だけを裏で起こす。
                    Ok(smart) => vec![
                        Effect::Status(
                            StatusLevel::Success,
                            format!("created '{}' and started Claude", smart.branch),
                        ),
                        Effect::SmartSession {
                            worktree: smart.path,
                            name: smart.session_name,
                            prompt: smart.prompt,
                        },
                        Effect::Spawn(Task::ListWorktrees),
                    ],
                    Err(e) => vec![Effect::Status(
                        StatusLevel::Error,
                        format!("smart worktree: {e}"),
                    )],
                }
            }
            TaskResult::WorktreeDeleted(result) => match result {
                Ok(branch) => vec![
                    Effect::Status(StatusLevel::Success, format!("deleted '{branch}'")),
                    Effect::Spawn(Task::ListWorktrees),
                ],
                Err(e) => vec![Effect::Status(StatusLevel::Error, format!("delete: {e}"))],
            },
            _ => Vec::new(),
        }
    }

    fn git_done(&self, outcome: Result<String, String>) -> Vec<Effect> {
        match outcome {
            Ok(message) => vec![
                Effect::Status(level_of(&message), message),
                Effect::Spawn(Task::ListWorktrees),
            ],
            Err(e) => vec![Effect::Status(StatusLevel::Error, e)],
        }
    }

    fn grab_done(&mut self, done: GrabDone) -> Vec<Effect> {
        self.grabbed = done.state;
        let mut effects = vec![
            Effect::Status(StatusLevel::Success, done.message),
            Effect::Spawn(Task::ListWorktrees),
        ];
        if let Some((id, worktree, ran_in)) = done.resume {
            effects.push(Effect::ResumeSession {
                id,
                worktree: Some(worktree),
                chrome: false,
                account: None,
                ran_in: Some(ran_in),
            });
        }
        effects
    }

    /// 選んでいるものから投げるのは、見ている行を先に埋めるため。
    /// 数えている最中のものを飛ばすのは、一覧が 3 秒ごとに来るので、重ねると
    /// 並走だけが増えてどれも返らなくなるため。
    fn count_changes(&mut self) -> Vec<Effect> {
        let selected = self.selected_index();
        let paths: Vec<PathBuf> = std::iter::once(selected)
            .chain((0..self.list.len()).filter(|i| *i != selected))
            .filter_map(|i| self.list.get(i))
            .map(|worktree| worktree.path.clone())
            .filter(|path| !self.counting.contains(path))
            .collect();
        self.counting.extend(paths.iter().cloned());
        paths
            .into_iter()
            .map(|path| Effect::Spawn(Task::WorktreeCounts { path }))
            .collect()
    }

    /// 一覧が入れ替わったあとの選択。指していた worktree が消えていたら、
    /// conductor を開いた worktree へ戻す。
    fn settle_selection(&mut self, root: &Path) -> Vec<Effect> {
        if let Some(wanted) = &self.pending_select
            && let Some(index) = self.list.iter().position(|w| w.path == *wanted)
        {
            self.pending_select = None;
            return vec![Effect::SelectWorktree(index)];
        }
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

    pub fn note_spawned(&mut self, task: &Task) {
        if matches!(task, Task::ListWorktrees) {
            self.listing = true;
        }
    }
}

fn create_modal() -> Effect {
    Effect::PushModal(Modal::Prompt(Prompt::with_modes(
        "New worktree",
        vec![
            Mode {
                label: "Branch name".into(),
                on_submit: |branch| match branch.trim() {
                    "" => vec![Effect::Status(
                        StatusLevel::Warning,
                        "no branch name".into(),
                    )],
                    branch => vec![Effect::Spawn(Task::CreateWorktree {
                        branch: branch.to_string(),
                    })],
                },
            },
            Mode {
                label: "Describe the task".into(),
                on_submit: |description| match description.trim() {
                    "" => vec![Effect::Status(
                        StatusLevel::Warning,
                        "no task description".into(),
                    )],
                    description => vec![Effect::SmartWorktree(description.to_string())],
                },
            },
        ],
    )))
}

/// pull だけは「動かなかった」も成功なので、文言から段を決める。
fn level_of(message: &str) -> StatusLevel {
    if message.contains("up-to-date") {
        StatusLevel::Info
    } else {
        StatusLevel::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandId;
    use crate::workspace::Workspace;
    use conductor_core::git_engine::ChangeCounts;

    fn info(path: &str, branch: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: PathBuf::from(path),
            branch: branch.into(),
            is_main,
            counts: Some(ChangeCounts::default()),
            ahead: None,
            behind: None,
            head_oid: None,
        }
    }

    /// 一覧を流し込んでからコマンド列をメニューと同じ口で当て、最後の状態を見る。
    fn drive(commands: &[CommandId]) -> Workspace {
        let mut ws = Workspace::for_test();
        let list = vec![
            info("/tmp/repo", "main", true),
            info("/tmp/wt/a", "feature/a", false),
            info("/tmp/wt/b", "feature/b", false),
        ];
        let mut svc = conductor_svc::Services::new();
        let paths: Vec<PathBuf> = list.iter().map(|w| w.path.clone()).collect();
        let effects = ws.accept(TaskResult::Worktrees(Ok(list)));
        crate::effect::apply(&mut ws, &mut svc, effects);
        // 数え終わったところから始める。数えかけのままだと投げ直しが見送られる。
        for path in paths {
            ws.accept(TaskResult::WorktreeCounts {
                path,
                counts: Some(ChangeCounts::default()),
            });
        }
        for command in commands {
            let effects = crate::command::execute(&mut ws, *command);
            crate::effect::apply(&mut ws, &mut svc, effects);
        }
        ws
    }

    fn select_last(ws: &mut Workspace) {
        let last = ws.panels.worktree.list().len() - 1;
        let mut svc = conductor_svc::Services::new();
        crate::effect::apply(ws, &mut svc, vec![Effect::SelectWorktree(last)]);
    }

    #[test]
    fn 一覧が届くと開いた場所が選ばれる() {
        let ws = drive(&[]);
        assert_eq!(ws.panels.worktree.selected_index(), 0);
        assert_eq!(ws.panels.worktree.selected().unwrap().branch, "main");
    }

    #[test]
    fn 選択は添字ではなくパスで覚える() {
        let mut ws = drive(&[]);
        select_last(&mut ws);
        let shorter = vec![info("/tmp/wt/b", "feature/b", false)];
        let effects = ws.accept(TaskResult::Worktrees(Ok(shorter)));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::SelectWorktree(_))),
            "選び直しが起きた: {effects:?}"
        );
        assert_eq!(ws.panels.worktree.selected().unwrap().branch, "feature/b");
    }

    #[test]
    fn 一覧が届いたら選んでいるものから件数を数えに行く() {
        let mut ws = drive(&[]);
        let list = vec![
            info("/tmp/repo", "main", true),
            info("/tmp/wt/a", "feature/a", false),
        ];
        ws.panels.worktree.selected = Some(PathBuf::from("/tmp/wt/a"));

        let effects = ws.accept(TaskResult::Worktrees(Ok(list)));
        let counted: Vec<&PathBuf> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Spawn(Task::WorktreeCounts { path }) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(
            counted,
            [&PathBuf::from("/tmp/wt/a"), &PathBuf::from("/tmp/repo")],
            "選んでいるものが先頭でない"
        );
    }

    #[test]
    fn 数えている最中は投げ直さず結果が届くと再開する() {
        let mut ws = drive(&[]);
        let list = vec![WorktreeInfo {
            counts: None,
            ..info("/tmp/wt/a", "feature/a", false)
        }];
        let counts = |effects: &[Effect]| {
            effects
                .iter()
                .filter(|e| matches!(e, Effect::Spawn(Task::WorktreeCounts { .. })))
                .count()
        };

        let first = ws.accept(TaskResult::Worktrees(Ok(list.clone())));
        assert_eq!(counts(&first), 1);

        let again = ws.accept(TaskResult::Worktrees(Ok(list.clone())));
        assert_eq!(counts(&again), 0, "数えている最中に重ねた");

        ws.accept(TaskResult::WorktreeCounts {
            path: PathBuf::from("/tmp/wt/a"),
            counts: Some(ChangeCounts::default()),
        });
        let after = ws.accept(TaskResult::Worktrees(Ok(list)));
        assert_eq!(counts(&after), 1, "届いたのに数え直さない");
    }

    #[test]
    fn 一覧が入れ替わっても数え終わった件数は残る() {
        let mut ws = drive(&[]);
        let fresh = || {
            vec![WorktreeInfo {
                counts: None,
                ..info("/tmp/wt/a", "feature/a", false)
            }]
        };
        ws.accept(TaskResult::Worktrees(Ok(fresh())));
        ws.accept(TaskResult::WorktreeCounts {
            path: PathBuf::from("/tmp/wt/a"),
            counts: Some(ChangeCounts {
                modified: 2,
                ..Default::default()
            }),
        });

        ws.accept(TaskResult::Worktrees(Ok(fresh())));

        assert_eq!(
            ws.panels.worktree.list()[0].counts.map(|c| c.total()),
            Some(2),
            "一覧の入れ替えで件数が ~? に戻った"
        );
    }

    #[test]
    fn 数え終わった件数は該当する_worktree_だけに入る() {
        let mut ws = drive(&[]);
        // 1 段目は件数を持たずに届く。どちらもまだ数えたことのない worktree。
        let list = vec![
            WorktreeInfo {
                counts: None,
                ..info("/tmp/fresh/x", "feature/x", false)
            },
            WorktreeInfo {
                counts: None,
                ..info("/tmp/fresh/y", "feature/y", false)
            },
        ];
        ws.accept(TaskResult::Worktrees(Ok(list)));

        ws.accept(TaskResult::WorktreeCounts {
            path: PathBuf::from("/tmp/fresh/y"),
            counts: Some(ChangeCounts {
                modified: 2,
                ..Default::default()
            }),
        });

        let list = ws.panels.worktree.list();
        assert_eq!(list[0].counts, None, "別の worktree に入った");
        assert_eq!(list[1].counts.unwrap().total(), 2);
    }

    #[test]
    fn 消えた行を指していたら開いた場所へ戻る() {
        let mut ws = drive(&[]);
        select_last(&mut ws);
        let mut svc = conductor_svc::Services::new();
        let remaining = vec![info("/tmp/repo", "main", true)];
        let effects = ws.accept(TaskResult::Worktrees(Ok(remaining)));
        crate::effect::apply(&mut ws, &mut svc, effects);
        assert_eq!(ws.panels.worktree.selected().unwrap().branch, "main");
    }

    #[test]
    fn 取得中は周期の取り直しを見送り結果が届くと再開する() {
        let mut panel = WorktreePanel::default();
        assert!(!panel.is_listing());

        panel.note_spawned(&Task::ListWorktrees);
        assert!(panel.is_listing(), "走っている間も投げてしまう");

        let mut ws = drive(&[]);
        ws.panels.worktree.note_spawned(&Task::ListWorktrees);
        ws.accept(TaskResult::Worktrees(Ok(vec![info(
            "/tmp/repo",
            "main",
            true,
        )])));
        assert!(!ws.panels.worktree.is_listing());

        ws.panels.worktree.note_spawned(&Task::ListWorktrees);
        ws.accept(TaskResult::Worktrees(Err("index unreadable".into())));
        assert!(!ws.panels.worktree.is_listing(), "失敗で取り直しが止まった");
    }

    #[test]
    fn mainは削除できず確認も出さない() {
        let mut ws = drive(&[CommandId::DeleteWorktree]);
        assert!(ws.modals.is_empty(), "main で確認が出た");
        let status = ws.chrome.status.take().expect("理由が出ていない");
        assert_eq!(status.level, StatusLevel::Warning);
        assert_eq!(status.text, "the main worktree cannot be deleted");
    }

    fn delete_question(worktree: WorktreeInfo) -> String {
        let mut ws = drive(&[]);
        let mut svc = conductor_svc::Services::new();
        let (path, counts) = (worktree.path.clone(), worktree.counts);
        let effects = ws.accept(TaskResult::Worktrees(Ok(vec![worktree])));
        crate::effect::apply(&mut ws, &mut svc, effects);
        // 数え直しを終わらせる。最中だと確認文は件数を名指しできない。
        ws.accept(TaskResult::WorktreeCounts { path, counts });
        let effects = crate::command::execute(&mut ws, CommandId::DeleteWorktree);
        crate::effect::apply(&mut ws, &mut svc, effects);
        let Some(Modal::Confirm(confirm)) = ws.modals.last() else {
            panic!("確認が出ていない: {:?}", ws.modals);
        };
        confirm.question.clone()
    }

    #[test]
    fn 削除の確認は失うものを名指しする() {
        let mut dirty = info("/tmp/wt/a", "feature/a", false);
        dirty.counts = Some(ChangeCounts {
            modified: 3,
            ..Default::default()
        });
        let question = delete_question(dirty);
        assert!(question.contains("feature/a"));
        assert!(question.contains('3'), "{question}");
    }

    #[test]
    fn 数え直しの最中の削除の確認は前回のcleanを名乗らない() {
        let mut ws = drive(&[]);
        let mut svc = conductor_svc::Services::new();
        let clean = vec![info("/tmp/wt/a", "feature/a", false)];

        // 数え終わって clean。ここで一覧が来ると数え直しが走る。
        let effects = ws.accept(TaskResult::Worktrees(Ok(clean)));
        crate::effect::apply(&mut ws, &mut svc, effects);
        let effects = crate::command::execute(&mut ws, CommandId::DeleteWorktree);
        crate::effect::apply(&mut ws, &mut svc, effects);

        let Some(Modal::Confirm(confirm)) = ws.modals.last() else {
            panic!("確認が出ていない: {:?}", ws.modals);
        };
        assert!(confirm.question.contains("LOST"), "{}", confirm.question);
    }

    #[test]
    fn 数え終わるまでの削除の確認は失うものが無いと名乗らない() {
        // まだ一度も数えていない worktree なので、持ち越せる件数が無い。
        let mut uncounted = info("/tmp/wt/never-counted", "feature/new", false);
        uncounted.counts = None;
        let question = delete_question(uncounted);
        assert!(question.contains("LOST"), "{question}");
    }

    #[test]
    fn 作成は入力を受けてからタスクになる() {
        let ws = drive(&[CommandId::CreateWorktree]);
        let Some(Modal::Prompt(prompt)) = ws.modals.last() else {
            panic!("入力が出ていない");
        };
        assert!(matches!(
            (prompt.mode().on_submit)("  ".into()).as_slice(),
            [Effect::Status(StatusLevel::Warning, _)]
        ));
        assert!(matches!(
            (prompt.mode().on_submit)("feature/c".into()).as_slice(),
            [Effect::Spawn(Task::CreateWorktree { branch })] if branch == "feature/c"
        ));
    }
}
