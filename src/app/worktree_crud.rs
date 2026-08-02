//! [App] における worktree の作成・削除ライフサイクル。
//!
//! バックグラウンドスレッドでの作成・削除フローを担う: git 操作をワーカー
//! スレッドで起動し、実行中は [PendingWorktree] エントリで追跡し、完了
//! (またはタイムアウト)したら結果を反映する。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use super::*;

impl App {
    /// ベース ref から worktree を作成する(2段階フロー) — バックグラウンドスレッドで実行する。
    pub fn create_worktree_from_base(&mut self, branch_name: &str, base_ref: &str) {
        let base = if base_ref.is_empty() {
            "origin/main"
        } else {
            base_ref
        };

        let pending = PendingWorktree {
            branch: branch_name.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: base.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            session_name: None,
            delete_branch_after: false,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(
            format!("Creating worktree '{branch_name}'..."),
            StatusLevel::Info,
        );

        let tx = self.worktree_op_sender();
        let repo_path = self.repo.path.clone();
        let branch = branch_name.to_string();
        let base_owned = base.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path).and_then(|engine| {
                engine.create_worktree_from_base(&branch, &base_owned, wt_dir.as_deref())
            });
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed {
                    error: format!("{e}"),
                    pending,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// リモートブランチから worktree を作成する — バックグラウンドスレッドで実行する。
    pub fn create_worktree_from_remote(&mut self, remote_branch: &str) {
        let local_branch = remote_branch
            .strip_prefix("origin/")
            .unwrap_or(remote_branch);

        let pending = PendingWorktree {
            branch: local_branch.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: remote_branch.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            session_name: None,
            delete_branch_after: false,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(
            format!("Creating worktree '{local_branch}'..."),
            StatusLevel::Info,
        );

        let tx = self.worktree_op_sender();
        let repo_path = self.repo.path.clone();
        let remote = remote_branch.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.create_worktree_from_remote(&remote, wt_dir.as_deref()));
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed {
                    error: format!("{e}"),
                    pending,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// ブランチを削除する(force 指定可)。
    pub fn delete_branch(&mut self, name: &str, force: bool) {
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.delete_branch(name, force) {
                Ok(()) => {
                    let mode = if force { "force-deleted" } else { "deleted" };
                    self.set_status(format!("Branch {mode}: {name}"), StatusLevel::Success);
                }
                Err(e) => {
                    self.set_status(format!("Branch delete error: {e}"), StatusLevel::Error);
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    pub fn delete_selected_worktree(&mut self, delete_branch_after: bool) {
        let wt = match self.worktrees.selected() {
            Some(wt) => wt,
            None => return,
        };

        if wt.is_main {
            self.set_status(
                "Cannot delete the main worktree.".to_string(),
                StatusLevel::Error,
            );
            return;
        }

        let wt_path = wt.path.clone();
        let branch = wt.branch.clone();

        // worktree ディレクトリを取り除く前に、この worktree に紐づく全ての PTY
        // セッション(Claude Code + Shell)を kill する。まだ処理していない
        // インデックスがずれないよう後ろから走査する。
        let session_indices: Vec<usize> = self
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path)
            .map(|(idx, _)| idx)
            .collect();
        for &idx in session_indices.iter().rev() {
            log::info!("killing PTY session {idx} for deleted worktree '{branch}'");
            self.close_terminal_session(idx);
        }

        // 保留エントリを追加し、git の削除をバックグラウンドスレッドで実行する。
        let pending = PendingWorktree {
            branch: branch.clone(),
            op: PendingWorktreeOp::Deleting,
            base_ref: String::new(),
            worktree_path: Some(wt_path.clone()),
            auto_spawn: false,
            smart_prompt: String::new(),
            session_name: None,
            delete_branch_after,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending);
        self.set_status(
            format!("Deleting worktree '{branch}'..."),
            StatusLevel::Info,
        );

        let tx = self.worktree_op_sender();
        let repo_path = self.repo.path.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.remove_worktree(&wt_path));
            let msg = match result {
                Ok(()) => WorktreeOpResult::Deleted { branch },
                Err(e) => WorktreeOpResult::DeleteFailed {
                    error: format!("{e}"),
                    branch,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// 指定パスの worktree が削除待ちかどうかを確認する。
    pub fn is_worktree_pending_delete(&self, path: &std::path::Path) -> bool {
        self.worktree_mgr.pending_worktrees.iter().any(|p| {
            p.op == PendingWorktreeOp::Deleting && p.worktree_path.as_deref() == Some(path)
        })
    }

    /// バックグラウンドの worktree 作成/削除結果が完了していないか確認する。
    pub fn poll_worktree_ops(&mut self) {
        let rx = match self.worktree_mgr.bg_worktree_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        let mut results = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(result) => results.push(result),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worktree_mgr.bg_worktree_rx = None;
                    self.worktree_mgr.bg_worktree_tx = None;
                    // 二度と完了することのない保留中の create/smart-create エントリを片付ける。
                    let orphaned: Vec<_> = self
                        .worktree_mgr
                        .pending_worktrees
                        .iter()
                        .filter(|p| {
                            matches!(
                                p.op,
                                PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                            )
                        })
                        .map(|p| p.description.clone())
                        .collect();
                    if !orphaned.is_empty() {
                        self.worktree_mgr.pending_worktrees.retain(|p| {
                            !matches!(
                                p.op,
                                PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                            )
                        });
                        log::warn!(
                            "Cleaned up {} orphaned pending worktrees on channel disconnect",
                            orphaned.len()
                        );
                        self.set_status(
                            "Worktree creation interrupted (channel disconnected)".to_string(),
                            StatusLevel::Error,
                        );
                    }
                    break;
                }
            }
        }
        for result in results {
            self.handle_worktree_op_result(result);
        }

        // タイムアウト検出: 保留中の create/smart-create が長時間実行され続けていたら警告する。
        const TIMEOUT_SECS: u64 = 120;
        let now = std::time::Instant::now();
        let timed_out: Vec<_> = self
            .worktree_mgr
            .pending_worktrees
            .iter()
            .filter(|p| {
                matches!(
                    p.op,
                    PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                ) && now.duration_since(p.created_at).as_secs() >= TIMEOUT_SECS
            })
            .map(|p| {
                if p.description.is_empty() {
                    p.branch.clone()
                } else {
                    p.description.clone()
                }
            })
            .collect();
        if !timed_out.is_empty() {
            self.worktree_mgr.pending_worktrees.retain(|p| {
                !(matches!(
                    p.op,
                    PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating
                ) && now.duration_since(p.created_at).as_secs() >= TIMEOUT_SECS)
            });
            let names = timed_out.join(", ");
            log::warn!("Timed out pending worktrees: {names}");
            self.set_status(
                format!("Worktree creation timed out: {names}"),
                StatusLevel::Error,
            );
        }
    }

    fn handle_worktree_op_result(&mut self, result: WorktreeOpResult) {
        match result {
            WorktreeOpResult::Created { path, pending } => {
                // 保留リストから取り除く(Creating と SmartCreating の両方にマッチする)。
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating
                        || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });

                self.new_worktree_paths.insert(path.clone());
                self.record_stat("branches_created");
                if let Some(store) = &self.review_store
                    && is_usable_diff_base(&pending.base_ref)
                {
                    let _ = store.save_worktree_base_branch(&pending.branch, &pending.base_ref);
                }
                self.refresh_worktrees();
                // 現在のフォーカスと選択中の worktree を保つ — 作成したばかりの
                // worktree へユーザのビューを切り替えたりしない。
                let prev_selected = self.worktrees.selected_index();
                let prev_focus = self.focus;
                self.set_status(
                    format!(
                        "Created worktree: {} (from {})",
                        path.display(),
                        pending.base_ref
                    ),
                    StatusLevel::Success,
                );

                // Smart Worktree: Claude Code を自動起動し、セッションが入力可能に
                // なるまでプロンプトを保留する。
                if pending.auto_spawn {
                    // spawn_claude_code が正しい working directory を拾えるよう、
                    // 一時的に新しい worktree を選択する。
                    // on_worktree_changed() が 🌱 の新規 worktree バッジを消して
                    // しまわないよう、select_worktree_by_path ではなくインデックスの
                    // 直接代入を使う。
                    if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
                        self.worktrees.select(idx);
                    }
                    match self.spawn_claude_code_with_name(pending.session_name.as_deref()) {
                        Ok(idx) => {
                            if !pending.smart_prompt.is_empty() {
                                self.terminal
                                    .deferred_prompts
                                    .insert(idx, pending.smart_prompt.clone());
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to auto-spawn Claude Code: {e}");
                        }
                    }
                    // 元の worktree 選択とフォーカスを復元する。
                    self.worktrees.select(prev_selected);
                    self.on_worktree_changed();
                    self.focus = prev_focus;
                }
            }
            WorktreeOpResult::CreateFailed { error, pending } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating
                        || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::Deleted { ref branch } => {
                let delete_branch_after = self.worktree_mgr.pending_worktrees.iter().any(|p| {
                    p.op == PendingWorktreeOp::Deleting
                        && p.branch == *branch
                        && p.delete_branch_after
                });
                self.worktree_mgr
                    .pending_worktrees
                    .retain(|p| !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch));
                self.refresh_worktrees();
                // 今 Explorer/Claude/Shell の各パネルに表示中の worktree が削除
                // された当のものだった場合、refresh_worktrees は選択を生き残って
                // いる worktree(例えば main)へずらしている — が、各パネルは
                // まだ消えた worktree を指したままで空表示になる。通常の切り替え
                // と全く同じように、新しい選択に合わせて再読み込みする。
                // (それ以外の worktree を削除した場合は選択がそのままなので、
                // ここは何もしない。)
                let selected_branch = self.selected_worktree_branch();
                let view_branch = self.view_restore.current_branch.clone().unwrap_or_default();
                if selected_branch != view_branch {
                    self.on_worktree_changed();
                }
                self.set_status(format!("Deleted worktree: {branch}"), StatusLevel::Success);

                if delete_branch_after {
                    self.delete_branch(branch, true);
                }
            }
            WorktreeOpResult::DeleteFailed { error, ref branch } => {
                self.worktree_mgr
                    .pending_worktrees
                    .retain(|p| !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch));
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::SmartBranchResolved {
                ref description,
                ref branch,
                ref prompt,
                ref session_name,
            } => {
                // 保留中のエントリを更新する: ブランチ名、プロンプト、セッション名をセットする。
                for p in &mut self.worktree_mgr.pending_worktrees {
                    if p.op == PendingWorktreeOp::SmartCreating && p.description == *description {
                        p.branch = branch.clone();
                        p.smart_prompt = prompt.clone();
                        p.session_name = session_name.clone();
                        break;
                    }
                }
                self.set_status(
                    format!("Smart worktree: creating '{branch}'... (Esc to cancel)"),
                    StatusLevel::Info,
                );
            }
            WorktreeOpResult::SmartFailed {
                ref description,
                ref error,
            } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::SmartCreating && p.description == *description)
                });
                // 操作がユーザによってキャンセルされた場合はエラーメッセージを抑制する。
                if error == "Cancelled" {
                    log::info!("Smart worktree cancelled for: {description}");
                } else {
                    log::warn!("Smart worktree failed: {error}");
                    self.set_status(
                        format!("Smart worktree failed: {error}"),
                        StatusLevel::Error,
                    );
                }
            }
        }
    }
}

