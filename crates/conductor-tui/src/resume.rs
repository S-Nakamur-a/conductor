//! 起動時に、前回の Claude セッションを worktree ごとに開き直す。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use conductor_core::claude_sessions::ResumableSession;
use conductor_core::git_engine::WorktreeInfo;

use crate::effect::Effect;
use crate::workspace::{StatusLevel, Workspace};

/// どの worktree でどのセッション id を開き直すか。
///
/// main を既定で外すのは、寿命が長くセッションが積み重なるため。grab したものだけは
/// 例外で、元の worktree で作られたセッションなので一覧から引いたものより優先する。
pub fn plan(
    worktrees: &[WorktreeInfo],
    sessions: &HashMap<PathBuf, ResumableSession>,
    main_grabbed: Option<&str>,
    resume_main: bool,
) -> Vec<(PathBuf, String)> {
    let mut plan = Vec::new();
    for worktree in worktrees {
        let id = match (worktree.is_main, main_grabbed) {
            (true, Some(grabbed)) => grabbed.to_string(),
            (true, None) if !resume_main => continue,
            _ => match sessions.get(&canonical(&worktree.path)) {
                Some(session) => session.session_id.clone(),
                None => continue,
            },
        };
        plan.push((worktree.path.clone(), id));
    }
    plan
}

/// 見つかったセッションを開き直す。1 本失敗しても残りは続ける。
pub fn accept(
    ws: &mut Workspace,
    sessions: HashMap<PathBuf, ResumableSession>,
    main_grabbed: Option<String>,
) -> Vec<Effect> {
    let plan = plan(
        ws.panels.worktree.list(),
        &sessions,
        main_grabbed.as_deref(),
        ws.config.general.auto_resume_main,
    );
    let mut resumed = 0;
    for (worktree, session_id) in plan {
        match ws
            .panels
            .terminal
            .resume(&session_id, &worktree, &ws.repo.root, &ws.config)
        {
            Ok(()) => resumed += 1,
            Err(e) => log::warn!(
                "could not resume the session in '{}': {e:#}",
                worktree.display()
            ),
        }
    }
    if resumed == 0 {
        return Vec::new();
    }
    vec![Effect::Status(
        StatusLevel::Info,
        format!("Resumed {resumed} session(s)"),
    )]
}

/// セッションの一覧は canonicalize したパスで引く。
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Task, TaskResult};
    use conductor_core::config::Config;

    fn worktree(path: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: PathBuf::from(path),
            branch: path.into(),
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

    fn session(id: &str) -> ResumableSession {
        ResumableSession {
            session_id: id.into(),
            display: "prompt".into(),
            project_name: "repo".into(),
            time_ago: "1h ago".into(),
        }
    }

    /// 実在しないパスは canonicalize できないのでそのまま鍵になる。
    fn sessions(pairs: &[(&str, &str)]) -> HashMap<PathBuf, ResumableSession> {
        pairs
            .iter()
            .map(|(path, id)| (PathBuf::from(path), session(id)))
            .collect()
    }

    #[test]
    fn 既定ではmainを飛ばしてリンク先だけ開き直す() {
        let worktrees = [worktree("/repo", true), worktree("/wt/feature", false)];
        let found = sessions(&[
            ("/repo", "main-session"),
            ("/wt/feature", "feature-session"),
        ]);

        assert_eq!(
            plan(&worktrees, &found, None, false),
            [(PathBuf::from("/wt/feature"), "feature-session".to_string())]
        );
        assert_eq!(
            plan(&worktrees, &found, None, true).len(),
            2,
            "auto_resume_main なら main も開き直す"
        );
    }

    #[test]
    fn grabしたセッションはmainの設定に関わらず優先する() {
        let worktrees = [worktree("/repo", true)];
        let found = sessions(&[("/repo", "main-session")]);

        assert_eq!(
            plan(&worktrees, &found, Some("grabbed"), false),
            [(PathBuf::from("/repo"), "grabbed".to_string())]
        );
    }

    fn searches(effects: &[Effect]) -> usize {
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Spawn(Task::FindResumable { .. })))
            .count()
    }

    fn listed() -> TaskResult {
        TaskResult::Worktrees(Ok(vec![worktree("/tmp/repo", true)]))
    }

    #[test]
    fn 一覧が2回届いても探しに行くのは1回だけ() {
        let mut ws = Workspace::for_test();
        assert_eq!(searches(&ws.accept(listed())), 1);
        assert_eq!(searches(&ws.accept(listed())), 0);
    }

    #[test]
    fn auto_resumeを切ってあれば探しに行かない() {
        let mut config = Config::default();
        config.general.auto_resume = false;
        let mut ws = Workspace::for_test_with(config);
        assert_eq!(searches(&ws.accept(listed())), 0);
    }

    #[test]
    fn セッションの無い_worktreeは何も起こさない() {
        let worktrees = [worktree("/wt/a", false), worktree("/wt/b", false)];
        let found = sessions(&[("/wt/b", "b-session")]);

        assert_eq!(
            plan(&worktrees, &found, None, false),
            [(PathBuf::from("/wt/b"), "b-session".to_string())]
        );
        assert!(plan(&worktrees, &HashMap::new(), None, false).is_empty());
    }
}
