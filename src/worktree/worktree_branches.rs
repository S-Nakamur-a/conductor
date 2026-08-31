//! [App] のブランチ/コミット周辺操作: switch/base/grab の各ブランチ一覧と
//! フィルタリング、worktree の pull、古い worktree の prune、cherry-pick。

use crate::app::*;
use crate::git_engine;

impl App {
    /// 古い worktree を全て prune する。
    pub fn execute_prune(&mut self) {
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => {
                let mut pruned = 0;
                for name in &self.overlays.prune.stale {
                    match engine.prune_stale_worktree(name) {
                        Ok(()) => pruned += 1,
                        Err(e) => {
                            log::warn!("failed to prune worktree '{name}': {e}");
                        }
                    }
                }
                self.set_status(
                    format!("Pruned {pruned} stale worktree(s)."),
                    StatusLevel::Success,
                );
                self.overlays.prune.stale.clear();
                self.refresh_worktrees();
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// switch オーバーレイ用にリモートブランチを読み込む。
    ///
    /// まずキャッシュ済みの ref から即座にリストを埋め、その後バックグラウンドで
    /// fetch を開始する。fetch が完了すると poll_bg_branches() が更新された
    /// リストを拾い上げ、ブロッキングなしでオーバーレイが更新される。
    pub fn load_switch_branches(&mut self) {
        // キャッシュ済みの ref を即座に表示する。
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.list_remote_branches() {
                Ok(branches) => {
                    self.overlays.switch_branch.branches = branches;
                    self.overlays.switch_branch.selected = 0;
                    self.overlays.switch_branch.filter.clear();
                }
                Err(e) => {
                    self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                    self.overlays.switch_branch.branches.clear();
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
                return;
            }
        }

        // バックグラウンドで fetch し、更新されたブランチ一覧を送り返す。
        let repo_path = self.repo.path.clone();
        self.bg.branch.start(move |tx| {
            let engine = match git_engine::GitEngine::open(&repo_path) {
                Ok(e) => e,
                Err(err) => {
                    log::warn!("bg fetch: failed to open repo: {err}");
                    return;
                }
            };
            if let Err(e) = engine.fetch_origin() {
                log::warn!("bg fetch failed: {e}");
            }
            match engine.list_remote_branches() {
                Ok(branches) => {
                    let _ = tx.send(branches);
                }
                Err(e) => {
                    log::warn!("bg list_remote_branches failed: {e}");
                }
            }
        });
    }

    /// バックグラウンドの fetch が完了したかを確認し、新しいデータがあれば
    /// switch-branch のリストを更新する。ノンブロッキング。
    pub fn poll_bg_branches(&mut self) {
        if let Some(branches) = self.bg.branch.poll() {
            // 可能な限り、ユーザの現在のフィルタ/選択を保つ。
            let prev_selected_name = self
                .filtered_switch_branches()
                .get(self.overlays.switch_branch.selected)
                .map(|(_, name)| (*name).clone());
            self.overlays.switch_branch.branches = branches;
            // 名前で選択の復元を試みる。
            if let Some(name) = prev_selected_name
                && let Some(pos) = self
                    .filtered_switch_branches()
                    .iter()
                    .position(|(_, b)| **b == name)
            {
                self.overlays.switch_branch.selected = pos;
            }
            self.bg.branch.clear();
        }
    }

    // worktree の pull (fetch + fast-forward)

    /// 選択中の worktree に対してバックグラウンドの pull(fetch + fast-forward)を開始する。
    pub fn start_pull_worktree(&mut self) {
        if self.bg.pull.is_running() {
            self.set_status(
                "A pull is already in progress.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }

        let wt = match self.worktrees.selected() {
            Some(wt) => wt,
            None => return,
        };

        let branch = wt.branch.clone();
        let wt_path = wt.path.clone();
        let repo_path = self.repo.path.clone();

        self.set_status(format!("Pulling '{branch}'..."), StatusLevel::Info);

        self.bg.pull.start(move |tx| {
            let result = (|| -> Result<String, String> {
                let engine = git_engine::GitEngine::open(&repo_path)
                    .map_err(|e| format!("Failed to open repo: {e}"))?;
                engine.pull_worktree(&wt_path).map_err(|e| format!("{e}"))
            })();
            let _ = tx.send(result);
        });
    }

    /// バックグラウンドの pull チャンネルをポーリングする。ノンブロッキング。
    pub fn poll_bg_pull(&mut self) {
        if let Some(result) = self.bg.pull.poll() {
            match result {
                Ok(msg) => {
                    let level = if msg.contains("up-to-date") {
                        StatusLevel::Info
                    } else if msg.contains("fast-forward") {
                        StatusLevel::Success
                    } else {
                        StatusLevel::Warning
                    };
                    self.set_status(msg, level);
                    self.refresh_worktrees();
                }
                Err(err) => {
                    self.set_status(format!("Pull failed: {err}"), StatusLevel::Error);
                }
            }
        }
    }

    /// 現在のフィルタに基づいて絞り込んだ switch ブランチの一覧を返す。
    pub fn filtered_switch_branches(&self) -> Vec<(usize, &String)> {
        if self.overlays.switch_branch.filter.is_empty() {
            self.overlays
                .switch_branch
                .branches
                .iter()
                .enumerate()
                .collect()
        } else {
            let filter_lower = self.overlays.switch_branch.filter.to_lowercase();
            self.overlays
                .switch_branch
                .branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    pub fn filtered_grab_branches(&self) -> Vec<(usize, &String)> {
        if self.overlays.grab.filter.is_empty() {
            self.overlays.grab.branches.iter().enumerate().collect()
        } else {
            let filter_lower = self.overlays.grab.filter.to_lowercase();
            self.overlays
                .grab
                .branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// worktree 作成のベースとして使えるブランチを読み込む。
    /// リモートブランチを一覧し、origin/<main_branch> をあらかじめ選択しておく。
    pub fn load_base_branches(&mut self) {
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => {
                // リモート追跡ブランチを優先する。リポジトリにリモートがない場合
                // (ローカル専用プロジェクトなど)はローカルブランチにフォール
                // バックする。そうしないとピッカーが空になり何も選べなくなる。
                let branches = match engine.list_remote_branches() {
                    Ok(remote) if !remote.is_empty() => Ok(remote),
                    Ok(_) => engine.list_local_branches(),
                    Err(e) => Err(e),
                };
                match branches {
                    Ok(branches) => {
                        self.worktree_mgr.base_branch_list = branches;
                        self.worktree_mgr.base_branch_selected = 0;
                        self.worktree_mgr.base_branch_filter.clear();
                        // origin/<main_branch>、リモートがなければローカルの
                        // <main_branch> をあらかじめ選択しておく。
                        let main_branch = self.config.general.main_branch.clone();
                        let remote_base = format!("origin/{main_branch}");
                        if let Some(pos) = self
                            .worktree_mgr
                            .base_branch_list
                            .iter()
                            .position(|b| b == &remote_base || b == &main_branch)
                        {
                            self.worktree_mgr.base_branch_selected = pos;
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.worktree_mgr.base_branch_list.clear();
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// 現在のフィルタに基づいて絞り込んだベースブランチの一覧を返す。
    pub fn filtered_base_branches(&self) -> Vec<(usize, &String)> {
        if self.worktree_mgr.base_branch_filter.is_empty() {
            self.worktree_mgr
                .base_branch_list
                .iter()
                .enumerate()
                .collect()
        } else {
            let filter_lower = self.worktree_mgr.base_branch_filter.to_lowercase();
            self.worktree_mgr
                .base_branch_list
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// grab のブランチ候補(main 以外の worktree のブランチ)を読み込む。
    pub fn load_grab_branches(&mut self) {
        self.overlays.grab.branches = self
            .worktrees
            .iter()
            .filter(|w| !w.is_main)
            .map(|w| w.branch.clone())
            .collect();
        self.overlays.grab.selected = 0;
    }

    // Cherry-pick 用ヘルパー

    pub fn load_cherry_pick_commits(&mut self) {
        let branch = self.overlays.cherry_pick.source_branch.clone();
        if branch.is_empty() {
            self.overlays.cherry_pick.commits.clear();
            return;
        }
        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.list_branch_commits(&branch, 20) {
                Ok(commits) => {
                    self.overlays.cherry_pick.commits = commits;
                    self.overlays.cherry_pick.selected = 0;
                }
                Err(e) => {
                    log::warn!("failed to list commits for branch '{branch}': {e}");
                    self.overlays.cherry_pick.commits.clear();
                }
            },
            Err(e) => {
                log::warn!("failed to open git repository for cherry-pick: {e}");
                self.overlays.cherry_pick.commits.clear();
            }
        }
    }

    pub fn execute_cherry_pick(&mut self) {
        let commit = match self
            .overlays
            .cherry_pick
            .commits
            .get(self.overlays.cherry_pick.selected)
        {
            Some(c) => c.clone(),
            None => {
                self.set_status("No commit selected.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let wt_path = match self.worktrees.selected() {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("No worktree selected.".to_string(), StatusLevel::Error);
                return;
            }
        };

        match git_engine::GitEngine::open(&self.repo.path) {
            Ok(engine) => match engine.cherry_pick_to_worktree(&wt_path, &commit.oid) {
                Ok(msg) => {
                    self.set_status(msg, StatusLevel::Success);
                    self.refresh_worktrees();
                }
                Err(e) => {
                    self.set_status(format!("Cherry-pick error: {e}"), StatusLevel::Error);
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }
}