/// base_ref が、単に git worktree add の有効な起点であるというだけでなく、
/// *diff* のベースとして意味を持つかどうか。
///
/// GitEngine::resolve_base_ref は origin/<main> も <main> も存在しないとき
/// リテラル文字列 "HEAD" にフォールバックする — worktree の作成としては
/// 正しいが、それを diff のベースにしても意味がない。ベースとして永続化
/// された "HEAD" は worktree *自身* の head に解決されるため、
/// merge-base(HEAD, HEAD) == HEAD となり、committed セクションは永遠に空
/// のままエラーも出ない: これはまさに、この変更全体が潰そうとしている
/// 「エラーの出ない空表示」が別口から入り込んでくるケースそのものである。
/// ここで弾いておけば main_branch にフォールバックし、それは動くか、
/// あるいはパネル上ではっきり失敗するかのどちらかになる。
fn is_usable_diff_base(base_ref: &str) -> bool {
    !base_ref.is_empty() && base_ref != "HEAD"
}

#[cfg(test)]
mod tests {
    use super::is_usable_diff_base;

    #[test]
    fn head_and_empty_are_rejected_as_diff_bases() {
        assert!(!is_usable_diff_base("HEAD"));
        assert!(!is_usable_diff_base(""));
    }

    #[test]
    fn real_refs_are_accepted() {
        for base in ["main", "origin/main", "release/1.0", "v1.2.3"] {
            assert!(is_usable_diff_base(base), "{base} should be usable");
        }
    }
}
