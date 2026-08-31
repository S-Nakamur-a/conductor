//! [App] における Claude Code セッションの resume。
//!
//! Claude のディスク上の履歴から resume 可能なセッションを読み込み・
//! フィルタリングすること、選んだセッションを ID で resume すること、
//! 各 worktree に以前のセッションを再アタッチする起動時の自動 resume フロー。

use std::path::PathBuf;

use crate::app::*;
use crate::pty_manager;

impl App {
    /// Claude の履歴から resume 可能な Claude Code セッションを読み込む。
    pub fn load_resume_sessions(&mut self) {
        let filter = if self.overlays.resume_session.all_projects {
            None
        } else {
            Some(self.repo.path.as_path())
        };
        match crate::claude_sessions::load_resumable_sessions(filter) {
            Ok(sessions) => {
                self.overlays.resume_session.sessions = sessions;
                self.overlays.resume_session.selected = 0;
                self.overlays.resume_session.filter.clear();
            }
            Err(e) => {
                log::warn!("failed to load resumable sessions: {e}");
                self.overlays.resume_session.sessions.clear();
                self.set_status(format!("Error loading sessions: {e}"), StatusLevel::Error);
            }
        }
    }

    /// 現在のフィルタ文字列に基づいて絞り込んだ resume セッションの一覧を返す。
    pub fn filtered_resume_sessions(
        &self,
    ) -> Vec<(usize, &crate::claude_sessions::ResumableSession)> {
        if self.overlays.resume_session.filter.is_empty() {
            self.overlays
                .resume_session
                .sessions
                .iter()
                .enumerate()
                .collect()
        } else {
            let filter_lower = self.overlays.resume_session.filter.to_lowercase();
            self.overlays
                .resume_session
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.display.to_lowercase().contains(&filter_lower)
                        || s.session_id.to_lowercase().contains(&filter_lower)
                        || s.project_name.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Claude Code セッションをセッション ID で resume する。
    pub fn resume_claude_session(
        &mut self,
        session_id: &str,
        display: &str,
    ) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let label: String = display.chars().take(40).collect();
        let label = if label.is_empty() {
            format!("Resume:{}", &session_id[..8.min(session_id.len())])
        } else {
            label
        };
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.claude.size;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::ClaudeCode,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            Some(session_id),
            &self.repo.path,
            None,
        )?;
        self.switch_claude_session(idx);
        Ok(idx)
    }

    /// 以前セッションがあった全ての worktree について Claude Code セッションを
    /// 自動的に resume する。最初のフレーム描画後に一度だけ呼ばれる。
    pub fn perform_auto_resume(&mut self) {
        if !self.terminal.pending_auto_resume {
            return;
        }
        self.terminal.pending_auto_resume = false;

        let paths: Vec<PathBuf> = self.worktrees.iter().map(|w| w.path.clone()).collect();
        if paths.is_empty() {
            return;
        }

        let sessions = match crate::claude_sessions::find_latest_sessions_for_paths(&paths) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("auto-resume: failed to find sessions: {e}");
                return;
            }
        };

        if sessions.is_empty() {
            return;
        }

        // セッション ID を持つ grab 済みブランチがあれば、通常の auto-resume が
        // 見つけるものの代わりにそちらを main worktree に使う(セッションは
        // main worktree ではなく元の worktree で作られたものだから)。
        let grabbed_session_for_main = self
            .worktree_mgr
            .grabbed_branch
            .as_ref()
            .and_then(|g| g.claude_session_id.clone());

        let selected_wt_path = self.selected_worktree_path();
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.claude.size;
        let repo_path = self.repo.path.clone();
        let mut resumed_count = 0;

        for wt in &self.worktrees.to_vec() {
            let canonical = std::fs::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());

            // grab 済みセッションを持つ main worktree では、grab 済みのセッション
            // ID を優先する。
            if wt.is_main
                && let Some(ref grabbed_id) = grabbed_session_for_main
            {
                let label = format!("Resume:{}", &grabbed_id[..8.min(grabbed_id.len())]);
                match self.terminal.pty_manager.spawn_session(
                    pty_manager::SessionKind::ClaudeCode,
                    &wt.branch,
                    &label,
                    &shell,
                    &wt.path,
                    rows,
                    cols,
                    Some(grabbed_id),
                    &repo_path,
                    None,
                ) {
                    Ok(idx) => {
                        resumed_count += 1;
                        if wt.path == selected_wt_path {
                            self.switch_claude_session(idx);
                        }
                    }
                    Err(e) => {
                        log::warn!("auto-resume: failed to resume grabbed session for main: {e}");
                    }
                }
                continue;
            }

            // 明示的にオプトインしない限り、main worktree の通常の auto-resume は
            // スキップする。grab 済みのセッション(上で処理済み)は常に resume する。
            if wt.is_main && !self.config.general.auto_resume_main {
                continue;
            }

            let session = match sessions.get(&canonical) {
                Some(s) => s,
                None => continue,
            };

            let label: String = session.display.chars().take(40).collect();
            let label = if label.is_empty() {
                format!(
                    "Resume:{}",
                    &session.session_id[..8.min(session.session_id.len())]
                )
            } else {
                label
            };

            match self.terminal.pty_manager.spawn_session(
                pty_manager::SessionKind::ClaudeCode,
                &wt.branch,
                &label,
                &shell,
                &wt.path,
                rows,
                cols,
                Some(&session.session_id),
                &repo_path,
                None,
            ) {
                Ok(idx) => {
                    resumed_count += 1;
                    // 現在選択中の worktree の場合のみこのセッションに切り替える。
                    if wt.path == selected_wt_path {
                        self.switch_claude_session(idx);
                    }
                }
                Err(e) => {
                    log::warn!(
                        "auto-resume: failed to spawn session for {}: {e}",
                        wt.branch
                    );
                }
            }
        }

        if resumed_count > 0 {
            self.set_status(
                format!("Auto-resumed {resumed_count} Claude session(s)"),
                StatusLevel::Success,
            );
        }
    }
}
