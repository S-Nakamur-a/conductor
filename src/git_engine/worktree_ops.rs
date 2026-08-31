//! worktree / ブランチの列挙と、worktree ごとのステータススナップショット。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use git2::{Repository, StatusOptions, StatusShow};

use super::{GitEngine, WorktreeInfo};

impl GitEngine {
    // worktree の列挙

    /// 全 worktree(main と linked のすべて)を、ブランチ・ステータス件数・
    /// 最終コミット情報とともに一覧する。
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let mut infos: Vec<WorktreeInfo> = Vec::new();

        // 1. main worktree — .git/ を所有しているもの
        let main_path = self.main_worktree_path()?;
        match self.worktree_info_at(&main_path, true) {
            Ok(info) => infos.push(info),
            Err(e) => {
                log::warn!(
                    "failed to inspect main worktree at {}: {e}",
                    main_path.display()
                );
            }
        }

        // 2. libgit2 が報告する linked worktree
        if let Ok(worktree_names) = self.repo.worktrees() {
            for name in worktree_names.iter().flatten() {
                match self.linked_worktree_info(name) {
                    Ok(info) => infos.push(info),
                    Err(e) => {
                        log::warn!("failed to inspect linked worktree '{name}': {e}");
                    }
                }
            }
        }

        Ok(infos)
    }

    // ローカルブランチの一覧取得

    /// ローカルブランチ名すべてをソート済みの一覧として返す。
    pub fn list_local_branches(&self) -> Result<Vec<String>> {
        let branches = self.repo.branches(Some(git2::BranchType::Local))?;
        let mut names: Vec<String> = branches
            .filter_map(|b| {
                let (branch, _) = b.ok()?;
                branch.name().ok()?.map(String::from)
            })
            .collect();
        names.sort();
        Ok(names)
    }

    // ブランチ prefix のヘルパー

    /// 短いディレクトリ名を得るため、よくあるブランチ prefix(feature/、
    /// fix/ など)を取り除く。
    pub fn strip_branch_prefix(branch: &str) -> &str {
        for prefix in &[
            "feature/", "fix/", "bugfix/", "hotfix/", "release/", "chore/",
        ] {
            if let Some(rest) = branch.strip_prefix(prefix) {
                return rest;
            }
        }
        branch
    }

    /// worktree のベースディレクトリを返す。
    ///
    /// 解決順序:
    /// 1. 環境変数 CONDUCTOR_WORKTREE_DIR
    /// 2. override_dir(config の general.worktree_dir から)
    /// 3. デフォルト: <main-repo-parent>/<repo-name>-worktrees/
    ///
    /// ディレクトリが存在しなければ作成する。
    pub fn worktrees_base_dir(&self, override_dir: Option<&Path>) -> Result<PathBuf> {
        let base = if let Ok(env_dir) = std::env::var("CONDUCTOR_WORKTREE_DIR") {
            PathBuf::from(env_dir)
        } else if let Some(dir) = override_dir {
            dir.to_path_buf()
        } else {
            let main_path = self.main_worktree_path()?;
            let repo_name = main_path
                .file_name()
                .ok_or_else(|| anyhow!("cannot determine repo name"))?
                .to_string_lossy();
            let parent = main_path
                .parent()
                .ok_or_else(|| anyhow!("cannot determine parent directory"))?;
            parent.join(format!("{repo_name}-worktrees"))
        };
        if !base.exists() {
            std::fs::create_dir_all(&base).with_context(|| {
                format!("failed to create worktrees base dir: {}", base.display())
            })?;
        }
        Ok(base)
    }

    // 内部: worktree ごとのステータススナップショット

    /// libgit2 の名前で識別される linked worktree の WorktreeInfo を
    /// 構築する。
    pub(super) fn linked_worktree_info(&self, name: &str) -> Result<WorktreeInfo> {
        let wt = self.repo.find_worktree(name)?;
        let wt_path = wt.path().to_path_buf();

        self.worktree_info_at(&wt_path, false)
    }

    /// path のリポジトリを開いて WorktreeInfo を構築する。
    pub(super) fn worktree_info_at(&self, path: &Path, is_main: bool) -> Result<WorktreeInfo> {
        let repo = Repository::open(path)
            .with_context(|| format!("cannot open repo at {}", path.display()))?;

        let branch = Self::current_branch_name(&repo);
        let (added, modified, deleted, staged) = Self::status_counts(&repo).unwrap_or((0, 0, 0, 0));
        let is_clean = added == 0 && modified == 0 && deleted == 0;
        let (ahead, behind) = Self::ahead_behind_upstream(&repo);
        let head = repo.head().ok();
        let head_oid = head
            .as_ref()
            .and_then(|h| h.target())
            .map(|oid| oid.to_string());
        // committer の時刻。commit / amend / rebase / merge のどれも新しい
        // 時刻を刻むので、「この時刻より古い成果物は、いまの HEAD より前の
        // ものだ」と言える。repo はもう開いているので追加のコストはほぼ無い。
        let head_time = head
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| c.time().seconds());

        Ok(WorktreeInfo {
            path: path.to_path_buf(),
            branch,
            is_main,
            added,
            modified,
            deleted,
            staged,
            is_clean,
            ahead,
            behind,
            head_oid,
            head_time,
        })
    }

    /// upstream の追跡ブランチに対する ahead/behind の件数を計算する。
    /// upstream が無い場合やエラー時は (None, None) を返す。
    fn ahead_behind_upstream(repo: &Repository) -> (Option<usize>, Option<usize>) {
        let head = match repo.head() {
            Ok(h) if h.is_branch() => h,
            _ => return (None, None),
        };
        let local_oid = match head.target() {
            Some(oid) => oid,
            None => return (None, None),
        };
        let branch_name = match head.shorthand() {
            Some(name) => name.to_string(),
            None => return (None, None),
        };
        let branch = match repo.find_branch(&branch_name, git2::BranchType::Local) {
            Ok(b) => b,
            Err(_) => return (None, None),
        };
        let upstream = match branch.upstream() {
            Ok(u) => u,
            Err(_) => return (None, None),
        };
        let upstream_oid = match upstream.get().target() {
            Some(oid) => oid,
            None => return (None, None),
        };
        match repo.graph_ahead_behind(local_oid, upstream_oid) {
            Ok((ahead, behind)) => (Some(ahead), Some(behind)),
            Err(_) => (None, None),
        }
    }

    /// 現在のブランチ名を取得する。detached の場合は "HEAD (detached)"。
    fn current_branch_name(repo: &Repository) -> String {
        if let Ok(head) = repo.head()
            && head.is_branch()
            && let Some(name) = head.shorthand()
        {
            return name.to_string();
        }
        "HEAD (detached)".to_string()
    }

    /// 最初の 3 つはファイルごとに 1 バケットだが、staged はそれらと重複してよく、
    /// git add / git reset に反応する唯一の値になる。
    fn status_counts(repo: &Repository) -> Result<(usize, usize, usize, usize)> {
        let mut opts = StatusOptions::new();
        opts.show(StatusShow::IndexAndWorkdir)
            .include_untracked(true)
            .renames_head_to_index(true);

        let statuses = repo.statuses(Some(&mut opts))?;

        let mut added: usize = 0;
        let mut modified: usize = 0;
        let mut deleted: usize = 0;
        let mut staged: usize = 0;

        for entry in statuses.iter() {
            let s = entry.status();
            // 以下の3つの合計とは別に(そして重複して)数える: あちらは
            // index を先にチェックしてファイルごとに1つのバケットを選ぶので、
            // 変更済みファイルをステージしてもどれも変わらない。これは
            // git add / git reset で動く唯一の数値で、poll ループに
            // git status を読み直させ、Explorer のステージ色を追従させる
            // トリガーになる。
            if s.intersects(
                git2::Status::INDEX_NEW
                    | git2::Status::INDEX_MODIFIED
                    | git2::Status::INDEX_DELETED
                    | git2::Status::INDEX_RENAMED
                    | git2::Status::INDEX_TYPECHANGE,
            ) {
                staged += 1;
            }
            // index の変更
            if s.intersects(git2::Status::INDEX_NEW) {
                added += 1;
            } else if s.intersects(
                git2::Status::INDEX_MODIFIED
                    | git2::Status::INDEX_RENAMED
                    | git2::Status::INDEX_TYPECHANGE,
            ) {
                modified += 1;
            } else if s.intersects(git2::Status::INDEX_DELETED) {
                deleted += 1;
            }
            // working-directory の変更(上の index 側ですでにカウント済み
            // でなければカウントする)。各ファイルが高々1回しか数えられ
            // ないよう else if チェーンを使う。
            else if s.intersects(git2::Status::WT_NEW) {
                added += 1;
            } else if s.intersects(
                git2::Status::WT_MODIFIED | git2::Status::WT_RENAMED | git2::Status::WT_TYPECHANGE,
            ) {
                modified += 1;
            } else if s.intersects(git2::Status::WT_DELETED) {
                deleted += 1;
            }
        }

        Ok((added, modified, deleted, staged))
    }

    // main worktree のパス解決

    /// main(プライマリ)worktree の絶対パスを求める。
    ///
    /// linked worktree から開かれた場合、repo.workdir() は main ではなく
    /// *その* worktree のパスを返す。これは git dir の構造を調べて検出する:
    /// linked worktree の git dir は <main>/.git/worktrees/<name>/ にある。
    pub fn main_worktree_path(&self) -> Result<PathBuf> {
        let git_dir = self.repo.path(); // linked: <main>/.git/worktrees/<name>/
        // main:   <main>/.git/

        // git_dir が .git/worktrees/ の中にあれば、main リポジトリのルートまで遡る。
        if let Some(worktrees_dir) = git_dir.parent()
            && worktrees_dir.file_name() == Some("worktrees".as_ref())
            && let Some(dot_git) = worktrees_dir.parent()
            && let Some(main_repo) = dot_git.parent()
        {
            return Ok(main_repo.to_path_buf());
        }

        // 通常の(bare でない)リポジトリ。
        // repo.workdir() は末尾スラッシュ付きのパスを返すことがある
        // (libgit2 の慣習)。linked-worktree のパスやシェルの $PWD の値と
        // 一致させるため components().collect() で正規化する。
        if let Some(workdir) = self.repo.workdir() {
            return Ok(workdir.components().collect());
        }

        // bare リポジトリ — "main worktree" は git dir の親ディレクトリになる。
        git_dir
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("cannot determine main worktree path"))
    }
}
